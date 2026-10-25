package io.github.redhatontop.iolite

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaFormat
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.LinkedBlockingDeque
import java.util.concurrent.TimeUnit

class AudioPlayer(opusHead: ByteArray) {
    private class Packet(val data: ByteArray, val ptsUs: Long)

    private val packets = LinkedBlockingDeque<Packet>()
    private val codec: MediaCodec
    private val track: AudioTrack
    private val thread: Thread

    @Volatile
    private var running = true

    init {
        val format = MediaFormat.createAudioFormat(MIME, SAMPLE_RATE, CHANNELS).apply {
            setByteBuffer("csd-0", ByteBuffer.wrap(opusHead))
            setByteBuffer("csd-1", ByteBuffer.wrap(littleEndianLong(preSkipNs(opusHead))))
            setByteBuffer("csd-2", ByteBuffer.wrap(littleEndianLong(SEEK_PREROLL_NS)))
        }
        codec = MediaCodec.createDecoderByType(MIME)
        codec.configure(format, null, null, 0)
        codec.start()

        val minBuffer = AudioTrack.getMinBufferSize(
            SAMPLE_RATE,
            AudioFormat.CHANNEL_OUT_STEREO,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        track = AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build(),
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(SAMPLE_RATE)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                    .build(),
            )
            .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
            .setBufferSizeInBytes(minBuffer * 2)
            .build()
        track.play()

        thread = Thread({ decodeLoop() }, "iolite-audio")
        thread.start()
    }

    fun submit(data: ByteArray, ptsUs: Long) {
        if (packets.size >= MAX_QUEUED) packets.pollFirst()
        packets.offerLast(Packet(data, ptsUs))
    }

    fun release() {
        if (!running) return
        running = false
        thread.join()
        runCatching { codec.stop() }
        codec.release()
        track.stop()
        track.release()
    }

    private fun decodeLoop() {
        val info = MediaCodec.BufferInfo()
        while (running) {
            packets.poll(5, TimeUnit.MILLISECONDS)?.let { feed(it) }
            drainOutput(info)
        }
    }

    private fun feed(packet: Packet) {
        val index = codec.dequeueInputBuffer(INPUT_TIMEOUT_US)
        if (index < 0) return
        val buffer = codec.getInputBuffer(index) ?: return
        buffer.clear()
        buffer.put(packet.data)
        codec.queueInputBuffer(index, 0, packet.data.size, packet.ptsUs, 0)
    }

    private fun drainOutput(info: MediaCodec.BufferInfo) {
        while (true) {
            val index = codec.dequeueOutputBuffer(info, 0)
            if (index < 0) return
            val pcm = codec.getOutputBuffer(index)
            if (pcm != null && info.size > 0) {
                val bytes = ByteArray(info.size)
                pcm.position(info.offset)
                pcm.get(bytes)
                track.write(bytes, 0, bytes.size)
            }
            codec.releaseOutputBuffer(index, false)
        }
    }

    private companion object {
        const val MIME = "audio/opus"
        const val SAMPLE_RATE = 48000
        const val CHANNELS = 2
        const val SEEK_PREROLL_NS = 80_000_000L
        const val MAX_QUEUED = 10
        const val INPUT_TIMEOUT_US = 5_000L

        fun preSkipNs(opusHead: ByteArray): Long {
            val preSkip = (opusHead[10].toInt() and 0xff) or ((opusHead[11].toInt() and 0xff) shl 8)
            return preSkip * 1_000_000_000L / SAMPLE_RATE
        }

        fun littleEndianLong(value: Long): ByteArray =
            ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(value).array()
    }
}
