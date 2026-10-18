// Search embeddings (EmbeddingGemma) served to the native core from LiteRT-LM.

package app.marginalia

import android.content.Context
import android.util.Log
import com.google.ai.edge.litertlm.Backend as LBackend
import com.google.ai.edge.litertlm.EmbeddingEngine
import com.google.ai.edge.litertlm.EmbeddingEngineConfig
import com.google.ai.edge.litertlm.EmbeddingOptions
import com.google.ai.edge.litertlm.InputData

/** What the native core calls to turn texts into EmbeddingGemma vectors. */
interface TextEmbedder {
    fun embed(texts: Array<String>): Array<FloatArray>
}

/**
 * EmbeddingGemma 2 (270M, int4) on Google's LiteRT-LM. The 768-dimension vectors it returns are
 * the same size the search index uses, so nothing else changes.
 */
@OptIn(com.google.ai.edge.litertlm.ExperimentalApi::class)
class LiteRtEmbedder(
    private val context: Context,
    private val modelPath: String,
    private val gpu: Boolean,
    /** CPU threads, or null for the library's own choice. */
    private val threads: Int? = null,
) : TextEmbedder {
    private var engine: EmbeddingEngine? = null
    val label: String get() = "LiteRT-LM · ${if (gpu) "GPU" else "CPU"}"

    /** Loads the model. Returns null on success or a message explaining why it could not start. */
    fun prepare(): String? = try {
        val be = if (gpu) LBackend.GPU() else LBackend.CPU(threads, null)
        val e = EmbeddingEngine(EmbeddingEngineConfig(modelPath, be, null, null, java.io.File(context.filesDir, "gpu-cache").also { it.mkdirs() }.path))
        e.initialize()
        engine = e
        null
    } catch (t: Throwable) {
        Log.w("Marginalia", "LiteRT embedder failed to start: $t")
        t.message ?: t.toString()
    }

    @Synchronized
    override fun embed(texts: Array<String>): Array<FloatArray> = try {
        embedNow(texts)
    } catch (t: Throwable) {
        // The core only learns that the embedder threw, so the reason is written down here.
        Log.w("Marginalia", "embedding ${texts.size} texts failed: $t")
        throw t
    }

    private fun embedNow(texts: Array<String>): Array<FloatArray> {
        val e = engine ?: error("embedder is not loaded")
        val options = EmbeddingOptions(true, true, 768)
        // Small batches keep memory flat on big documents.
        val out = ArrayList<FloatArray>(texts.size)
        var i = 0
        while (i < texts.size) {
            val chunk = texts.slice(i until minOf(i + 8, texts.size))
            val res = e.computeEmbeddingBatch(chunk.map { listOf<InputData>(InputData.Text(it)) }, options)
            res.forEach { out += it.embedding }
            i += 8
        }
        return out.toTypedArray()
    }

    fun close() {
        runCatching { engine?.close() }
        engine = null
    }
}
