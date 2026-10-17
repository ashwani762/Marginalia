// Writes the meeting audio to a small AAC file so it can be played back and used to tell speakers apart.

package app.marginalia

import android.content.Context
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.util.Log
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * Keeps the recording itself next to its transcript: 16 kHz mono AAC at 24 kbit/s, about 11 MB an hour.
 * It is written as a plain stream of AAC frames, so a recording that is cut off by Android is still
 * playable up to the last frame, with nothing to finish or repair.
 */
class AudioKeeper(private val file: File) {
    private val queue = ArrayBlockingQueue<ShortArray>(900) // 90 s of slack if the disk stalls
    @Volatile private var closing = false
    private var worker: Thread? = null
    @Volatile var dropped = 0
        private set

    fun start() {
        file.parentFile?.mkdirs()
        worker = Thread({ run() }, "audio-keep").also {
            it.priority = Thread.NORM_PRIORITY - 1
            it.start()
        }
    }

    /** Called from the microphone thread, so it never blocks: if the writer is far behind, a frame is dropped. */
    fun offer(buf: ShortArray, n: Int) {
        if (closing) return
        if (!queue.offer(buf.copyOf(n))) dropped++
    }

    fun close() {
        closing = true
        worker?.join(5000)
        worker = null
    }

    private fun run() {
        var codec: MediaCodec? = null
        try {
            val fmt = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_AAC, SAMPLE_RATE, 1).apply {
                setInteger(MediaFormat.KEY_AAC_PROFILE, MediaCodecInfo.CodecProfileLevel.AACObjectLC)
                setInteger(MediaFormat.KEY_BIT_RATE, 24_000)
                setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 16 * 1024)
            }
            codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_AAC).apply {
                configure(fmt, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                start()
            }
            FileOutputStream(file, true).use { out ->
                val info = MediaCodec.BufferInfo()
                var ptsUs = 0L
                var inputDone = false
                while (true) {
                    // feed
                    if (!inputDone) {
                        val idx = codec.dequeueInputBuffer(10_000)
                        if (idx >= 0) {
                            val frame = queue.poll(20, TimeUnit.MILLISECONDS)
                            val ib = codec.getInputBuffer(idx)!!
                            if (frame == null) {
                                if (closing && queue.isEmpty()) {
                                    codec.queueInputBuffer(idx, 0, 0, ptsUs, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                                    inputDone = true
                                } else {
                                    codec.queueInputBuffer(idx, 0, 0, ptsUs, 0)
                                }
                            } else {
                                ib.clear()
                                ib.order(java.nio.ByteOrder.LITTLE_ENDIAN).asShortBuffer().put(frame)
                                codec.queueInputBuffer(idx, 0, frame.size * 2, ptsUs, 0)
                                ptsUs += frame.size * 1_000_000L / SAMPLE_RATE
                            }
                        }
                    }
                    // drain
                    var out_idx = codec.dequeueOutputBuffer(info, 0)
                    while (out_idx >= 0) {
                        val ob = codec.getOutputBuffer(out_idx)!!
                        if (info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                            val frameLen = info.size + 7
                            val header = adts(frameLen)
                            ob.position(info.offset)
                            ob.limit(info.offset + info.size)
                            val body = ByteArray(info.size)
                            ob.get(body)
                            out.write(header)
                            out.write(body)
                        }
                        codec.releaseOutputBuffer(out_idx, false)
                        if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) return
                        out_idx = codec.dequeueOutputBuffer(info, 0)
                    }
                    if (inputDone && closing) {
                        // wait a little for the last frames
                        out_idx = codec.dequeueOutputBuffer(info, 50_000)
                        if (out_idx < 0 && queue.isEmpty()) continue
                        if (out_idx >= 0) codec.releaseOutputBuffer(out_idx, false)
                    }
                }
            }
        } catch (t: Throwable) {
            Log.w("Marginalia", "keeping the audio stopped: $t")
        } finally {
            runCatching { codec?.stop() }
            runCatching { codec?.release() }
        }
    }

    /** ADTS header for one AAC-LC frame at 16 kHz, mono. */
    private fun adts(len: Int): ByteArray {
        val profile = 2 // AAC LC
        val freqIdx = 8 // 16000 Hz
        val chan = 1
        return byteArrayOf(
            0xFF.toByte(), 0xF1.toByte(),
            (((profile - 1) shl 6) + (freqIdx shl 2) + (chan shr 2)).toByte(),
            (((chan and 3) shl 6) + (len shr 11)).toByte(),
            ((len and 0x7FF) shr 3).toByte(),
            (((len and 7) shl 5) + 0x1F).toByte(),
            0xFC.toByte(),
        )
    }

    companion object {
        private const val SAMPLE_RATE = 16_000

        fun dir(context: Context): File = File(context.filesDir, "audio").also { it.mkdirs() }

        fun newFile(context: Context, meetingId: Long): File = File(dir(context), "m$meetingId-${System.currentTimeMillis()}.aac")

        /** Removes everything kept for a meeting. */
        fun deleteFor(context: Context, meetingId: Long) {
            dir(context).listFiles { f -> f.name.startsWith("m$meetingId-") }?.forEach { it.delete() }
        }
    }
}
