package app.marginalia

import android.content.Context
import android.net.Uri
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException
import java.util.zip.ZipEntry
import java.util.zip.ZipInputStream
import java.util.zip.ZipOutputStream

/**
 * One file with everything the person made: meetings, transcripts, notes, documents, photos and the
 * kept audio. The models are not in it (they are the same on every phone). It goes wherever the
 * person chooses, and nothing is sent anywhere by the app.
 */
object Backup {
    private const val DB = "marginalia.sqlite3"
    private val files = listOf(DB, "settings.json")
    private val dirs = listOf("photos", "audio", "library")

    /** Writes the backup to [uri] (from the system "create document" screen). Returns the size in bytes. */
    suspend fun export(context: Context, backend: Backend, uri: Uri, progress: (Int) -> Unit = {}): Long = withContext(Dispatchers.IO) {
        runCatching { backend.call("db.checkpoint") } // fold the write-ahead log in, so the database file is whole
        val root = context.filesDir
        val all = ArrayList<File>()
        files.map { File(root, it) }.filter { it.isFile }.forEach { all += it }
        dirs.forEach { d -> File(root, d).walkTopDown().filter { it.isFile }.forEach { all += it } }
        val total = all.sumOf { it.length() }.coerceAtLeast(1)
        var done = 0L
        var written = 0L
        context.contentResolver.openOutputStream(uri, "w")?.use { raw ->
            val counting = object : java.io.FilterOutputStream(raw) {
                override fun write(b: ByteArray, off: Int, len: Int) { out.write(b, off, len); written += len }
                override fun write(b: Int) { out.write(b); written += 1 }
            }
            ZipOutputStream(counting.buffered(1 shl 16)).use { zip ->
                zip.setLevel(1) // audio and photos are already compressed
                zip.putNextEntry(ZipEntry("MARGINALIA-BACKUP-1")).also { zip.closeEntry() }
                for (f in all) {
                    zip.putNextEntry(ZipEntry(f.relativeTo(root).path))
                    f.inputStream().use { it.copyTo(zip) }
                    zip.closeEntry()
                    done += f.length()
                    progress((done * 100 / total).toInt())
                }
            }
        } ?: throw IOException("Could not write the backup file.")
        written
    }

    private fun staging(context: Context) = File(context.filesDir, "restore-staging")

    /**
     * Checks the file and unpacks it next to the live data. Nothing the person has is touched yet:
     * the swap happens on the next start, before the database opens ([applyStaged]).
     */
    suspend fun stage(context: Context, uri: Uri): Unit = withContext(Dispatchers.IO) {
        val dir = staging(context)
        dir.deleteRecursively()
        dir.mkdirs()
        var sawMarker = false
        var sawDb = false
        context.contentResolver.openInputStream(uri)?.use { raw ->
            ZipInputStream(raw.buffered(1 shl 16)).use { zip ->
                while (true) {
                    val e = zip.nextEntry ?: break
                    if (e.name == "MARGINALIA-BACKUP-1") { sawMarker = true; continue }
                    // Only the places a backup writes to, and never outside the folder.
                    val top = e.name.substringBefore('/')
                    if (e.isDirectory || (top != DB && top != "settings.json" && top !in dirs)) continue
                    val out = File(dir, e.name).canonicalFile
                    if (!out.path.startsWith(dir.canonicalPath + File.separator)) throw IOException("The file is not a Marginalia backup.")
                    out.parentFile?.mkdirs()
                    out.outputStream().use { zip.copyTo(it) }
                    if (e.name == DB) sawDb = true
                }
            }
        } ?: throw IOException("Could not read that file.")
        if (!sawMarker || !sawDb) {
            dir.deleteRecursively()
            throw IOException("That file is not a Marginalia backup.")
        }
        val header = File(dir, DB).inputStream().use { s -> ByteArray(16).also { s.read(it) } }
        if (String(header, Charsets.ISO_8859_1) != "SQLite format 3\u0000") {
            dir.deleteRecursively()
            throw IOException("The database inside that backup is damaged.")
        }
    }

    /** True when a staged restore is waiting for the next start. */
    fun hasStaged(context: Context): Boolean = File(staging(context), DB).isFile

    /**
     * Called first thing at start, before anything opens the database. The current data moves aside
     * (kept until the restored data has opened once) and the staged data takes its place.
     */
    fun applyStaged(context: Context) {
        val dir = staging(context)
        if (!File(dir, DB).isFile) return
        val root = context.filesDir
        val aside = File(root, "before-restore").also { it.deleteRecursively(); it.mkdirs() }
        for (name in files + listOf("$DB-wal", "$DB-shm")) File(root, name).takeIf { it.exists() }?.renameTo(File(aside, name))
        for (d in dirs) File(root, d).takeIf { it.exists() }?.renameTo(File(aside, d))
        for (name in files) File(dir, name).takeIf { it.exists() }?.renameTo(File(root, name))
        for (d in dirs) File(dir, d).takeIf { it.exists() }?.renameTo(File(root, d))
        dir.deleteRecursively()
    }

    /** Restarts the app so the restored data is the only data in memory. */
    fun restart(context: Context) {
        val intent = context.packageManager.getLaunchIntentForPackage(context.packageName)!!
            .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK or android.content.Intent.FLAG_ACTIVITY_CLEAR_TASK)
        context.startActivity(intent)
        Runtime.getRuntime().exit(0)
    }
}
