// The review screen for a finished meeting: summary, notes, transcript and assist tabs.

@file:OptIn(ExperimentalFoundationApi::class)

package app.marginalia.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.imePadding
import androidx.compose.ui.focus.focusRequester
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.Mic
import androidx.compose.material.icons.rounded.ArrowBackIosNew
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material.icons.rounded.Image
import androidx.compose.material.icons.rounded.IosShare
import androidx.compose.material.icons.rounded.PhotoCamera
import androidx.compose.material.icons.rounded.AutoAwesome
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import app.marginalia.AppVm
import app.marginalia.Jot
import app.marginalia.MeetingDetail
import app.marginalia.formatClock
import app.marginalia.Photo
import app.marginalia.Source
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

private val tabs = listOf("Summary", "Notes", "Transcript", "Assist")

@Composable
fun ReviewScreen(
    vm: AppVm,
    d: MeetingDetail,
    onCite: (Source) -> Unit,
    onAsk: (String) -> Unit,
    /** Shown beside the list instead of over it: no back arrow and no push-in gesture. */
    embedded: Boolean = false,
    /** False while the live screen covers this one, so Back goes to that first. */
    backEnabled: Boolean = true,
) {
    val p = pal
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val haptic = LocalHapticFeedback.current
    val pager = rememberPagerState { tabs.size }
    var backProgress by remember { mutableFloatStateOf(0f) }
    var viewing by remember { mutableStateOf<Photo?>(null) }
    val camera = rememberCameraLauncher { vm.addPhoto(it) }
    val gallery = rememberPhotoPicker(multiple = true) { uris -> uris.forEach { vm.addPhoto(it) } }
    val folders by vm.folders.collectAsState()
    val folderUi = rememberFolderUi()
    var pickingFolder by remember { mutableStateOf(false) }
    var jotSheet by remember { mutableStateOf<Jot?>(null) }
    var addingJot by remember { mutableStateOf(false) }
    val finishing by vm.finishing.collectAsState()

    // The predictive back gesture peeks at what is underneath before it commits.
    if (embedded) {
        androidx.activity.compose.BackHandler(enabled = viewing == null && backEnabled) { vm.closeReview() }
    } else {
        PredictiveBackHandler(enabled = viewing == null && backEnabled) { progress ->
            try {
                progress.collect { backProgress = it.progress }
                vm.closeReview()
            } catch (_: CancellationException) {
                backProgress = 0f
            }
        }
    }
    val compact = win.shortH

    val m = d.meeting
    Box(
        Modifier.fillMaxSize()
            .graphicsLayer {
                val s = 1f - 0.07f * backProgress
                scaleX = s; scaleY = s; translationX = backProgress * 20.dp.toPx()
                shape = RoundedCornerShape((28 * backProgress).dp); clip = backProgress > 0f
            }
            .background(p.surface),
    ) {
        Box(Modifier.fillMaxSize().statusBarsPadding(), contentAlignment = Alignment.TopCenter) {
        Column(Modifier.widthIn(max = ReadingWidth + 80.dp).fillMaxSize()) {
            val copy = {
                (ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText("Meeting", vm.meetingMarkdown(d)))
                vm.toast("success", "Copied to clipboard")
            }
            val share = {
                val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, vm.meetingMarkdown(d)).putExtra(Intent.EXTRA_SUBJECT, m.title)
                ctx.startActivity(Intent.createChooser(send, "Share meeting"))
            }
            val delete = {
                haptic.performHapticFeedback(HapticFeedbackType.Reject)
                vm.deleteMeeting(m)
            }
            val meta = buildString {
                append(dayLabel(dayOf(m.startedAt))).append(", ").append(timeOf(m.startedAt))
                if (m.durationSecs > 0) append("  ·  ").append(durationLabel(m.durationSecs))
                if (d.photos.isNotEmpty()) append("  ·  ${d.photos.size} ${if (d.photos.size == 1) "photo" else "photos"}")
            }
            val wrapState = finishing[m.id]
            if (compact) {
                // ---- a phone on its side: back, title and actions share one line, and the details share the next
                Row(Modifier.fillMaxWidth().height(52.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconAction(Icons.Rounded.ArrowBackIosNew, "Back", tint = p.text) { vm.closeReview() }
                    TitleField(m.title.ifBlank { "Untitled meeting" }, onDone = { vm.rename(m.id, it) }, modifier = Modifier.weight(1f).padding(horizontal = 4.dp), scale = 0.62f)
                    IconAction(Icons.Rounded.ContentCopy, "Copy notes") { copy() }
                    IconAction(Icons.Rounded.IosShare, "Share") { share() }
                    IconAction(Icons.Rounded.DeleteOutline, "Delete meeting", tint = p.danger) { delete() }
                }
                Row(Modifier.padding(horizontal = 20.dp).padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(meta, style = type.time, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
                    Spacer(Modifier.width(12.dp))
                    FolderPill(folders.firstOrNull { it.id == m.folderId }, onClick = { pickingFolder = true })
                    if (wrapState != null) {
                        Spacer(Modifier.width(12.dp))
                        ThinkingDots(color = pal.accentInk)
                        Spacer(Modifier.width(8.dp))
                        Text(wrapState.label, style = type.caption.copy(color = pal.accentInk), maxLines = 1)
                    }
                }
            } else {
                // ---- bar
                Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    if (!embedded) IconAction(Icons.Rounded.ArrowBackIosNew, "Back", tint = p.text) { vm.closeReview() }
                    Spacer(Modifier.weight(1f))
                    IconAction(Icons.Rounded.ContentCopy, "Copy notes") { copy() }
                    IconAction(Icons.Rounded.IosShare, "Share") { share() }
                    IconAction(Icons.Rounded.DeleteOutline, "Delete meeting", tint = p.danger) { delete() }
                }
                // ---- title
                TitleField(m.title.ifBlank { "Untitled meeting" }, onDone = { vm.rename(m.id, it) })
                Text(meta, style = type.time, modifier = Modifier.padding(horizontal = 20.dp).padding(top = 2.dp, bottom = 10.dp))
                Row(Modifier.padding(horizontal = 20.dp).padding(bottom = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                    FolderPill(folders.firstOrNull { it.id == m.folderId }, onClick = { pickingFolder = true })
                    if (wrapState != null) {
                        Spacer(Modifier.width(12.dp))
                        ThinkingDots(color = pal.accentInk)
                        Spacer(Modifier.width(8.dp))
                        Text(wrapState.label, style = type.caption.copy(color = pal.accentInk), maxLines = 1)
                    }
                }
            }
            // ---- tabs that track the swipe
            TabStrip(pager.currentPage + pager.currentPageOffsetFraction, tabs) { i ->
                scope.launch { pager.animateScrollToPage(i) }
            }
            HorizontalPager(pager, Modifier.weight(1f).fillMaxWidth(), beyondViewportPageCount = 1) { page ->
                when (page) {
                    0 -> SummaryPage(vm, d, onPhoto = { viewing = it }, onCamera = { camera() }, onGallery = { gallery() }, onAsk = onAsk)
                    1 -> NotesTab(d, onEdit = { jotSheet = it }, onAdd = { addingJot = true }, onDelete = vm::deleteJot)
                    2 -> TranscriptTab(vm, d, onPhoto = { viewing = it })
                    else -> AssistTab(d, onCite)
                }
            }
        }
        }
        AnimatedVisibility(viewing != null, enter = fadeIn() + scaleIn(initialScale = 0.92f), exit = fadeOut() + scaleOut(targetScale = 0.96f)) {
            val ph = viewing?.let { v -> d.photos.firstOrNull { it.id == v.id } ?: v }
            if (ph != null) PhotoViewer(ph, onClose = { viewing = null }, onDelete = { vm.deletePhoto(ph); viewing = null })
        }
    }
    if (pickingFolder) {
        FolderPicker(
            title = "Move to folder", folders = folders, current = m.folderId,
            onPick = { vm.moveMeeting(m.id, it); pickingFolder = false },
            onNew = { folderUi.afterCreate = { id -> vm.moveMeeting(m.id, id) }; pickingFolder = false; folderUi.creating = true },
            onDismiss = { pickingFolder = false },
        )
    }
    FolderSheets(vm, folderUi)
    if (addingJot || jotSheet != null) {
        JotSheet(
            editing = jotSheet,
            onDismiss = { addingJot = false; jotSheet = null },
            onSave = { text, kind ->
                val e = jotSheet
                if (e != null) vm.editJot(e, text) else vm.addJot(text, kind)
                addingJot = false; jotSheet = null
            },
        )
    }
}

@Composable
private fun TitleField(title: String, onDone: (String) -> Unit, modifier: Modifier = Modifier.padding(horizontal = 20.dp), scale: Float = 0.9f) {
    val p = pal
    var text by remember(title) { mutableStateOf(title) }
    var focused by remember { mutableStateOf(false) }
    BasicTextField(
        value = text, onValueChange = { text = it },
        textStyle = type.largeTitle.copy(fontSize = type.largeTitle.fontSize * scale, lineHeight = type.largeTitle.lineHeight * scale),
        singleLine = scale < 0.8f,
        cursorBrush = SolidColor(p.accentInk),
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onDone(text) }),
        modifier = modifier.fillMaxWidth().onFocusChanged {
            if (focused && !it.isFocused) onDone(text)
            focused = it.isFocused
        },
    )
}

