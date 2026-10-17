package app.marginalia

import android.content.Context
import android.net.Uri
import com.k2fsa.sherpa.onnx.FastClusteringConfig
import com.k2fsa.sherpa.onnx.OfflineSpeakerDiarization
import com.k2fsa.sherpa.onnx.OfflineSpeakerDiarizationConfig
import com.k2fsa.sherpa.onnx.OfflineSpeakerSegmentationModelConfig
import com.k2fsa.sherpa.onnx.OfflineSpeakerSegmentationPyannoteModelConfig
import com.k2fsa.sherpa.onnx.SpeakerEmbeddingExtractorConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream
import org.apache.commons.compress.compressors.bzip2.BZip2CompressorInputStream
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Who spoke when, in seconds from the start of the meeting. */
data class SpeakerSpan(val start: Double, val end: Double, val speaker: Int)

/**
 * Tells speakers apart on the kept audio, after the meeting, on this phone (sherpa-onnx: a small
 * pyannote model finds who talks when and a speaker model groups the voices). The two models are
 * about 36 MB and are fetched once, the only download besides the larger Gemma.
 */
object Diarizer {
    private const val SEG_URL = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2"
    private const val EMB_URL = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_CAM++.onnx"
    private const val SEG_FILE = "pyannote-segmentation-3-0.int8.onnx"
    private const val EMB_FILE = "wespeaker-campp.onnx"
    private const val SAMPLE_RATE = 16_000
    /** How different two voices must be to count as two people. Higher merges more. */
    const val THRESHOLD = 0.5f

    private fun dir(context: Context) = File(ModelInstaller.dir(context), "speakers").also { it.mkdirs() }

    fun ready(context: Context) = File(dir(context), SEG_FILE).isFile && File(dir(context), EMB_FILE).isFile

    /** Downloads what is missing. [onProgress] gets a fraction from 0 to 1. */
    suspend fun fetchModels(context: Context, onProgress: (Float) -> Unit) = withContext(Dispatchers.IO) {
        val d = dir(context)
        if (!File(d, EMB_FILE).isFile) {
            download(EMB_URL, File(d, "$EMB_FILE.part")) { onProgress(it * 0.8f) }
            File(d, "$EMB_FILE.part").renameTo(File(d, EMB_FILE))
        }
        if (!File(d, SEG_FILE).isFile) {
            val tar = File(d, "seg.tar.bz2")
            download(SEG_URL, tar) { onProgress(0.8f + it * 0.18f) }
            TarArchiveInputStream(BZip2CompressorInputStream(tar.inputStream().buffered())).use { t ->
                while (true) {
                    val e = t.nextEntry ?: break
                    if (!e.isDirectory && e.name.endsWith("model.int8.onnx")) {
                        File(d, "$SEG_FILE.part").outputStream().use { out -> t.copyTo(out) }
                        File(d, "$SEG_FILE.part").renameTo(File(d, SEG_FILE))
                    }
                }
            }
            tar.delete()
            if (!File(d, SEG_FILE).isFile) throw IOException("The speaker model was not in the download.")
        }
        onProgress(1f)
    }

    private fun download(url: String, to: File, onProgress: (Float) -> Unit) {
        val c = URL(url).openConnection() as HttpURLConnection
        c.connectTimeout = 20_000
        c.readTimeout = 30_000
        if (c.responseCode != 200) throw IOException("The speaker model download did not start (code ${c.responseCode}).")
        val total = c.contentLengthLong.coerceAtLeast(1)
        c.inputStream.use { src ->
            to.outputStream().use { out ->
                val buf = ByteArray(1 shl 16)
                var done = 0L
                while (true) {
                    val n = src.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    done += n
                    onProgress(done.toFloat() / total)
                }
            }
        }
    }

    /**
     * Runs on the kept audio of one meeting. [numSpeakers] is how many people spoke if known, or 0 to let it
     * decide. [onProgress] gets a fraction.
     */
    suspend fun run(context: Context, parts: List<AudioPart>, numSpeakers: Int, onProgress: (Float) -> Unit, threshold: Float = THRESHOLD): List<SpeakerSpan> = withContext(Dispatchers.Default) {
        val usable = parts.filter { it.exists }.sortedBy { it.offset }
        if (usable.isEmpty()) throw IOException("There is no kept audio for this meeting.")
        // Everything on one timeline, silence where nothing was recorded.
        val tmp = File(context.cacheDir, "diar").also { it.mkdirs() }
        val pieces = ArrayList<Pair<Double, FloatArray>>()
        var total = 0
        for ((i, p) in usable.withIndex()) {
            val wav = File(tmp, "part$i.wav")
            MediaTools.decodeToWav(context, Uri.fromFile(File(p.path)), wav) { f -> onProgress(0.15f * (i + f) / usable.size) }
            val samples = readWav(wav)
            wav.delete()
            pieces += p.offset to samples
            total = maxOf(total, (p.offset * SAMPLE_RATE).toInt() + samples.size)
        }
        if (total > 3 * 3600 * SAMPLE_RATE) throw IOException("That meeting is too long to separate speakers in one go.")
        val all = FloatArray(total)
        for ((off, s) in pieces) System.arraycopy(s, 0, all, (off * SAMPLE_RATE).toInt(), s.size)
        pieces.clear()

        val d = dir(context)
        // A wider step between windows (0.4 of 10 s) keeps the clustering small on long meetings.
        val config = OfflineSpeakerDiarizationConfig(
            OfflineSpeakerSegmentationModelConfig(OfflineSpeakerSegmentationPyannoteModelConfig(File(d, SEG_FILE).path, if (total > 40 * 60 * SAMPLE_RATE) 0.4f else 0.25f), 4, false, "cpu"),
            SpeakerEmbeddingExtractorConfig(File(d, EMB_FILE).path, 4, false, "cpu"),
            FastClusteringConfig(if (numSpeakers > 0) numSpeakers else -1, threshold),
            0.3f, 0.5f,
        )
        val sd = OfflineSpeakerDiarization(null, config)
        try {
            // The library calls this back from native code by its exact JVM signature, so it must be a real
            // class and not a lambda (a lambda has no such method and the app aborts).
            val progress = object : Function3<Int, Int, Long, Int> {
                override fun invoke(done: Int, totalChunks: Int, arg: Long): Int {
                    onProgress(0.15f + 0.85f * done.toFloat() / totalChunks.coerceAtLeast(1))
                    return 0
                }
            }
            val out = sd.processWithCallback(all, progress, 0L)
            out.map { SpeakerSpan(it.start.toDouble(), it.end.toDouble(), it.speaker) }
        } finally {
            sd.release()
            tmp.deleteRecursively()
        }
    }

    private fun readWav(f: File): FloatArray {
        val bytes = f.readBytes()
        val n = (bytes.size - 44) / 2
        val out = FloatArray(n)
        val sb = ByteBuffer.wrap(bytes, 44, n * 2).order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
        for (i in 0 until n) out[i] = sb.get(i) / 32768f
        return out
    }
}
