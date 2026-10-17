// The screen that shows one imported document or image.

package app.marginalia.ui

import androidx.activity.compose.BackHandler
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.pdf.PdfRenderer
import android.os.ParcelFileDescriptor
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.material.icons.automirrored.rounded.OpenInNew
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.produceState
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ArrowBackIosNew
import androidx.compose.material.icons.rounded.AutoAwesome
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material3.Text
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import app.marginalia.AppVm
import app.marginalia.DocDetail

/**
 * A document from the knowledge base: what it is, and the text Marginalia read from it, in order.
 * The original file is not kept, only this text, which is what search and Ask draw from.
 */
@Composable
fun DocumentScreen(vm: AppVm, d: DocDetail, onAsk: (String) -> Unit) {
    val p = pal
    val ctx = LocalContext.current
    val folders by vm.folders.collectAsState()
    var pickingFolder by remember { mutableStateOf(false) }
    val doc = d.doc
    val file = d.file?.takeIf { File(it.path).isFile }
    var showText by remember(doc.id) { mutableStateOf(file == null) }
    BackHandler { vm.closeDoc() }

    Box(Modifier.fillMaxSize().background(p.surface), contentAlignment = Alignment.TopCenter) {
        Column(Modifier.widthIn(max = ReadingWidth + 80.dp).fillMaxSize().statusBarsPadding()) {
            Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconAction(Icons.Rounded.ArrowBackIosNew, "Back", tint = p.text) { vm.closeDoc() }
                Spacer(Modifier.weight(1f))
                if (file != null) IconAction(Icons.AutoMirrored.Rounded.OpenInNew, "Open in another app") { openElsewhere(ctx, file) }
                IconAction(Icons.Rounded.ContentCopy, "Copy the text") {
                    val text = d.chunks.joinToString("\n\n") { it.text }
                    (ctx.getSystemService(android.content.Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager)
                        .setPrimaryClip(android.content.ClipData.newPlainText(doc.title, text))
                    vm.toast("success", "Copied the text.")
                }
                IconAction(Icons.Rounded.Delete, "Delete this document", tint = p.danger) { vm.deleteDoc(doc) }
            }
            LazyColumn(Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 120.dp)) {
                item(key = "head") {
                    Column(Modifier.padding(horizontal = 20.dp)) {
                        Text(doc.title, style = type.largeTitle)
                        Spacer(Modifier.height(6.dp))
                        Text(
                            buildString {
                                append(doc.kind.replaceFirstChar { it.uppercase() })
                                append("  ·  ").append(dayLabel(dayOf(doc.addedAt))).append(", ").append(timeOf(doc.addedAt))
                                append("  ·  ").append(doc.chunks).append(" passages")
                                append("  ·  ").append(if (doc.words >= 1000) "%.1fk words".format(doc.words / 1000f) else "${doc.words} words")
                            },
                            style = type.time,
                        )
                        Spacer(Modifier.height(12.dp))
                        FolderPill(folders.firstOrNull { it.id == doc.folderId }, onClick = { pickingFolder = true })
                        Spacer(Modifier.height(16.dp))
                        TonalButton("Ask about this document", icon = Icons.Rounded.AutoAwesome) {
                            onAsk("What are the key points of “${doc.title}”?")
                        }
                        Spacer(Modifier.height(22.dp))
                        if (file != null) {
                            Segmented(listOf("Original", "Text"), if (showText) "Text" else "Original", { showText = it == "Text" })
                            Spacer(Modifier.height(14.dp))
                        }
                        if (showText) {
                            if (d.chunks.isEmpty()) {
                                Text("No text was read from this file.", style = type.callout.copy(color = p.textFaint))
                            } else if (file == null) {
                                Text("TEXT", style = type.micro)
                                Spacer(Modifier.height(6.dp))
                            }
                        }
                    }
                }
                if (!showText && file != null) {
                    originalItems(file)
                }
                if (showText) itemsIndexed(d.chunks, key = { i, _ -> "c$i" }) { i, c ->
                    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp)) {
                        val label = listOfNotNull(c.section.takeIf { it.isNotBlank() }, c.page.takeIf { it > 0 }?.let { "page $it" }).joinToString("  ·  ")
                        if (label.isNotEmpty() && (i == 0 || d.chunks[i - 1].section != c.section || d.chunks[i - 1].page != c.page)) {
                            Text(label, style = type.caption.copy(color = p.accentInk))
                            Spacer(Modifier.height(4.dp))
                        }
                        Text(c.text, style = type.reading)
                    }
                }
            }
        }
        if (pickingFolder) {
            FolderPicker(
                title = "Move to folder", folders = folders, current = doc.folderId,
                onPick = { vm.moveDoc(doc.id, it); pickingFolder = false },
                onNew = { pickingFolder = false },
                onDismiss = { pickingFolder = false },
            )
        }
    }
}


