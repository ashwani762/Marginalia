// Chat generation (Gemma) served to the native core from LiteRT-LM.

package app.marginalia

import android.content.Context
import android.util.Log
import com.google.ai.edge.litertlm.Backend as LBackend
import com.google.ai.edge.litertlm.Content
import com.google.ai.edge.litertlm.Contents
import com.google.ai.edge.litertlm.Conversation
import com.google.ai.edge.litertlm.ConversationConfig
import com.google.ai.edge.litertlm.Engine
import com.google.ai.edge.litertlm.EngineConfig
import com.google.ai.edge.litertlm.ExperimentalFlags
import com.google.ai.edge.litertlm.Message
import com.google.ai.edge.litertlm.MessageCallback
import com.google.ai.edge.litertlm.SamplerConfig
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors

/** What the native core calls when the chat model is served from Kotlin. */
interface LlmGenerator {
    fun start(request: Long, json: String)
    fun cancel(request: Long)
}

/**
 * Gemma on Google's LiteRT-LM. The native core hands over a request (messages, photos, limits),
 * and this streams the answer back through [Native.emit]. Requests run one at a time, as the
 * model has a single set of weights and caches.
 */
@OptIn(com.google.ai.edge.litertlm.ExperimentalApi::class)
class LiteRtGenerator(
    private val context: Context,
    private val modelPath: String,
    private val gpu: Boolean,
    /** Speculative decoding: the model drafts a few words ahead and checks them. */
    private val speedUp: Boolean = true,
) : LlmGenerator {
    private var engine: Engine? = null
    private val pool = Executors.newSingleThreadExecutor { r -> Thread(r, "litert-gen") }
    private val live = ConcurrentHashMap<Long, Conversation>()
    private val cancelled = ConcurrentHashMap.newKeySet<Long>()

    val label: String get() = "LiteRT-LM · ${if (gpu) "GPU" else "CPU"}"

    /** Loads the model. Returns null on success or a message explaining why it could not start. */
    fun prepare(): String? = try {
        ExperimentalFlags.enableBenchmark = true
        // Development switches, read from a small file next to the database. Absent in normal use.
        val tune = java.util.Properties().apply {
            runCatching { java.io.File(context.filesDir, "tune.properties").takeIf { it.isFile }?.inputStream()?.use { load(it) } }
        }
        // Speculative decoding (the model drafts a few words ahead and checks them) measured 12 against 9
        // tokens a second on this phone's GPU. On the CPU it gave nothing, so it stays at the model's default.
        ExperimentalFlags.enableSpeculativeDecoding = when (val t = tune.getProperty("mtp")) {
            null -> if (gpu) speedUp else null
            "auto" -> null
            else -> t.toBoolean()
        }
        val ctxTokens = tune.getProperty("maxTokens")?.toIntOrNull() ?: 4096
        val main = if (gpu) LBackend.GPU() else LBackend.CPU()
        // The compiled GPU programs are kept in a place the system will not clear, so a later start is quick.
        val cache = java.io.File(context.filesDir, "gpu-cache").also { it.mkdirs() }
        // Vision rides on the same backend; audio is not used here (Whisper does speech).
        val cfg = EngineConfig(modelPath, main, main, null, ctxTokens, 4, cache.path)
        val e = Engine(cfg)
        e.initialize()
        engine = e
        null
    } catch (t: Throwable) {
        Log.w("Marginalia", "LiteRT-LM failed to start: $t")
        t.message ?: t.toString()
    }

    fun close() {
        runCatching { engine?.close() }
        engine = null
    }

    override fun start(request: Long, json: String) {
        pool.execute { run(request, JSONObject(json)) }
    }

    override fun cancel(request: Long) {
        cancelled.add(request)
        live[request]?.let { runCatching { it.cancelProcess() } }
    }

    private fun run(id: Long, req: JSONObject) {
        val e = engine
        if (e == null) { Native.emit(id, 2, "LiteRT-LM is not loaded"); return }
        var conv: Conversation? = null
        try {
            val msgs = req.getJSONArray("messages")
            val system = StringBuilder()
            val history = ArrayList<Message>()
            var user = ""
            for (i in 0 until msgs.length()) {
                val m = msgs.getJSONObject(i)
                val text = m.getString("text").replace("<__media__>", "").trim()
                when (m.getString("role")) {
                    "system" -> system.append(text)
                    "assistant" -> history.add(Message.model(text))
                    else -> if (i == msgs.length() - 1) user = text else history.add(Message.user(text))
                }
            }
            val images = req.optJSONArray("images")
            val parts = ArrayList<Content>()
            if (images != null) for (i in 0 until images.length()) parts.add(Content.ImageFile(images.getString(i)))
            parts.add(Content.Text(user))

            val maxTokens = req.optInt("max_tokens", 512)
            conv = e.createConversation(
                ConversationConfig(
                    systemInstruction = if (system.isEmpty()) null else Contents.of(system.toString()),
                    initialMessages = history,
                    samplerConfig = SamplerConfig(
                        topK = req.optInt("top_k", 64), topP = req.optDouble("top_p", 0.95),
                        temperature = req.optDouble("temperature", 0.4), seed = 0,
                    ),
                    maxOutputToken = maxTokens,
                ),
            )
            live[id] = conv
            if (id in cancelled) { finish(id, conv, 0); return }
            val done = CountDownLatch(1)
            var failure: Throwable? = null
            conv.sendMessageAsync(
                Contents.of(parts),
                object : MessageCallback {
                    override fun onMessage(message: Message) { Native.emit(id, 0, message.toString()) }
                    override fun onDone() { done.countDown() }
                    override fun onError(throwable: Throwable) { failure = throwable; done.countDown() }
                },
            )
            done.await()
            val f = failure
            if (f != null && id !in cancelled && f.message?.contains("cancel", ignoreCase = true) != true) {
                Native.emit(id, 2, f.message ?: f.toString())
            } else {
                finish(id, conv, 0)
            }
        } catch (t: Throwable) {
            Native.emit(id, 2, t.message ?: t.toString())
        } finally {
            live.remove(id)
            cancelled.remove(id)
            runCatching { conv?.close() }
        }
    }

    private fun finish(id: Long, conv: Conversation, @Suppress("UNUSED_PARAMETER") unused: Int) {
        val b = runCatching { conv.getBenchmarkInfo() }.getOrNull()
        Native.emit(
            id, 1,
            JSONObject().put("prompt_tokens", b?.lastPrefillTokenCount ?: 0).put("generated_tokens", b?.lastDecodeTokenCount ?: 0).toString(),
        )
    }
}
