package app.marginalia

import android.content.Context
import android.util.Log
import com.google.ai.edge.litertlm.Backend as LBackend
import com.google.ai.edge.litertlm.ConversationConfig
import com.google.ai.edge.litertlm.Contents
import com.google.ai.edge.litertlm.Engine
import com.google.ai.edge.litertlm.EngineConfig
import com.google.ai.edge.litertlm.ExperimentalFlags
import com.google.ai.edge.litertlm.Message
import com.google.ai.edge.litertlm.SamplerConfig
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.collect
import org.json.JSONObject

/**
 * Debug-only benchmark: the same meeting-notes prompt through two engines, same metrics.
 * Run with `adb shell am start -a app.marginalia.DEBUG_LITERT ...` and read `adb logcat -s MarginaliaDbg`.
 */
@OptIn(com.google.ai.edge.litertlm.ExperimentalApi::class)
object Bench {
    // The kind of prompt the live "Enhance notes" feature sends: a system rule, a recent transcript, shorthand notes.
    const val SYSTEM = "You are Marginalia, an on-device meeting assistant. The user types shorthand notes during a live meeting. Rewrite their notes into clear, complete meeting notes in Markdown. Rules: keep the user's structure, order and intent; expand abbreviations and fragments using the transcript for context; add names, numbers, decisions and action items that the transcript confirms; never invent facts that are in neither the notes nor the transcript; no preamble, output only the notes."

    val TRANSCRIPT = """
[00:04] Okay, let's get started. The goal today is to decide the launch date for the Orchid release and who owns the migration.
[00:19] Priya, where are we on the database migration? Last week you said the staging copy was failing on the orders table.
[00:31] Right, so the failure was a foreign key on the legacy orders table. Marco and I fixed it on Tuesday, staging is clean now, and the production dry run is scheduled for Thursday the twenty-third.
[00:52] If the dry run passes, can we ship on the thirtieth? Design needs the freeze on the twenty-eighth for the final screenshots.
[01:08] The thirtieth works for engineering, but support wants a week of notice before we change the billing page, so we should email customers by the twenty-third.
[01:27] Fine. Decision: launch on October thirtieth, contingent on the Thursday dry run. Priya owns the migration, Marco owns the rollback plan, and Dana sends the customer email by the twenty-third.
[01:49] One open question: do we keep the old export format for enterprise customers, or retire it with this release?
[02:03] Enterprise has about forty accounts still using it. I'd keep it for one more quarter and put a banner in the app announcing the sunset in January.
[02:21] Agreed. Dana, please also draft the banner text. Anything else? Budget for the load test is four thousand dollars, already approved by finance.
[02:40] Great, that's everything. Let's meet again Thursday after the dry run.
""".trim()

    const val NOTES = "launch date? -> oct 30 if dry run ok thu\nmigration - priya, rollback - marco\ndana: cust email by 23rd + banner text\nold export fmt keep 1 more qtr, sunset jan\nload test 4k approved"

    fun userPrompt() = "Transcript so far (most recent last):\n$TRANSCRIPT\n\nMy shorthand notes:\n$NOTES\n\nStyle: Standard: complete sentences, keep every point, group under short headings when natural.\n\nRewrite my notes now."

    /** llama.cpp via the native core. */
    suspend fun llama(backend: app.marginalia.Backend, maxTokens: Int): JSONObject = JSONObject(
        (backend.obj(
            "debug.llm_bench",
            JSONObject().put("system", SYSTEM).put("user", userPrompt()).put("max_tokens", maxTokens),
        )).toString(),
    )

    /** LiteRT-LM on the given backend ("cpu" or "gpu"). */
    suspend fun litert(context: Context, modelPath: String, backend: String, maxTokens: Int, mtp: Boolean): JSONObject {
        val out = JSONObject().put("engine", "LiteRT-LM 0.18.0 ($backend${if (mtp) ", MTP" else ""})")
        ExperimentalFlags.enableBenchmark = true
        ExperimentalFlags.enableSpeculativeDecoding = mtp
        val be = if (backend == "gpu") LBackend.GPU() else LBackend.CPU()
        val cfg = EngineConfig(modelPath, be, null, null, 4096, null, context.cacheDir.path)
        val t0 = System.nanoTime()
        val engine = Engine(cfg)
        try {
            engine.initialize()
            out.put("init_s", (System.nanoTime() - t0) / 1e9)
            val conv = engine.createConversation(
                ConversationConfig(
                    systemInstruction = Contents.of(SYSTEM),
                    samplerConfig = SamplerConfig(topK = 64, topP = 0.95, temperature = 0.2, seed = 1),
                ),
            )
            conv.use {
                val sb = StringBuilder()
                val start = System.nanoTime()
                var first = 0L
                var chunks = 0
                try {
                    conv.sendMessageAsync(userPrompt()).collect { m ->
                        if (first == 0L) first = System.nanoTime()
                        sb.append(m.toString()); chunks++
                        if (sb.length > maxTokens * 5) conv.cancelProcess()
                    }
                } catch (e: Exception) {
                    out.put("note", "stream ended: ${e.message}")
                }
                val end = System.nanoTime()
                out.put("ttft_s_wall", if (first > 0) (first - start) / 1e9 else -1.0)
                out.put("total_s", (end - start) / 1e9)
                out.put("chars", sb.length)
                runCatching { conv.getBenchmarkInfo() }.getOrNull()?.let { b ->
                    out.put("ttft_s", b.timeToFirstTokenInSecond)
                    out.put("prefill_tokens", b.lastPrefillTokenCount)
                    out.put("prefill_tok_s", b.lastPrefillTokensPerSecond)
                    out.put("decode_tokens", b.lastDecodeTokenCount)
                    out.put("decode_tok_s", b.lastDecodeTokensPerSecond)
                    out.put("engine_init_s", b.initTimeInSecond)
                }
                out.put("text", sb.toString().take(1500))
            }
        } finally {
            engine.close()
        }
        return out
    }

    fun log(tag: String, o: Any) = Log.i("MarginaliaDbg", "$tag -> $o")
}
