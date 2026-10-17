@file:OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)

package app.marginalia.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Article
import androidx.compose.material.icons.rounded.AudioFile
import androidx.compose.material.icons.rounded.Code
import androidx.compose.material.icons.rounded.Event
import androidx.compose.material.icons.rounded.Image
import androidx.compose.material.icons.rounded.Language
import androidx.compose.material.icons.rounded.PhotoCamera
import androidx.compose.material.icons.rounded.PictureAsPdf
import androidx.compose.material.icons.rounded.Slideshow
import androidx.compose.material.icons.rounded.TableChart
import androidx.compose.material.icons.rounded.UploadFile
import androidx.compose.material.icons.rounded.VideoLibrary
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.marginalia.AppVm
import app.marginalia.DocItem
import app.marginalia.IngestJob
import app.marginalia.Source
import kotlinx.coroutines.delay

@Composable
fun KnowledgeScreen(vm: AppVm, onCite: (Source) -> Unit) {
    val p = pal
    val allDocs by vm.docs.collectAsState()
    val folders by vm.folders.collectAsState()
    val filter by vm.knowledgeFilter.collectAsState()
    val folderUi = rememberFolderUi()
    var moving by remember { mutableStateOf<DocItem?>(null) }
    // Meetings have their own tab; this list is what you added yourself.
    val docs = remember(allDocs, filter) {
        allDocs.filter { it.kind != "meeting" && (filter == null || (filter == -1L && it.folderId == null) || it.folderId == filter) }
    }
    val jobs by vm.ingest.collectAsState()
    val stats by vm.stats.collectAsState()
    val passages by vm.passages.collectAsState()
    val list = rememberLazyListState()
    var query by rememberSaveable { mutableStateOf("") }
    var adding by remember { mutableStateOf(false) }

    // Search as you type, after a short pause.
    LaunchedEffect(query) {
        if (query.isBlank()) vm.searchPassages("") else { delay(350); vm.searchPassages(query) }
    }

    val sideBySide = win.twoPane
    val searchField: @Composable (Modifier) -> Unit = { m ->
        SearchField(query, { query = it }, if (filter != null && filter!! > 0) "Search this folder" else "Search everything you've added", m)
    }
    val results: LazyListScope.() -> Unit = {
        if (query.isNotBlank()) {
            val res = passages
            if (res == null) {
                item(key = "searching") { Box(Modifier.fillMaxWidth().padding(32.dp), contentAlignment = Alignment.Center) { ThinkingDots() } }
            } else if (res.isEmpty()) {
                item(key = "none") { EmptyState("Nothing found", "No passage matches “$query”.") }
            } else {
                item(key = "plabel") { SectionLabel("Best passages") }
                items(res, key = { "p${it.chunkKey()}" }) { s -> PassageCard(s, Modifier.animateItem()) { onCite(s) } }
            }
        }
    }
    val searching = query.isNotBlank() && !sideBySide
    val clearance = bottomClearance()

    /** The documents, folders and add button. Beside the search pane it is only the library. */
    val library: @Composable () -> Unit = {
        Box(Modifier.fillMaxSize()) {
            LazyColumn(state = list, contentPadding = PaddingValues(bottom = clearance), modifier = Modifier.fillMaxSize()) {
                item(key = "title") {
                    LargeTitle(
                        "Knowledge",
                        if (docs.isEmpty()) "Documents Marginalia can quote" else "${docs.size} ${if (docs.size == 1) "item" else "items"} · ${docs.sumOf { it.chunks }} passages",
                    )
                }
                item(key = "folders") {
                    FolderBar(
                        folders, filter, onSelect = { vm.knowledgeFilter.value = it; vm.refreshDocs() },
                        onNew = { folderUi.creating = true }, onEdit = { folderUi.editing = it },
                        modifier = Modifier.padding(bottom = 10.dp),
                    )
                }
                if (!sideBySide) item(key = "search") { searchField(Modifier.padding(horizontal = 20.dp).padding(bottom = 12.dp)) }
                items(jobs, key = { "job${it.job}" }) { j -> IngestRow(j, Modifier.animateItem()) }

                if (searching) {
                    results()
                } else if (docs.isEmpty() && jobs.isEmpty()) {
                    item(key = "empty") {
                        EmptyState(
                            "Add what you want it to know",
                            "PDFs, notes, text, Markdown, web pages, photos, voice memos and videos. Marginalia reads them on this phone and cites them when it answers.",
                        )
                    }
                } else {
                    items(docs, key = { "d${it.id}" }) { d ->
                        SwipeToDelete(
                            onDelete = { vm.deleteDoc(d) },
                            modifier = Modifier.animateItem().padding(horizontal = 16.dp, vertical = 5.dp),
                        ) { DocRow(d, folder = folders.firstOrNull { it.id == d.folderId }.takeIf { filter == null }, onLongClick = { moving = d }, onOpen = { vm.openDoc(d) }) }
                    }
                }
            }
            TopBarOverlay("Knowledge", list, Modifier.align(Alignment.TopCenter))

            // Add button, in the thumb zone.
            val fabBottom = if (win.rail) 20.dp else 108.dp
            Box(
                Modifier.align(Alignment.BottomEnd).navigationBarsPadding().padding(end = 20.dp, bottom = fabBottom)
                    .shadow(14.dp, CircleShape, ambientColor = p.accent, spotColor = p.accent)
                    .clip(CircleShape).background(p.accentBrush)
                    .pressable(pressedScale = 0.92f, haptic = HapticFeedbackType.ContextClick) { adding = true }
                    .size(56.dp),
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Rounded.Add, "Add to knowledge", tint = p.onAccent, modifier = Modifier.size(28.dp)) }
            // With nothing added yet, point at the button (its top edge is 164dp above the bottom).
            if (docs.isEmpty() && jobs.isEmpty() && query.isBlank() && !win.rail) {
                PointerArrow(Modifier.align(Alignment.BottomEnd).navigationBarsPadding().padding(end = 32.dp, bottom = 170.dp))
            }
        }
    }

    if (sideBySide) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val libW = (maxWidth * 0.46f).coerceIn(300.dp, 460.dp)
            Row(Modifier.fillMaxSize()) {
                Box(Modifier.width(libW).fillMaxHeight()) { library() }
                Box(Modifier.width(1.dp).fillMaxHeight().background(p.line))
                // Search sits beside the library, so results never push the documents away.
                Box(Modifier.weight(1f).fillMaxHeight(), contentAlignment = Alignment.TopCenter) {
                    LazyColumn(
                        Modifier.widthIn(max = ReadingWidth).fillMaxSize(),
                        contentPadding = PaddingValues(top = 18.dp, bottom = clearance),
                    ) {
                        item(key = "stitle") {
                            Text("Search", style = type.largeTitle, modifier = Modifier.statusBarsPadding().padding(start = 20.dp, end = 20.dp, top = 38.dp, bottom = 12.dp))
                        }
                        item(key = "sfield") { searchField(Modifier.padding(horizontal = 20.dp).padding(bottom = 12.dp)) }
                        if (query.isBlank()) {
                            item(key = "shint") {
                                EmptyState("Find the exact passage", "Search finds meaning, not just words. Results quote the passage they come from.")
                            }
                        } else results()
                    }
                }
            }
        }
    } else {
        CappedPane { library() }
    }
    if (adding) AddSheet(vm, folders.firstOrNull { it.id == filter }, onDismiss = { adding = false })
    moving?.let { d ->
        FolderPicker(
            title = "File “${d.title.take(30)}”", folders = folders, current = d.folderId,
            onPick = { vm.moveDoc(d.id, it); moving = null },
            onNew = { folderUi.afterCreate = { id -> vm.moveDoc(d.id, id) }; moving = null; folderUi.creating = true },
            onDismiss = { moving = null },
        )
    }
    FolderSheets(vm, folderUi)
}

