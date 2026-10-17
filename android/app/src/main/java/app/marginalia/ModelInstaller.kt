package app.marginalia

import android.content.Context
import android.os.StatFs
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException

/**
 * The models ship inside the APK (`assets/models`). The first launch unpacks them into
 * private storage, where llama.cpp can map them. For development they can instead be
 * pushed to the app's external files directory (see scripts/push-models.sh).
 */
object ModelInstaller {
    const val LITERT_MODEL = "gemma-4-E2B-it.litertlm"
    /** EmbeddingGemma 2, text only, int4: 165 MB. */
    const val EMBED_LITERT = "embeddinggemma-2-text-270m.litertlm"
    /** Everything the phone needs: search, speech, and Gemma (with vision) in one file each. */
    val required: List<String> = listOf(EMBED_LITERT, "ggml-small-q8_0.bin", LITERT_MODEL)

    fun dir(context: Context): File = File(context.filesDir, "models").also { it.mkdirs() }

    private fun external(context: Context): File? = context.getExternalFilesDir("models")

    private fun embeddedLength(context: Context, name: String): Long? = try {
        context.assets.openFd("models/$name").use { it.length }
    } catch (_: IOException) {
        null
    }

    fun missing(context: Context): List<String> = required.filter { !File(dir(context), it).isFile }

    /** True when the missing files can be provided from the APK or the external directory. */
    fun canInstall(context: Context): Boolean = missing(context).all { name ->
        embeddedLength(context, name) != null || File(external(context), name).isFile
    }

    /** Bytes still to be written. */
    fun bytesNeeded(context: Context): Long = missing(context).sumOf { name ->
        embeddedLength(context, name) ?: File(external(context), name).length()
    }

    fun freeBytes(context: Context): Long = StatFs(context.filesDir.absolutePath).availableBytes

    /**
     * Copies whatever is missing. [onProgress] gets (bytesDone, bytesTotal, fileName).
     * Throws [IOException] with a readable message on failure.
     */
    fun install(context: Context, onProgress: (Long, Long, String) -> Unit) {
        val todo = missing(context)
        if (todo.isEmpty()) return
        val total = bytesNeeded(context)
        if (freeBytes(context) < total + (256L shl 20)) {
            val gb = "%.1f".format((total + (256L shl 20)) / 1e9)
            throw IOException("Not enough free storage. Marginalia needs about $gb GB to unpack its on-device models.")
        }
        var done = 0L
        val target = dir(context)
        for (name in todo) {
            val part = File(target, "$name.part")
            part.delete()
            val ext = File(external(context), name)
            val input = if (embeddedLength(context, name) != null) context.assets.open("models/$name") else FileInputStream(ext)
            input.use { src ->
                FileOutputStream(part).use { out ->
                    val buf = ByteArray(1 shl 20)
                    var lastReport = 0L
                    while (true) {
                        val n = src.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        done += n
                        if (done - lastReport >= (8L shl 20)) {
                            lastReport = done
                            onProgress(done, total, name)
                        }
                    }
                    out.fd.sync()
                }
            }
            if (!part.renameTo(File(target, name))) throw IOException("Could not finish unpacking $name")
            onProgress(done, total, name)
        }
    }
}
