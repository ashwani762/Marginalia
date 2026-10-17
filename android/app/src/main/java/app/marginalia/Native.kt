package app.marginalia

import android.os.Build
import java.io.File

/** Receives JSON events from the native library, on whatever thread produced them. */
interface NativeListener {
    fun onEvent(json: String)
}

/**
 * The Java/Kotlin ↔ native bridge (JNI). The library is the Rust core: llama.cpp,
 * whisper.cpp and the retrieval engine. Commands go in as JSON, events come back as JSON,
 * and audio crosses as raw PCM16.
 */
object Native {
    @Volatile
    var variant: String = "not loaded"
        private set

    /**
     * Pick the build that matches this CPU. The i8mm build uses Armv8.6 integer matrix
     * instructions (much faster prompt processing) and would crash on older cores.
     */
    @Synchronized
    fun load() {
        if (variant != "not loaded") return
        val wantFast = Build.SUPPORTED_ABIS.firstOrNull() == "arm64-v8a" && Cpu.hasFeature("i8mm")
        if (wantFast) {
            try {
                System.loadLibrary("marginalia_core_i8mm")
                variant = "arm64 · i8mm"
                return
            } catch (_: UnsatisfiedLinkError) {
            }
        }
        System.loadLibrary("marginalia_core")
        variant = if (Build.SUPPORTED_ABIS.firstOrNull() == "arm64-v8a") "arm64 · dotprod" else Build.SUPPORTED_ABIS.firstOrNull().orEmpty()
    }

    /** Creates the core and starts loading models. Returns `{"ok":{"missing":[…]}}`. */
    @JvmStatic external fun init(dataDir: String, modelsDir: String, tzOffsetSecs: Int, llmBackend: String, externalEmbedder: Boolean): String

    /** Runs a command. Returns `{"ok": value}` or `{"error": "message"}`. Blocks; call off the main thread. */
    @JvmStatic external fun call(method: String, args: String): String

    /** Feeds 16 kHz mono PCM16 from the microphone; returns the input level. */
    @JvmStatic external fun pushAudio(samples: ShortArray, len: Int): Float

    @JvmStatic external fun setListener(listener: NativeListener?)

    /** Chat generations for the LiteRT-LM engine (see [LiteRtGenerator]). */
    @JvmStatic external fun setGenerator(generator: LlmGenerator?)

    /** Embeddings for the search index (see [LiteRtEmbedder]). */
    @JvmStatic external fun setEmbedder(embedder: TextEmbedder?)

    /** A generation streams back to the core: kind 0 text, 1 finished (stats JSON), 2 failed (message). */
    @JvmStatic external fun emit(request: Long, kind: Int, text: String)
}

object Cpu {
    private val features: Set<String> by lazy {
        try {
            File("/proc/cpuinfo").useLines { lines ->
                lines.firstOrNull { it.startsWith("Features") }
                    ?.substringAfter(':')?.trim()?.split(' ')?.toSet().orEmpty()
            }
        } catch (_: Exception) {
            emptySet()
        }
    }

    fun hasFeature(name: String) = name in features
}