private fun Source.chunkKey() = "$docId-$n-${snippet.hashCode()}"

@Composable
private fun IngestRow(j: IngestJob, modifier: Modifier = Modifier) {
    val p = pal
    Row(
        modifier.padding(horizontal = 16.dp, vertical = 4.dp).fillMaxWidth().clip(RoundedCornerShape(p.rMid)).background(p.tonal).padding(14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (j.active) ThinkingDots(color = p.accentInk) else Icon(
            if (j.stage == "failed") Icons.Rounded.UploadFile else Icons.Rounded.Article, null,
            tint = if (j.stage == "failed") p.danger else p.success, modifier = Modifier.size(20.dp),
        )
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(j.label, style = type.label, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val status = when (j.stage) {
                "decoding" -> if (j.total > 0) "Preparing audio · ${j.done}%" else "Preparing audio"
                "queued" -> "Waiting"
                "extracting" -> if (isImageName(j.label)) "Reading the image" else "Reading"
                // For a picture, this stage is Gemma reading it.
                "chunking" -> if (isImageName(j.label)) "Reading the image" else "Splitting into passages"
                "embedding" -> if (j.total > 0) "Indexing · ${j.done}/${j.total}" else "Indexing"
                "done" -> "Added"
                "skipped" -> j.message.ifBlank { "Already in your knowledge" }
                "failed" -> j.message.ifBlank { "Could not read this file" }
                else -> j.stage
            }
            Text(status, style = type.caption.copy(color = if (j.stage == "failed") p.danger else p.textDim), maxLines = 2)
        }
    }
}

private fun isImageName(label: String): Boolean =
    label.substringAfterLast('.', "").lowercase() in setOf("jpg", "jpeg", "png", "webp", "heic", "heif", "gif", "bmp")

private fun iconFor(kind: String): ImageVector = when (kind) {
    "pdf" -> Icons.Rounded.PictureAsPdf
    "slides" -> Icons.Rounded.Slideshow
    "sheet" -> Icons.Rounded.TableChart
    "web", "html" -> Icons.Rounded.Language
    "meeting" -> Icons.Rounded.Event
    "code" -> Icons.Rounded.Code
    "image" -> Icons.Rounded.Image
    "audio" -> Icons.Rounded.AudioFile
    else -> Icons.Rounded.Article
}

@Composable
private fun DocRow(d: DocItem, folder: app.marginalia.Folder?, onLongClick: () -> Unit, onOpen: () -> Unit) {
    val p = pal
    val shape = RoundedCornerShape(p.rMid)
    Row(
        Modifier.fillMaxWidth().clip(shape).background(p.surface3).border(1.dp, p.line, shape).pressable(pressedScale = 0.99f, onLongClick = onLongClick, onClick = onOpen).padding(14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(42.dp).clip(RoundedCornerShape(p.rSmall)).background(p.tonal), contentAlignment = Alignment.Center) {
            Icon(iconFor(d.kind), null, tint = p.accentInk, modifier = Modifier.size(22.dp))
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(d.title, style = type.headline, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                "${d.kind.replaceFirstChar { it.uppercase() }}  ·  ${d.chunks} passages  ·  ${wordsLabel(d.words)}",
                style = type.caption, maxLines = 1,
            )
            if (folder != null) {
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 4.dp)) {
                    FolderDot(folder.color, 8.dp); Spacer(Modifier.width(6.dp)); Text(folder.name, style = type.caption.copy(color = p.textFaint))
                }
            }
        }
    }
}

