package io.github.redhatontop.iolite

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.Surface

class VideoDecoder(
    surface: Surface,
    private val width: Int,
    private val height: Int,
    private val requestKeyframe: () -> Unit,
    private val onFailure: () -> Unit,
) {
    private class Frame(val data: ByteArray, val ptsUs: Long)

    private val lock = Any()
    private val freeInputs = ArrayDeque<Int>()
    private val pending = ArrayDeque<Frame>()
    private val thread = HandlerThread("iolite-video").apply { start() }
    private val handler = Handler(thread.looper)
    private var awaitingKeyframe = true
    private var released = false
    private var rendered = false

    private val callback = object : MediaCodec.Callback() {
        override fun onInputBufferAvailable(mediaCodec: MediaCodec, index: Int) {
            synchronized(lock) {
                if (released) return
                freeInputs.addLast(index)
                drain()
            }
        }

        override fun onOutputBufferAvailable(mediaCodec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
            synchronized(lock) {
                if (released) return
                mediaCodec.releaseOutputBuffer(index, true)
                if (!rendered) {
                    rendered = true
                    Log.i(TAG, "first frame rendered ${width}x${height} by $codecName")
                }
            }
        }

        override fun onOutputFormatChanged(mediaCodec: MediaCodec, format: MediaFormat) = Unit

        override fun onError(mediaCodec: MediaCodec, e: MediaCodec.CodecException) {
            Log.w(TAG, "video decoder failed", e)
            synchronized(lock) { if (released) return }
            onFailure()
        }
    }

    private val codecName: String
    private val codec: MediaCodec

    init {
        val info = pickDecoder()
        codecName = info.name
        val format = MediaFormat.createVideoFormat(MIME, width, height).apply {
            setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            setInteger(MediaFormat.KEY_PRIORITY, 0)
            setInteger(MediaFormat.KEY_OPERATING_RATE, 120)
            setInteger("vendor.qti-ext-dec-low-latency.enable", 1)
            setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, width * height)
        }
        codec = MediaCodec.createByCodecName(info.name)
        codec.setCallback(callback, handler)
        codec.configure(format, surface, null, 0)
        codec.start()
        Log.i(TAG, "video decoder $codecName")
    }

    fun submit(data: ByteArray, ptsUs: Long, keyframe: Boolean) {
        var request = false
        synchronized(lock) {
            if (released || (awaitingKeyframe && !keyframe)) return
            if (pending.size >= MAX_PENDING) {
                Log.w(TAG, "decoder is ${pending.size} frames behind; skipping to the next keyframe")
                pending.clear()
                awaitingKeyframe = !keyframe
                request = !keyframe
                if (!keyframe) return@synchronized
            }
            awaitingKeyframe = false
            pending.addLast(Frame(data, ptsUs))
            drain()
        }
        if (request) requestKeyframe()
    }

    fun release() {
        synchronized(lock) {
            if (released) return
            released = true
            freeInputs.clear()
            pending.clear()
            runCatching { codec.stop() }
            codec.release()
        }
        thread.quitSafely()
    }

    private fun drain() {
        while (freeInputs.isNotEmpty() && pending.isNotEmpty()) {
            val index = freeInputs.removeFirst()
            val frame = pending.removeFirst()
            val buffer = codec.getInputBuffer(index) ?: continue
            buffer.clear()
            if (frame.data.size > buffer.remaining()) {
                Log.w(TAG, "dropping a ${frame.data.size}-byte frame; input buffer holds ${buffer.remaining()}")
                freeInputs.addFirst(index)
                awaitingKeyframe = true
                pending.clear()
                handler.post { requestKeyframe() }
                return
            }
            buffer.put(frame.data)
            codec.queueInputBuffer(index, 0, frame.data.size, frame.ptsUs, 0)
        }
    }

    private fun pickDecoder(): MediaCodecInfo {
        val candidates = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.filter { info ->
            !info.isEncoder && info.supportedTypes.any { it.equals(MIME, ignoreCase = true) }
        }
        return candidates.firstOrNull { it.isHardwareAccelerated }
            ?: candidates.firstOrNull()
            ?: error("no $MIME decoder")
    }

    private companion object {
        const val MIME = "video/av01"
        const val TAG = "iolite"
        const val MAX_PENDING = 30
    }
}
