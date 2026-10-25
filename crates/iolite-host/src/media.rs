//! GStreamer pipelines that turn the screen into AV1 and the speakers into Opus.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;

pub const OPUS_PRE_SKIP: u16 = 312;
pub const MIN_KBPS: u32 = 800;

pub struct Packet {
    pub data: Vec<u8>,
    pub pts_us: u64,
    pub keyframe: bool,
    /// Video frame size from the negotiated caps.
    pub size: Option<(u32, u32)>,
}

pub enum VideoSource {
    PipeWire { node_id: u32 },
    Test,
}

pub struct VideoPipeline {
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
    encoder: gst::Element,
    adaptive: bool,
}

const AV1_TAIL: &str = "av1parse ! video/x-av1,stream-format=obu-stream,alignment=tu \
     ! appsink name=sink sync=false max-buffers=2 drop=false";

impl VideoPipeline {
    /// `size` asks the source for that frame size. A virtual monitor takes it as its own size;
    /// a real one has a fixed size, so it is left out for those.
    pub fn new(
        source: &VideoSource,
        size: Option<(u32, u32)>,
        fps: u32,
        kbps: u32,
    ) -> Result<Self> {
        let dims = size
            .map(|(w, h)| format!(",width={w},height={h}"))
            .unwrap_or_default();
        // PipeWire hands over tiled DMA-BUFs (XR24) at a variable rate; the GL stage converts
        // them to NV12 on the GPU and videorate only stamps a fixed rate on the caps, because
        // vaav1enc's rate control fails on 0/1. The leaky queue drops frames while the encoder
        // or the network is behind, so latency stays flat instead of growing. A drop-only
        // videorate after gldownload floods stderr with GStreamer criticals during caps
        // negotiation, so it runs on the DMA-BUFs instead.
        let (desc, adaptive) = match source {
            VideoSource::PipeWire { node_id } => (
                format!(
                    "pipewiresrc path={node_id} always-copy=false do-timestamp=true keepalive-time=500 \
                     ! queue max-size-buffers=1 max-size-bytes=0 max-size-time=0 leaky=downstream \
                     ! videorate drop-only=true max-rate={fps} \
                     ! video/x-raw(memory:DMABuf){dims},framerate={fps}/1 \
                     ! glupload ! glcolorconvert ! video/x-raw(memory:GLMemory),format=NV12 \
                     ! gldownload ! video/x-raw,format=NV12 \
                     ! vaav1enc name=enc rate-control=cbr bitrate={kbps} target-usage=7 \
                       ref-frames=1 hierarchical-level=1 key-int-max=1024 \
                     ! {AV1_TAIL}"
                ),
                true,
            ),
            VideoSource::Test => (
                format!(
                    "videotestsrc is-live=true pattern=ball \
                     ! video/x-raw{},framerate=30/1 \
                     ! videoconvert ! video/x-raw,format=I420 ! {} ! {AV1_TAIL}",
                    if dims.is_empty() {
                        ",width=1280,height=720"
                    } else {
                        dims.as_str()
                    },
                    software_encoder(kbps)?
                ),
                false,
            ),
        };
        let pipeline = gst::parse::launch(&desc)
            .context("building the video pipeline")?
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow!("video description is not a pipeline"))?;
        let sink = pipeline
            .by_name("sink")
            .context("video pipeline has no sink")?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| anyhow!("video sink is not an appsink"))?;
        let encoder = pipeline
            .by_name("enc")
            .context("video pipeline has no encoder")?;
        Ok(Self {
            pipeline,
            sink,
            encoder,
            adaptive,
        })
    }

    pub fn start(&self) -> Result<()> {
        self.pipeline
            .set_state(gst::State::Playing)
            .context("starting the video pipeline")?;
        Ok(())
    }

    pub fn stop(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    /// Waits up to `timeout` for the next temporal unit.
    pub fn pull(&self, timeout: Duration) -> Result<Option<Packet>> {
        pull(&self.pipeline, &self.sink, timeout)
    }

    pub fn force_keyframe(&self) {
        let event = gst_video::UpstreamForceKeyUnitEvent::builder()
            .all_headers(true)
            .build();
        self.sink.send_event(event);
    }

    pub fn set_bitrate(&self, kbps: u32) {
        if self.adaptive {
            self.encoder.set_property("bitrate", kbps);
        }
    }
}