/** Tabs whose indicator slides with the finger while paging. */
@Composable
fun TabStrip(position: Float, titles: List<String>, onSelect: (Int) -> Unit) {
    val p = pal
    val haptic = LocalHapticFeedback.current
    BoxWithConstraints(Modifier.fillMaxWidth().padding(horizontal = 16.dp).height(44.dp).clip(RoundedCornerShape(p.rSmall + 2.dp)).background(p.surface3).border(1.dp, p.line, RoundedCornerShape(p.rSmall + 2.dp)).padding(3.dp)) {
        val seg = maxWidth / titles.size
        Box(
            Modifier.offset(x = seg * position).width(seg).fillMaxHeight().clip(RoundedCornerShape(p.rSmall)).background(if (p.dark) p.surface4 else p.surface2),
        )
        Row(Modifier.fillMaxSize()) {
            titles.forEachIndexed { i, t ->
                val near = (1f - kotlin.math.abs(position - i)).coerceIn(0f, 1f)
                Box(Modifier.weight(1f).fillMaxHeight().pressable(pressedScale = 0.97f) {
                    haptic.performHapticFeedback(HapticFeedbackType.SegmentTick); onSelect(i)
                }, contentAlignment = Alignment.Center) {
                    Text(t, style = type.label.copy(color = androidx.compose.ui.graphics.lerp(p.textDim, p.text, near)), maxLines = 1)
                }
            }
        }
    }
}

