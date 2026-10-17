package app.marginalia.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.FileProvider
import java.io.File

/** Opens the system camera and hands back the picture. No camera permission is needed: the camera app takes it. */
@Composable
fun rememberCameraLauncher(onPhoto: (Uri) -> Unit): () -> Unit {
    val ctx = LocalContext.current
    val cb = rememberUpdatedState(onPhoto)
    // The camera app can push ours out of memory; the picture it is taking must survive that.
    var pending by androidx.compose.runtime.saveable.rememberSaveable { androidx.compose.runtime.mutableStateOf("") }
    var pendingFile by androidx.compose.runtime.saveable.rememberSaveable { androidx.compose.runtime.mutableStateOf("") }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { ok ->
        val uri = pending.takeIf { it.isNotEmpty() }?.let { Uri.parse(it) }
        val file = pendingFile.takeIf { it.isNotEmpty() }?.let { File(it) }
        // Some camera apps report "cancelled" after they have saved the picture, or hand back "done" a moment
        // before the file is complete. A real file on disk counts either way, and the reader waits for it to settle.
        val saved = file != null && file.isFile && file.length() > 10_000
        if (uri != null && (ok || saved)) cb.value(uri)
        pending = ""
        pendingFile = ""
    }
    return {
        val dir = File(ctx.cacheDir, "camera").also { it.mkdirs() }
        // Old shots that were never used do not pile up.
        dir.listFiles()?.filter { System.currentTimeMillis() - it.lastModified() > 6 * 3600_000L }?.forEach { it.delete() }
        val f = File.createTempFile("shot_", ".jpg", dir)
        val uri = FileProvider.getUriForFile(ctx, ctx.packageName + ".files", f)
        pending = uri.toString()
        pendingFile = f.absolutePath
        try {
            launcher.launch(uri)
        } catch (e: android.content.ActivityNotFoundException) {
            pending = ""; pendingFile = ""
        }
    }
}

/** The system photo picker (no storage permission needed). */
@Composable
fun rememberPhotoPicker(multiple: Boolean = true, onPicked: (List<Uri>) -> Unit): () -> Unit {
    val cb = rememberUpdatedState(onPicked)
    val multi = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(10)) { cb.value(it) }
    val single = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { cb.value(listOfNotNull(it)) }
    return {
        val req = PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)
        if (multiple) multi.launch(req) else single.launch(req)
    }
}
