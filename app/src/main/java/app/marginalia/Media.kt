// Photo shrinking and audio or video conversion helpers.

package app.marginalia

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.media.MediaCodec
import android.media.MediaExtractor
import android.media.MediaFormat
import android.net.Uri
import androidx.exifinterface.media.ExifInterface
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.max

/** Phone-side media handling: shrink photos for the vision model, turn any audio or video into 16 kHz mono WAV. */
object MediaTools {
    /**
     * Writes an upright JPEG whose longer side is at most [maxEdge]. Phone cameras produce
     * 12 MP+ images; the vision model gains nothing from them and pays for every pixel.
     */
    fun downscaleJpeg(context: Context, source: Uri, dest: File, maxEdge: Int = 1280, quality: Int = 86) {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        context.contentResolver.openInputStream(source)?.use { BitmapFactory.decodeStream(it, null, bounds) }
        if (bounds.outWidth <= 0) throw IOException("Could not read that image")
        var sample = 1
        while (max(bounds.outWidth, bounds.outHeight) / (sample * 2) >= maxEdge) sample *= 2
        val opts = BitmapFactory.Options().apply { inSampleSize = sample }
        val bmp = context.contentResolver.openInputStream(source)?.use { BitmapFactory.decodeStream(it, null, opts) }
            ?: throw IOException("Could not decode that image")
        val orientation = context.contentResolver.openInputStream(source)?.use {
            ExifInterface(it).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)
        } ?: ExifInterface.ORIENTATION_NORMAL
        val m = Matrix()
        when (orientation) {
            ExifInterface.ORIENTATION_ROTATE_90 -> m.postRotate(90f)
            ExifInterface.ORIENTATION_ROTATE_180 -> m.postRotate(180f)
            ExifInterface.ORIENTATION_ROTATE_270 -> m.postRotate(270f)
            ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> m.postScale(-1f, 1f)
            ExifInterface.ORIENTATION_FLIP_VERTICAL -> m.postScale(1f, -1f)
        }
        val scale = maxEdge.toFloat() / max(bmp.width, bmp.height)
        if (scale < 1f) m.postScale(scale, scale)
        val out = if (m.isIdentity) bmp else Bitmap.createBitmap(bmp, 0, 0, bmp.width, bmp.height, m, true)
        dest.parentFile?.mkdirs()
        FileOutputStream(dest).use { out.compress(Bitmap.CompressFormat.JPEG, quality, it) }
        if (out !== bmp) bmp.recycle()
        out.recycle()
    }

    /**
     * Decodes the first audio track of any audio or video file the platform understands
     * (AAC, MP3, Opus, Vorbis, AMR, FLAC… in MP4, 3GP, WebM, OGG, MKV…) into a 16 kHz mono
     * PCM16 WAV. Returns the duration in seconds.
     */
    fun decodeToWav(context: Context, source: Uri, dest: File, onProgress: (Float) -> Unit = {}): Double {
        val ex = MediaExtractor()
        try {
            ex.setDataSource(context, source, null)
            var track = -1
            var format: MediaFormat? = null
            for (i in 0 until ex.trackCount) {
                val f = ex.getTrackFormat(i)
                if (f.getString(MediaFormat.KEY_MIME)?.startsWith("audio/") == true) { track = i; format = f; break }
            }
            if (track < 0 || format == null) throw IOException("There is no audio in that file")
            ex.selectTrack(track)
            val mime = format.getString(MediaFormat.KEY_MIME)!!
            val durationUs = if (format.containsKey(MediaFormat.KEY_DURATION)) format.getLong(MediaFormat.KEY_DURATION) else 0L
            val codec = MediaCodec.createDecoderByType(mime)
            codec.configure(format, null, null, 0)
            codec.start()

            dest.parentFile?.mkdirs()
            val raf = RandomAccessFile(dest, "rw")
            raf.setLength(0)
            raf.write(ByteArray(44)) // header, patched at the end
            var inRate = format.getInteger(MediaFormat.KEY_SAMPLE_RATE)
            var inChannels = format.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
            val resampler = Resampler()
            var samplesWritten = 0L

            val info = MediaCodec.BufferInfo()
            var inputDone = false
            var outputDone = false
            while (!outputDone) {
                if (!inputDone) {
                    val idx = codec.dequeueInputBuffer(10_000)
                    if (idx >= 0) {
                        val buf = codec.getInputBuffer(idx)!!
                        val n = ex.readSampleData(buf, 0)
                        if (n < 0) {
                            codec.queueInputBuffer(idx, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                            inputDone = true
                        } else {
                            codec.queueInputBuffer(idx, 0, n, ex.sampleTime, 0)
                            if (durationUs > 0) onProgress((ex.sampleTime.toFloat() / durationUs).coerceIn(0f, 1f))
                            ex.advance()
                        }
                    }
                }
                val o = codec.dequeueOutputBuffer(info, 10_000)
                when {
                    o == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                        val nf = codec.outputFormat
                        inRate = nf.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                        inChannels = nf.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
                    }
                    o >= 0 -> {
                        val ob: ByteBuffer = codec.getOutputBuffer(o)!!
                        ob.order(ByteOrder.LITTLE_ENDIAN)
                        val shorts = ob.asShortBuffer()
                        val n = info.size / 2
                        val frames = n / inChannels
                        val mono = ShortArray(frames)
                        for (f in 0 until frames) {
                            var sum = 0
                            for (c in 0 until inChannels) sum += shorts.get(f * inChannels + c).toInt()
                            mono[f] = (sum / inChannels).toShort()
                        }
                        val res = resampler.process(mono, inRate, 16_000)
                        val bb = ByteBuffer.allocate(res.size * 2).order(ByteOrder.LITTLE_ENDIAN)
                        bb.asShortBuffer().put(res)
                        raf.write(bb.array(), 0, res.size * 2)
                        samplesWritten += res.size
                        codec.releaseOutputBuffer(o, false)
                        if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) outputDone = true
                    }
                }
            }
            codec.stop(); codec.release()
            // WAV header.
            val dataBytes = samplesWritten * 2
            raf.seek(0)
            val h = ByteBuffer.allocate(44).order(ByteOrder.LITTLE_ENDIAN)
            h.put("RIFF".toByteArray()); h.putInt((36 + dataBytes).toInt()); h.put("WAVE".toByteArray())
            h.put("fmt ".toByteArray()); h.putInt(16); h.putShort(1); h.putShort(1)
            h.putInt(16_000); h.putInt(32_000); h.putShort(2); h.putShort(16)
            h.put("data".toByteArray()); h.putInt(dataBytes.toInt())
            raf.write(h.array())
            raf.close()
            return samplesWritten / 16_000.0
        } finally {
            ex.release()
        }
    }

    /** Streaming linear resampler to 16 kHz. Carries its position across calls. */
    private class Resampler {
        private var pos = 0.0
        private var prev = 0.0

        fun process(input: ShortArray, inRate: Int, outRate: Int): ShortArray {
            if (inRate == outRate || input.isEmpty()) return input
            val step = inRate.toDouble() / outRate
            val out = ShortArray(((input.size - pos) / step).toInt() + 2)
            var n = 0
            var p = pos
            while (true) {
                val i = Math.floor(p).toInt()
                if (i + 1 >= input.size) break
                val a = if (i < 0) prev else input[i].toDouble()
                val b = input[i + 1].toDouble()
                out[n++] = (a + (b - a) * (p - i)).toInt().toShort()
                p += step
            }
            pos = p - input.size
            prev = input.last().toDouble()
            return out.copyOf(n)
        }
    }
}