@Composable
private fun SummaryPage(
    vm: AppVm,
    d: MeetingDetail,
    onPhoto: (Photo) -> Unit,
    onCamera: () -> Unit,
    onGallery: () -> Unit,
    onAsk: (String) -> Unit,
) {
    val p = pal
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 18.dp)) {
        if (d.meeting.summary.isBlank()) {
            Text(
                if (d.lines.isEmpty()) "This meeting has no transcript yet." else "This one was too short for a summary. The full transcript is one swipe away.",
                style = type.callout.copy(color = p.textFaint),
            )
        } else Markdown(d.meeting.summary)

        Spacer(Modifier.height(22.dp))
        TonalButton(
            "Ask about this meeting", icon = Icons.Rounded.AutoAwesome,
        ) { onAsk("What were the key points of “${d.meeting.title.ifBlank { "this meeting" }}”?") }

        Spacer(Modifier.height(10.dp))
        TonalButton("Continue recording", icon = Icons.Rounded.Mic) { vm.continueRecording(d.meeting.id) }

        Spacer(Modifier.height(26.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("PHOTOS", style = type.micro, modifier = Modifier.weight(1f))
            Box(Modifier.size(44.dp).clip(RoundedCornerShape(22.dp)).pressable(pressedScale = 0.88f, onLongClick = onGallery, onClick = onCamera), contentAlignment = Alignment.Center) {
                Icon(Icons.Rounded.PhotoCamera, "Add a photo", tint = p.accentInk, modifier = Modifier.size(22.dp))
            }
            Box(Modifier.size(44.dp).clip(RoundedCornerShape(22.dp)).pressable(pressedScale = 0.88f, onClick = onGallery), contentAlignment = Alignment.Center) {
                Icon(Icons.Rounded.Image, "Choose from gallery", tint = p.accentInk, modifier = Modifier.size(22.dp))
            }
        }
        if (d.photos.isEmpty()) {
            Text("Add photos from the meeting. The AI reads them and they become searchable with the rest.", style = type.caption.copy(color = p.textFaint))
        } else {
            Spacer(Modifier.height(8.dp))
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                d.photos.forEach { ph -> Thumb(ph) { onPhoto(ph) } }
            }
        }
        Spacer(Modifier.height(120.dp))
    }
}

