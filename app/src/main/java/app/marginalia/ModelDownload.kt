// The optional download of the larger Gemma model.

package app.marginalia

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import kotlin.coroutines.coroutineContext
import kotlinx.coroutines.ensureActive

/** The larger Gemma, fetched once on request. It is the only thing the app ever downloads. */
object ModelDownload {
    const val E4B_FILE = "gemma-4-E4B-it.litertlm"
    private const val E4B_URL = "https://huggingface.co/litert-community/gemma-4-E4B-it-litert-lm/resolve/main/$E4B_FILE"
    const val E4B_BYTES = 3_659_530_240L

    fun have(context: Context, file: String): Boolean = File(ModelInstaller.dir(context), file).isFile

    /** Bytes of a half-finished download, so it can carry on instead of starting over. */
    fun partial(context: Context): Long = File(ModelInstaller.dir(context), "$E4B_FILE.part").takeIf { it.isFile }?.length() ?: 0L

    /** Looks for the file in the app's external folder first (pushed by hand), then downloads. */
    suspend fun fetchE4b(context: Context, onProgress: (Long, Long) -> Unit) = withContext(Dispatchers.IO) {
        val target = File(ModelInstaller.dir(context), E4B_FILE)
        if (target.isFile) return@withContext
        val pushed = File(context.getExternalFilesDir("models"), E4B_FILE)
        if (ModelInstaller.freeBytes(context) < E4B_BYTES + (512L shl 20)) {
            throw IOException("Not enough free storage. The larger Gemma AI needs about 4 GB.")
        }
        val part = File(target.parentFile, "$E4B_FILE.part")
        if (pushed.isFile) {
            pushed.inputStream().use { src -> part.outputStream().use { out -> copy(src, out, 0L, pushed.length(), onProgress) } }
        } else {
            var have = part.takeIf { it.isFile }?.length() ?: 0L
            val c = URL(E4B_URL).openConnection() as HttpURLConnection
            c.connectTimeout = 20_000
            c.readTimeout = 30_000
            if (have > 0) c.setRequestProperty("Range", "bytes=$have-")
            val code = c.responseCode
            if (code == 200) have = 0L // the server ignored the range: start over
            else if (code != 206) throw IOException("The download did not start (code $code). Check the connection and try again.")
            c.inputStream.use { src ->
                FileOutputStream(part, have > 0).use { out -> copy(src, out, have, E4B_BYTES, onProgress) }
            }
        }
        if (part.length() < E4B_BYTES * 99 / 100) throw IOException("The download stopped early. Try again and it will carry on.")
        if (!part.renameTo(target)) throw IOException("Could not finish the download.")
    }

    private suspend fun copy(src: java.io.InputStream, out: java.io.OutputStream, start: Long, total: Long, onProgress: (Long, Long) -> Unit) {
        val buf = ByteArray(1 shl 20)
        var done = start
        var last = 0L
        while (true) {
            coroutineContext.ensureActive()
            val n = src.read(buf)
            if (n < 0) break
            out.write(buf, 0, n)
            done += n
            if (done - last >= (4L shl 20)) { last = done; onProgress(done, total) }
        }
        onProgress(done, total)
    }

    fun delete(context: Context) {
        File(ModelInstaller.dir(context), E4B_FILE).delete()
        File(ModelInstaller.dir(context), "$E4B_FILE.part").delete()
    }
}