private fun wordsLabel(w: Int) = if (w >= 1000) "%.1fk words".format(w / 1000f) else "$w words"

@Composable
fun PassageCard(s: Source, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val p = pal
    val shape = RoundedCornerShape(p.rMid)
    Column(
        modifier.padding(horizontal = 16.dp, vertical = 5.dp).fillMaxWidth().clip(shape).background(p.surface3).border(1.dp, p.line, shape)
            .pressable(pressedScale = 0.98f, onClick = onClick).padding(16.dp),
    ) {
        Text(s.title, style = type.label, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (s.locator.isNotBlank()) Text(s.locator, style = type.time)
        Spacer(Modifier.height(8.dp))
        Text(s.snippet, style = type.callout.copy(color = p.text), maxLines = 4, overflow = TextOverflow.Ellipsis)
    }
}

/** Where things come from: files, photos, the camera, audio and video, or a web page. */
@Composable
private fun AddSheet(vm: AppVm, folder: app.marginalia.Folder?, onDismiss: () -> Unit) {
    val p = pal
    var url by remember { mutableStateOf("") }
    val files = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { vm.ingestUris(it); onDismiss() }
    val media = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { vm.ingestUris(it); onDismiss() }
    val photos = rememberPhotoPicker { vm.ingestUris(it); onDismiss() }
    val camera = rememberCameraLauncher { vm.ingestUris(listOf(it)); onDismiss() }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = p.surface2, contentColor = p.text, scrimColor = p.scrim,
        shape = RoundedCornerShape(topStart = p.rLarge, topEnd = p.rLarge),
        dragHandle = { SheetHandle() },
    ) {
        Column(Modifier.imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).navigationBarsPadding().padding(bottom = 24.dp)) {
            Text("Add to knowledge", style = type.title)
            if (folder != null) {
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 8.dp)) {
                    Text("Filing into", style = type.caption); Spacer(Modifier.width(8.dp)); FolderPill(folder, onClick = {}, modifier = Modifier)
                }
            }
            Spacer(Modifier.height(16.dp))
            AddOption(Icons.Rounded.UploadFile, "Documents", "PDF, notes, text, Markdown, Word, slides, sheets, code") {
                files.launch(arrayOf("*/*"))
            }
            AddOption(Icons.Rounded.Image, "Photos", "Pictures from your gallery. The AI reads them") { photos() }
            AddOption(Icons.Rounded.PhotoCamera, "Take a photo", "Take a picture of a page right now") { camera() }
            AddOption(Icons.Rounded.VideoLibrary, "Audio and video", "Voice memos, recordings, videos. Transcribed here") {
                media.launch(arrayOf("audio/*", "video/*"))
            }
            Spacer(Modifier.height(10.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.weight(1f)) {
                    SearchField(url, { url = it }, "Paste a web page address", onSearch = { vm.ingestUrl(url); onDismiss() })
                }
                Spacer(Modifier.width(10.dp))
                TonalButton("Add", Modifier.height(48.dp)) { vm.ingestUrl(url); onDismiss() }
            }
        }
    }
}

@Composable
private fun AddOption(icon: ImageVector, title: String, subtitle: String, onClick: () -> Unit) {
    val p = pal
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(p.rMid)).pressable(pressedScale = 0.98f, onClick = onClick).padding(vertical = 10.dp, horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(46.dp).clip(RoundedCornerShape(p.rSmall)).background(p.tonal), contentAlignment = Alignment.Center) {
            Icon(icon, null, tint = p.accentInk, modifier = Modifier.size(24.dp))
        }
        Spacer(Modifier.width(16.dp))
        Column {
            Text(title, style = type.headline)
            Text(subtitle, style = type.caption)
        }
    }
}