@Composable
private fun Thumb(photo: Photo, onClick: () -> Unit) {
    val p = pal
    val bmp = rememberBitmap(photo.path, 400)
    var peek by remember { mutableStateOf(false) }
    Box(Modifier.size(width = 132.dp, height = 100.dp).clip(RoundedCornerShape(p.rSmall)).background(p.surface4).border(1.dp, p.line, RoundedCornerShape(p.rSmall)).peekable("Photo", { peek = it }, onClick)) {
        bmp?.let { Image(it, null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize()) }
        PhotoPeek(photo, peek)
        if (photo.describing) Box(Modifier.align(Alignment.BottomStart).padding(8.dp)) { ThinkingDots(color = androidx.compose.ui.graphics.Color.White) }
    }
}

@Composable
private fun NotesTab(d: MeetingDetail, onEdit: (Jot) -> Unit, onAdd: () -> Unit, onDelete: (Jot) -> Unit) {
    val p = pal
    if (d.jots.isNotEmpty() || (d.meeting.notesEnhanced.isBlank() && d.meeting.notesRaw.isBlank())) {
        // Notes taken as jots: grouped by what they are, each with the clean version first.
        androidx.compose.foundation.lazy.LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 8.dp, bottom = 120.dp)) {
            if (d.jots.isEmpty()) {
                item { Text("You didn't jot anything in this meeting.", style = type.callout.copy(color = p.textFaint), modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp)) }
            }
            listOf("decision" to "Decisions", "action" to "Action items", "question" to "Open questions", "other" to "Notes").forEach { (k, heading) ->
                val group = d.jots.filter { if (k == "other") it.kind !in setOf("decision", "action", "question") else it.kind == k }
                if (group.isNotEmpty()) {
                    item(key = "h$k") { SectionLabel(heading) }
                    items(group, key = { "j${it.id}" }) { j ->
                        SwipeToDelete(onDelete = { onDelete(j) }, modifier = Modifier.animateItem().padding(horizontal = 16.dp, vertical = 5.dp)) {
                            JotCard(j, onEdit = { onEdit(j) }, flat = true)
                        }
                    }
                }
            }
            item(key = "add") {
                Box(Modifier.padding(horizontal = 20.dp, vertical = 16.dp)) { TonalButton("Add a note", icon = Icons.Rounded.AutoAwesome, onClick = onAdd) }
            }
        }
        return
    }
    var showRaw by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 18.dp)) {
        val enhanced = d.meeting.notesEnhanced
        val raw = d.meeting.notesRaw
        when {
            enhanced.isNotBlank() -> Markdown(enhanced)
            raw.isNotBlank() -> Text(raw, style = type.body)
            else -> Text("You didn't type any notes in this meeting.", style = type.callout.copy(color = p.textFaint))
        }
        if (enhanced.isNotBlank() && raw.isNotBlank()) {
            Spacer(Modifier.height(24.dp))
            TonalButton(if (showRaw) "Hide what I typed" else "Show what I typed") { showRaw = !showRaw }
            AnimatedVisibility(showRaw) {
                Text(raw, style = type.callout.copy(color = p.text), modifier = Modifier.padding(top = 14.dp))
            }
        }
        Spacer(Modifier.height(120.dp))
    }
}