impl Drop for VideoPipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

fn software_encoder(kbps: u32) -> Result<String> {
    if gst::ElementFactory::find("av1enc").is_some() {
        return Ok(format!(
            "av1enc name=enc usage-profile=realtime cpu-used=10 end-usage=cbr \
             target-bitrate={kbps} lag-in-frames=0 keyframe-max-dist=60 threads=4"
        ));
    }
    if gst::ElementFactory::find("svtav1enc").is_some() {
        return Ok(format!(
            "svtav1enc name=enc preset=12 target-bitrate={kbps}"
        ));
    }
    bail!("no software AV1 encoder (av1enc or svtav1enc) is installed")
}

pub struct AudioPipeline {
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
}

impl AudioPipeline {
    /// Captures what the default output device plays, or a quiet tone for tests.
    pub fn new(test: bool) -> Result<Self> {
        let src = if test {
            "audiotestsrc is-live=true wave=sine freq=440 volume=0.2"
        } else {
            "pipewiresrc stream-properties=\"props,stream.capture.sink=true\" do-timestamp=true"
        };
        let desc = format!(
            "{src} ! queue max-size-time=100000000 leaky=downstream \
             ! audioconvert ! audioresample ! audio/x-raw,format=S16LE,rate=48000,channels=2 \
             ! opusenc bitrate=128000 frame-size=10 audio-type=restricted-lowdelay \
             ! appsink name=sink sync=false max-buffers=50 drop=true"
        );
        let pipeline = gst::parse::launch(&desc)
            .context("building the audio pipeline")?
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow!("audio description is not a pipeline"))?;
        let sink = pipeline
            .by_name("sink")
            .context("audio pipeline has no sink")?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| anyhow!("audio sink is not an appsink"))?;
        Ok(Self { pipeline, sink })
    }

    pub fn start(&self) -> Result<()> {
        self.pipeline
            .set_state(gst::State::Playing)
            .context("starting the audio pipeline")?;
        Ok(())
    }

    pub fn stop(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    pub fn pull(&self, timeout: Duration) -> Result<Option<Packet>> {
        pull(&self.pipeline, &self.sink, timeout)
    }
}

impl Drop for AudioPipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

fn pull(
    pipeline: &gst::Pipeline,
    sink: &gst_app::AppSink,
    timeout: Duration,
) -> Result<Option<Packet>> {
    if let Some(bus) = pipeline.bus() {
        while let Some(msg) = bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos]) {
            match msg.view() {
                gst::MessageView::Error(e) => bail!(
                    "{}: {} ({})",
                    e.src().map(|s| s.path_string()).unwrap_or_default(),
                    e.error(),
                    e.debug().unwrap_or_default()
                ),
                gst::MessageView::Eos(_) => bail!("pipeline ended"),
                _ => {}
            }
        }
    }
    let millis = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(millis)) else {
        if sink.is_eos() {
            bail!("pipeline ended");
        }
        return Ok(None);
    };
    let buffer = sample.buffer().context("sample without a buffer")?;
    let map = buffer.map_readable()?;
    let size = sample
        .caps()
        .and_then(|caps| caps.structure(0))
        .and_then(|s| {
            let w = s.get::<i32>("width").ok()?;
            let h = s.get::<i32>("height").ok()?;
            Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?))
        });
    Ok(Some(Packet {
        data: map.as_slice().to_vec(),
        pts_us: buffer.pts().map(|t| t.useconds()).unwrap_or(0),
        keyframe: !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT),
        size,
    }))
}

/// RFC 7845 identification header for 48 kHz stereo, channel mapping family 0.
pub fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(2);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&48_000u32.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    head
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_head_layout() {
        let head = opus_head(312);
        assert_eq!(head.len(), 19);
        assert_eq!(&head[..8], b"OpusHead");
        assert_eq!(u16::from_le_bytes([head[10], head[11]]), 312);
        assert_eq!(u32::from_le_bytes(head[12..16].try_into().unwrap()), 48_000);
    }
}