/** Hands the kept original to whatever app the person prefers. */
private fun openElsewhere(ctx: android.content.Context, f: app.marginalia.DocFile) {
    runCatching {
        val uri = androidx.core.content.FileProvider.getUriForFile(ctx, ctx.packageName + ".files", File(f.path))
        ctx.startActivity(
            android.content.Intent(android.content.Intent.ACTION_VIEW)
                .setDataAndType(uri, f.mime)
                .addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or android.content.Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }
}

/** The kept original: a picture is shown whole, a PDF page by page, anything else opens in another app. */
private fun androidx.compose.foundation.lazy.LazyListScope.originalItems(f: app.marginalia.DocFile) {
    when {
        f.mime.startsWith("image/") -> item(key = "image") { ImageOriginal(f.path) }
        f.mime == "application/pdf" -> pdfItems(f.path)
        else -> item(key = "other") {
            Column(Modifier.padding(horizontal = 20.dp)) {
                Text("This kind of file opens in another app.", style = type.callout.copy(color = pal.textFaint))
                Spacer(Modifier.height(10.dp))
                val ctx = LocalContext.current
                TonalButton("Open") { openElsewhere(ctx, f) }
            }
        }
    }
}

@Composable
private fun ImageOriginal(path: String) {
    val bmp by produceState<Bitmap?>(null, path) {
        value = withContext(Dispatchers.IO) {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeFile(path, bounds)
            var sample = 1
            while (bounds.outWidth / sample > 2200 || bounds.outHeight / sample > 3200) sample *= 2
            BitmapFactory.decodeFile(path, BitmapFactory.Options().apply { inSampleSize = sample })
        }
    }
    val b = bmp
    if (b == null) {
        Box(Modifier.fillMaxWidth().height(240.dp))
    } else {
        Image(
            b.asImageBitmap(), "The picture",
            Modifier.fillMaxWidth().padding(horizontal = 20.dp).clip(RoundedCornerShape(pal.rMid)),
            contentScale = ContentScale.FillWidth,
        )
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.pdfItems(path: String) {
    item(key = "pdf") { PdfPages(path) }
}

/**
 * Pages are drawn one at a time, only while they are on screen, at the width of the window. The system
 * renderer can only have one page open, so a lock keeps the page renders in a queue.
 */
@Composable
private fun PdfPages(path: String) {
    val holder = remember(path) {
        runCatching {
            val fd = ParcelFileDescriptor.open(File(path), ParcelFileDescriptor.MODE_READ_ONLY)
            fd to PdfRenderer(fd)
        }.getOrNull()
    }
    DisposableEffect(path) {
        onDispose { runCatching { holder?.second?.close() }; runCatching { holder?.first?.close() } }
    }
    if (holder == null) {
        Text("This PDF could not be opened here. Try opening it in another app.", style = type.callout.copy(color = pal.textFaint), modifier = Modifier.padding(horizontal = 20.dp))
        return
    }
    val renderer = holder.second
    val lock = remember(path) { Mutex() }
    val widthPx = with(androidx.compose.ui.platform.LocalDensity.current) { (androidx.compose.ui.platform.LocalConfiguration.current.screenWidthDp.dp.coerceAtMost(760.dp)).roundToPx() }
    // A plain column inside the list: pages are few enough and each draws only when it first appears.
    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        for (i in 0 until renderer.pageCount) {
            val page by produceState<Bitmap?>(null, path, i, widthPx) {
                value = withContext(Dispatchers.IO) {
                    lock.withLock {
                        runCatching {
                            renderer.openPage(i).use { pg ->
                                val scale = widthPx.toFloat() / pg.width
                                val bm = Bitmap.createBitmap(widthPx, (pg.height * scale).toInt(), Bitmap.Config.ARGB_8888)
                                bm.eraseColor(android.graphics.Color.WHITE)
                                pg.render(bm, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                                bm
                            }
                        }.getOrNull()
                    }
                }
            }
            val b = page
            Box(Modifier.fillMaxWidth().clip(RoundedCornerShape(pal.rSmall)).background(androidx.compose.ui.graphics.Color.White)) {
                if (b == null) Box(Modifier.fillMaxWidth().aspectRatio(0.71f))
                else Image(b.asImageBitmap(), "Page ${i + 1}", Modifier.fillMaxWidth(), contentScale = ContentScale.FillWidth)
            }
        }
    }
}