@Composable
private fun TranscriptTab(vm: AppVm, d: MeetingDetail, onPhoto: (Photo) -> Unit) {
    val p = pal
    val hasAudio = d.audio.any { it.exists }
    val playing by vm.player.playing.collectAsState()
    val at by vm.player.at.collectAsState()
    val items = remember(d.lines, d.photos) {
        (d.lines.map { Triple(it.t0, "l${it.i}", it as Any) } + d.photos.map { Triple(it.t ?: Double.MAX_VALUE, "p${it.id}", it as Any) }).sortedBy { it.first }
    }
    if (items.isEmpty()) {
        Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
            Text("Nothing was transcribed.", style = type.callout.copy(color = p.textFaint))
        }
        return
    }
    Box(Modifier.fillMaxSize()) {
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 10.dp, bottom = 120.dp)) {
        item(key = "speakers") { SpeakerHeader(vm, d) }
        if (hasAudio) item(key = "hint") {
            Text("Tap a line to hear it.", style = type.caption.copy(color = p.textFaint), modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp))
        }
        items(items, key = { it.second }) { (_, _, obj) ->
            when (obj) {
                is app.marginalia.Line -> TranscriptRow(
                    obj,
                    active = at?.let { it >= obj.t0 && it < obj.t1 + 1.5 } ?: false,
                    onPlay = if (hasAudio) ({ vm.player.play(d.audio, obj.t0) }) else null,
                    speaker = obj.speaker?.let { speakerName(d, it) },
                    speakerColor = obj.speaker?.let { speakerColor(it) },
                )
                is Photo -> PhotoCard(obj, onOpen = { onPhoto(obj) })
            }
        }
    }
    AnimatedVisibility(at != null, modifier = Modifier.align(Alignment.BottomCenter), enter = fadeIn() + scaleIn(initialScale = 0.92f), exit = fadeOut()) {
        Row(
            Modifier.padding(bottom = 96.dp).clip(RoundedCornerShape(28.dp)).background(p.surface2).border(1.dp, p.lineStrong, RoundedCornerShape(28.dp))
                .padding(start = 6.dp, end = 18.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconAction(if (playing) Icons.Rounded.Pause else Icons.Rounded.PlayArrow, if (playing) "Pause" else "Play", tint = p.accentInk) { vm.player.toggle() }
            Text(formatClock(at ?: 0.0), style = type.time.copy(color = p.text))
            Spacer(Modifier.width(10.dp))
            IconAction(Icons.Rounded.Close, "Stop playback", tint = p.textDim) { vm.player.stop() }
        }
    }
    }
}

@Composable
private fun AssistTab(d: MeetingDetail, onCite: (Source) -> Unit) {
    val p = pal
    if (d.cards.isEmpty()) {
        Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
            Text("No questions were answered during this meeting.", style = type.callout.copy(color = p.textFaint))
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 120.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        items(d.cards, key = { it.id }) { c -> AssistCard(c, onCite = onCite) }
    }
}

/** Add or change a note on a finished meeting. */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun JotSheet(editing: Jot?, onDismiss: () -> Unit, onSave: (String, String) -> Unit) {
    val p = pal
    var text by remember { mutableStateOf(editing?.raw.orEmpty()) }
    var kind by remember { mutableStateOf(editing?.kind ?: "note") }
    val focus = remember { androidx.compose.ui.focus.FocusRequester() }
    androidx.compose.runtime.LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    androidx.compose.material3.ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = androidx.compose.material3.rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = p.surface2, contentColor = p.text, scrimColor = p.scrim,
        shape = RoundedCornerShape(topStart = p.rLarge, topEnd = p.rLarge),
        dragHandle = { SheetHandle() },
    ) {
        Column(Modifier.androidx_imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).navigationBarsPadding().padding(bottom = 20.dp)) {
            Text(if (editing == null) "Add a note" else "Edit note", style = type.title)
            Spacer(Modifier.height(14.dp))
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                jotKinds.forEach { (k, label) -> Chip(label, selected = kind == k) { kind = k } }
            }
            Spacer(Modifier.height(12.dp))
            val shape = RoundedCornerShape(p.rMid)
            Box(Modifier.fillMaxWidth().height(120.dp).clip(shape).background(p.surface3).border(1.dp, p.line, shape).padding(16.dp)) {
                if (text.isEmpty()) Text("Short is fine. The AI will tidy it.", style = type.body.copy(color = p.textFaint))
                BasicTextField(value = text, onValueChange = { text = it }, textStyle = type.body, cursorBrush = SolidColor(p.accentInk), modifier = Modifier.fillMaxSize().focusRequester(focus))
            }
            Spacer(Modifier.height(16.dp))
            PrimaryButton("Save", Modifier.fillMaxWidth(), enabled = text.isNotBlank()) { onSave(text, kind) }
        }
    }
}

private fun Modifier.androidx_imePadding(): Modifier = this.then(Modifier.imePadding())
