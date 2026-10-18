// The live meeting screen: transcript, jots, question cards and the stop control.

@file:OptIn(ExperimentalFoundationApi::class)

package app.marginalia.ui

import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ArrowDownward
import androidx.compose.material.icons.rounded.ArrowUpward
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.PhotoCamera
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material.icons.rounded.Visibility
import androidx.compose.material.icons.rounded.VisibilityOff
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.onLongClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import app.marginalia.AppVm
import app.marginalia.Card
import app.marginalia.Jot
import app.marginalia.Line
import app.marginalia.LiveState
import app.marginalia.Photo
import app.marginalia.RecorderBus
import app.marginalia.correctionLabel
import app.marginalia.Source
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Everything that happens in a meeting, in the order it happened. */
private sealed interface Moment {
    val key: String
    val at: Double

    data class Said(val line: Line) : Moment { override val key = "l${line.id}"; override val at = line.t0 }
    data class Noted(val jot: Jot) : Moment { override val key = "j${jot.id}"; override val at = jot.t ?: Double.MAX_VALUE }
    data class Shot(val photo: Photo) : Moment { override val key = "p${photo.id}"; override val at = photo.t ?: Double.MAX_VALUE }
    data class Asked(val card: Card) : Moment { override val key = "c${card.id}"; override val at = card.t }
}

private fun moments(s: LiveState, notesOnly: Boolean): List<Moment> {
    val out = ArrayList<Moment>()
    if (!notesOnly) s.lines.forEach { out += Moment.Said(it) }
    s.jots.forEach { out += Moment.Noted(it) }
    s.photos.forEach { out += Moment.Shot(it) }
    s.cards.forEach { if (it.state != app.marginalia.CardState.NoMatch) out += Moment.Asked(it) }
    return out.sortedBy { it.at }
}

@Composable
fun LiveScreen(vm: AppVm, onMinimize: () -> Unit, onCite: (Source) -> Unit) {
    val p = pal
    val live by vm.live.collectAsState()
    val s = live ?: return
    val level by RecorderBus.level.collectAsState()
    val scope = rememberCoroutineScope()
    val haptic = LocalHapticFeedback.current
    val view = LocalView.current
    val list = rememberLazyListState()
    var viewing by remember { mutableStateOf<Photo?>(null) }
    var showTranscript by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf(true) }
    val notesOnly = !showTranscript
    var composing by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf(false) }
    var draft by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf("") }
    var kind by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf("note") }
    var editing by remember { mutableStateOf<Jot?>(null) }
    val folders by vm.folders.collectAsState()
    val prefsNow by vm.prefs.collectAsState()
    val liveFolder by vm.liveFolder.collectAsState()
    val folderUi = rememberFolderUi()
    var pickingFolder by remember { mutableStateOf(false) }

    DisposableEffect(Unit) {
        val w = (view.context as? ComponentActivity)?.window
        w?.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose { w?.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) }
    }
    BackHandler(enabled = composing) { composing = false; editing = null; draft = "" }

    val dragY = remember { Animatable(0f) }
    var heightPx by remember { mutableStateOf(1f) }
    val camera = rememberCameraLauncher { vm.addPhoto(it) }
    val gallery = rememberPhotoPicker(multiple = true) { uris -> uris.forEach { vm.addPhoto(it) } }

    val send = {
        val e = editing
        if (draft.isNotBlank()) {
            if (e != null) vm.editJot(e, draft) else if (kind == "ask") vm.askLive(draft) else vm.addJot(draft, kind)
            draft = ""; editing = null
            haptic.performHapticFeedback(HapticFeedbackType.ContextClick)
        }
    }

    Box(
        Modifier.fillMaxSize()
            .graphicsLayer {
                translationY = dragY.value
                val f = (dragY.value / heightPx).coerceIn(0f, 1f)
                scaleX = 1f - 0.06f * f; scaleY = 1f - 0.06f * f
                shape = RoundedCornerShape((28 * f).dp); clip = f > 0f
            }
            .background(p.surface)
            .onSizeChanged { heightPx = it.height.toFloat() }
            .imePadding(),
    ) {
        Aurora(level = { level }, modifier = Modifier.fillMaxSize())

        val split = win.split
        val short = win.shortH
        // A phone on its side has room for little above the keyboard, so while you jot the header gives way.
        val imeUp = WindowInsets.ime.getBottom(androidx.compose.ui.platform.LocalDensity.current) > 0
        val hideChrome = short && composing && imeUp
        // Beside the transcript when the window is wider than tall. Otherwise one column, no wider than is easy to read.
        Box(Modifier.fillMaxSize().statusBarsPadding(), contentAlignment = Alignment.TopCenter) {
        Column(Modifier.then(if (split) Modifier else Modifier.widthIn(max = ReadingWidth + 40.dp)).fillMaxSize()) {
            // ---- header: grab it to tuck the meeting away
            androidx.compose.animation.AnimatedVisibility(
                visible = !hideChrome,
                enter = fadeIn(tween(200)) + androidx.compose.animation.expandVertically(tween(220, easing = Motion.Emphasized)),
                exit = fadeOut(tween(150)) + androidx.compose.animation.shrinkVertically(tween(180, easing = Motion.Emphasized)),
            ) {
            Row(
                Modifier.fillMaxWidth().height(if (short) 48.dp else 56.dp).padding(horizontal = 4.dp)
                    .draggable(
                        orientation = Orientation.Vertical,
                        state = rememberDraggableState { dy -> scope.launch { dragY.snapTo((dragY.value + dy).coerceAtLeast(0f)) } },
                        onDragStopped = { v ->
                            if (dragY.value > heightPx * 0.18f || v > 1400f) {
                                haptic.performHapticFeedback(HapticFeedbackType.GestureEnd)
                                scope.launch { dragY.animateTo(heightPx, Motion.fade(220)); onMinimize() }
                            } else scope.launch { dragY.animateTo(0f, Motion.bouncy()) }
                        },
                    ),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconAction(Icons.Rounded.KeyboardArrowDown, "Minimise", onClick = { scope.launch { dragY.animateTo(heightPx, Motion.fade(220)); onMinimize() } })
                if (s.recording) PulseDot(p.danger, 9.dp) else ThinkingDots()
                Spacer(Modifier.width(4.dp))
                RollingTimer(formatTimer(s.secs), type.headline.copy(fontFeatureSettings = "tnum"))
                Box(Modifier.weight(1f).height(26.dp).padding(horizontal = 12.dp)) {
                    LiveWaveform(level = { level }, active = s.recording, bars = 30, modifier = Modifier.fillMaxSize())
                }
                if (s.wrap == null) {
                    // Show or hide the spoken transcript. Hidden, only what you and Gemma made stays: jots, photos, answers.
                    Row(
                        Modifier.padding(end = 12.dp).height(34.dp).clip(CircleShape)
                            .background(if (showTranscript) p.tonal else Color.Transparent)
                            .border(1.dp, if (showTranscript) p.accentInk.copy(alpha = 0.4f) else p.lineStrong, CircleShape)
                            .pressable(pressedScale = 0.94f) { showTranscript = !showTranscript; haptic.performHapticFeedback(HapticFeedbackType.SegmentTick) }
                            .padding(horizontal = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(
                            if (showTranscript) Icons.Rounded.Visibility else Icons.Rounded.VisibilityOff,
                            if (showTranscript) "Hide the transcript" else "Show the transcript",
                            tint = if (showTranscript) p.accentInk else p.textDim, modifier = Modifier.size(17.dp),
                        )
                        Spacer(Modifier.width(7.dp))
                        Text("Transcript", style = type.caption.copy(color = if (showTranscript) p.accentInk else p.textDim, fontWeight = androidx.compose.ui.text.font.FontWeight.Medium))
                    }
                }
            }
            }

            // The jot box replaces the controls while you type.
            val bottom: @Composable () -> Unit = {
                AnimatedContent(
                    targetState = composing,
                    transitionSpec = { (fadeIn(Motion.fade(200)) + slideInVertically(Motion.snappy()) { it / 3 }) togetherWith fadeOut(Motion.fade(120)) },
                    label = "bottom",
                ) { c ->
                    if (c) {
                        Composer(
                            draft = draft, onDraft = { draft = it }, kind = kind, onKind = { kind = it },
                            editing = editing != null, onSend = send,
                            onClose = { composing = false; editing = null; draft = "" },
                            onCamera = { camera() },
                            onIdeas = { vm.suggestQuestions(); composing = false },
                            inPanel = split,
                        )
                    } else {
                        Controls(
                            recording = s.recording,
                            photoCount = s.photos.size,
                            jotCount = s.jots.size,
                            onStop = { vm.stopRecording() },
                            onCamera = { camera() },
                            onGallery = { gallery() },
                            onJot = { composing = true },
                            inPanel = split,
                        )
                    }
                }
            }
            val timeline: @Composable (Modifier) -> Unit = { m ->
                Timeline(
                    s = s, list = list, notesOnly = notesOnly, onCite = onCite,
                    onPhoto = { viewing = it },
                    onReadPhoto = vm::readPhotoNow,
                    onKeep = { vm.addJot(it.text, "keep", it.t0) },
                    onEdit = { j -> editing = j; draft = j.raw; kind = j.kind; composing = true },
                    onDeleteJot = { vm.deleteJot(it) },
                    onRemoveCard = { vm.deleteCard(it, live = true) },
                    onPickIdea = { q -> vm.addJot(q, "question"); vm.toast("success", "Kept in your notes as a question.") },
                    modifier = m,
                )
            }
            val folderPill: @Composable (Modifier) -> Unit = { m ->
                // Live Assistance answers only from this folder.
                FolderPill(folders.firstOrNull { it.id == liveFolder }, onClick = { pickingFolder = true }, emptyLabel = "All knowledge", modifier = m)
            }

            AnimatedContent(
                targetState = s.wrap == null,
                transitionSpec = { fadeIn(Motion.fade(260)) togetherWith fadeOut(Motion.fade(160)) },
                label = "live-body",
                modifier = Modifier.weight(1f),
            ) { recording ->
                if (recording && split) {
                    val panelW by androidx.compose.animation.core.animateDpAsState(if (hideChrome) 440.dp else if (short) 300.dp else 340.dp, tween(220, easing = Motion.Emphasized), label = "panel")
                    // ---- transcript on the left, status and controls on the right
                    Row(Modifier.fillMaxSize()) {
                        Box(Modifier.weight(1f).fillMaxHeight(), contentAlignment = Alignment.TopCenter) {
                            timeline(Modifier.widthIn(max = ReadingWidth).fillMaxHeight())
                        }
                        Column(
                            Modifier.width(panelW).fillMaxHeight().padding(end = 8.dp),
                            verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally,
                        ) {
                            if (!hideChrome) {
                                folderPill(Modifier)
                                Spacer(Modifier.height(4.dp))
                                ActivityStrip(s.activity, s.recording, note = if (showTranscript) correctionLabel(s.correctedBy, s.correctionPhase, s.workFrom, s.workTo, s.checkedUpto, prefsNow.liveCorrectionWindowS) else null)
                            }
                            bottom()
                        }
                    }
                } else if (recording) {
                    Column(Modifier.fillMaxSize()) {
                        ActivityStrip(
                            s.activity, s.recording,
                            note = if (showTranscript) correctionLabel(s.correctedBy, s.correctionPhase, s.workFrom, s.workTo, s.checkedUpto, prefsNow.liveCorrectionWindowS) else null,
                            trailing = { folderPill(Modifier) },
                        )
                        timeline(Modifier.weight(1f))
                    }
                } else {
                    WrapUp(s, Modifier.fillMaxSize())
                }
            }

            // ---- bottom: controls, or the composer when you are jotting
            if (s.wrap == null && !split) bottom()
        }
        }

        if (pickingFolder) {
            FolderPicker(
                title = "Answer from", folders = folders, current = liveFolder, noneLabel = "All knowledge",
                onPick = { vm.setLiveFolder(it); pickingFolder = false },
                onNew = { folderUi.afterCreate = { id -> vm.setLiveFolder(id) }; pickingFolder = false; folderUi.creating = true },
                onDismiss = { pickingFolder = false },
            )
        }
        FolderSheets(vm, folderUi)
        AnimatedVisibility(viewing != null, enter = fadeIn() + scaleIn(initialScale = 0.92f), exit = fadeOut() + scaleOut(targetScale = 0.96f)) {
            val ph = viewing?.let { v -> s.photos.firstOrNull { it.id == v.id } ?: v }
            if (ph != null) PhotoViewer(ph, onClose = { viewing = null }, onDelete = { vm.deletePhoto(ph); viewing = null })
        }
    }
}

// ------------------------------------------------------------------------------------------------

@Composable
private fun Timeline(
    s: LiveState,
    list: LazyListState,
    notesOnly: Boolean,
    onCite: (Source) -> Unit,
    onPhoto: (Photo) -> Unit,
    onReadPhoto: (Photo) -> Unit,
    onKeep: (Line) -> Unit,
    onEdit: (Jot) -> Unit,
    onDeleteJot: (Jot) -> Unit,
    onPickIdea: (String) -> Unit,
    onRemoveCard: (Card) -> Unit,
    modifier: Modifier = Modifier,
) {
    val p = pal
    val scope = rememberCoroutineScope()
    val items = remember(s.lines, s.jots, s.photos, s.cards, notesOnly) { moments(s, notesOnly) }
    val extra = if (!notesOnly && (s.partial.isNotBlank() || s.lines.isEmpty())) 1 else 0
    val total = items.size + extra
    val atEnd by remember { derivedStateOf { val last = list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0; last >= list.layoutInfo.totalItemsCount - 2 } }
    val newestLine = s.lines.lastOrNull()?.id

    LaunchedEffect(items.size, s.partial.length / 10, notesOnly) {
        if (atEnd && total > 0) list.animateScrollToItem((total - 1).coerceAtLeast(0))
    }
    Box(modifier.fillMaxWidth()) {
        if (items.isEmpty() && s.partial.isBlank()) {
            Column(Modifier.fillMaxSize().padding(horizontal = 40.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                Text(if (notesOnly) "Nothing noted yet" else "Listening", style = type.title, textAlign = TextAlign.Center)
                Spacer(Modifier.height(8.dp))
                Text(
                    if (notesOnly) "Tap Jot to note something. The AI polishes it with what was said."
                    else "Words appear here as people speak. Tap Jot to note something, or drag a line to the right to keep it.",
                    style = type.callout, textAlign = TextAlign.Center,
                )
            }
        }
        LazyColumn(state = list, modifier = Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 6.dp, bottom = 14.dp)) {
            items(items, key = { it.key }) { m ->
                when (m) {
                    // Rows do not animate at all: no fade, no slide, on arrival, on removal or when others move. A line the compute node fixed
                    // replaces the old one on the spot, and a row that fades or slides while the rows around it are replaced was left behind half
                    // transparent over the transcript (a closed card stayed as a ghost, and cards drew over fixed lines).
                    is Moment.Said -> SwipeToKeep(onKeep = { onKeep(m.line) }) {
                        TranscriptRow(m.line, animateIn = m.line.id !in s.freshFixes, dim = if (m.line.id == newestLine) 1f else 0.7f, fresh = m.line.id in s.freshFixes)
                    }
                    is Moment.Noted -> SwipeToDelete(
                        onDelete = { onDeleteJot(m.jot) },
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                    ) { JotCard(m.jot, Modifier.padding(0.dp), onEdit = { onEdit(m.jot) }, flat = true) }
                    is Moment.Shot -> PhotoCard(m.photo, Modifier, onReadNow = { onReadPhoto(m.photo) }, onOpen = { onPhoto(m.photo) })
                    is Moment.Asked -> Column(Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) {
                        val label = when (m.card.kind) { "ask" -> "YOU ASKED"; "ideas" -> null; else -> "FROM YOUR KNOWLEDGE" }
                        if (label != null) Text(label, style = type.micro.copy(color = p.accentInk), modifier = Modifier.padding(start = 4.dp, bottom = 6.dp))
                        AssistCard(m.card, onCite = onCite, onPick = onPickIdea, onRemove = { onRemoveCard(m.card) })
                    }
                }
            }
            if (extra == 1 && s.partial.isNotBlank()) item(key = "partial") { PartialRow(s.partial) }
        }
        AnimatedVisibility(
            visible = !atEnd && items.isNotEmpty(),
            enter = fadeIn() + scaleIn(), exit = fadeOut() + scaleOut(),
            modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 10.dp),
        ) {
            // Solid and dark in both themes, so the transcript underneath never shows through it.
            val shape = RoundedCornerShape(50)
            Row(
                Modifier.shadow(10.dp, shape).clip(shape).background(Color(0xFF15151C)).border(1.dp, Color(0xFF3A3A48), shape)
                    .pressable(pressedScale = 0.96f, onClick = { scope.launch { list.animateScrollToItem((total - 1).coerceAtLeast(0)) } })
                    .padding(horizontal = 18.dp, vertical = 11.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.Rounded.ArrowDownward, null, tint = Color(0xFF22D3EE), modifier = Modifier.size(16.dp))
                Spacer(Modifier.width(8.dp))
                Text("Jump to live", style = type.label.copy(color = Color.White))
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Composer
// ------------------------------------------------------------------------------------------------

@Composable
private fun Composer(
    draft: String,
    onDraft: (String) -> Unit,
    kind: String,
    onKind: (String) -> Unit,
    editing: Boolean,
    onSend: () -> Unit,
    onClose: () -> Unit,
    onCamera: () -> Unit,
    onIdeas: () -> Unit,
    /** In the side panel there is no screen edge below it, so no fade and no navigation bar padding. */
    inPanel: Boolean = false,
) {
    val p = pal
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    val haptic = LocalHapticFeedback.current
    Column(
        Modifier.fillMaxWidth()
            .then(if (inPanel) Modifier else Modifier.background(androidx.compose.ui.graphics.Brush.verticalGradient(listOf(Color.Transparent, p.surface.copy(alpha = 0.96f), p.surface))).navigationBarsPadding())
            .padding(top = 8.dp, bottom = 10.dp),
    ) {
        Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (!editing) Chip("Questions to ask", selected = false) { haptic.performHapticFeedback(HapticFeedbackType.ContextClick); onIdeas() }
            jotKinds.forEach { (k, label) ->
                Chip(label, selected = kind == k) { onKind(k); haptic.performHapticFeedback(HapticFeedbackType.SegmentTick) }
            }
            if (!editing) Chip("Ask the AI", selected = kind == "ask") { onKind("ask"); haptic.performHapticFeedback(HapticFeedbackType.SegmentTick) }
        }
        Spacer(Modifier.height(10.dp))
        val shape = RoundedCornerShape(26.dp)
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp).clip(shape).background(p.surface3).border(1.dp, p.lineStrong, shape).padding(start = 6.dp, end = 6.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.Bottom,
        ) {
            // A picture without leaving the note box.
            if (!editing) Box(
                Modifier.size(44.dp).clip(CircleShape).pressable(pressedScale = 0.88f, onClick = onCamera),
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Rounded.PhotoCamera, "Take a photo", tint = p.accentInk, modifier = Modifier.size(22.dp)) }
            Box(Modifier.weight(1f).padding(vertical = 10.dp).padding(start = if (editing) 12.dp else 4.dp)) {
                if (draft.isEmpty()) Text(if (editing) "Edit your note" else if (kind == "ask") "Ask about what was said" else "Jot it down. Short is fine", style = type.body.copy(color = p.textFaint))
                BasicTextField(
                    value = draft, onValueChange = onDraft, textStyle = type.body, cursorBrush = SolidColor(p.accentInk), maxLines = 4,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = androidx.compose.ui.text.input.ImeAction.Send),
                    keyboardActions = androidx.compose.foundation.text.KeyboardActions(onSend = { onSend() }),
                    modifier = Modifier.fillMaxWidth().heightIn(max = 110.dp).focusRequester(focus),
                )
            }
            Box(
                Modifier.size(44.dp).clip(CircleShape).pressable(pressedScale = 0.88f, onClick = onClose),
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Rounded.Close, "Close the note box", tint = p.textDim, modifier = Modifier.size(22.dp)) }
            val ready = draft.isNotBlank()
            Box(
                Modifier.size(44.dp).clip(CircleShape).background(if (ready) p.accentBrush else androidx.compose.ui.graphics.Brush.linearGradient(listOf(p.surface4, p.surface4)))
                    .pressable(enabled = ready, pressedScale = 0.88f, onClick = onSend),
                contentAlignment = Alignment.Center,
            ) { Icon(if (editing) Icons.Rounded.CheckCircle else Icons.Rounded.ArrowUpward, if (editing) "Save the change" else "Add the note", tint = if (ready) p.onAccent else p.textFaint, modifier = Modifier.size(22.dp)) }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Controls
// ------------------------------------------------------------------------------------------------

@Composable
private fun Controls(
    recording: Boolean,
    photoCount: Int,
    jotCount: Int,
    onStop: () -> Unit,
    onCamera: () -> Unit,
    onGallery: () -> Unit,
    onJot: () -> Unit,
    inPanel: Boolean = false,
) {
    val p = pal
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
    Row(
        Modifier.widthIn(max = 440.dp).fillMaxWidth().then(if (inPanel) Modifier else Modifier.navigationBarsPadding()).padding(horizontal = if (inPanel) 12.dp else 28.dp).padding(top = 6.dp, bottom = 10.dp).height(112.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        LabelledButton("Photo", badge = photoCount) {
            Box(
                Modifier.size(56.dp).clip(CircleShape).background(p.surface3).border(1.dp, p.line, CircleShape)
                    .pressable(pressedScale = 0.88f, haptic = HapticFeedbackType.ContextClick, onLongClick = onGallery, onClick = onCamera),
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Rounded.PhotoCamera, "Add a photo", tint = p.text, modifier = Modifier.size(26.dp)) }
        }
        HoldToStop(enabled = recording, onStop = onStop)
        LabelledButton("Jot", badge = jotCount) {
            Box(
                Modifier.size(56.dp).shadow(10.dp, CircleShape, ambientColor = p.accent, spotColor = p.accent).clip(CircleShape).background(p.accentBrush)
                    .pressable(pressedScale = 0.88f, haptic = HapticFeedbackType.ContextClick, onClick = onJot),
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Rounded.Edit, "Jot a note", tint = p.onAccent, modifier = Modifier.size(25.dp)) }
        }
    }
    }
}

@Composable
private fun LabelledButton(label: String, badge: Int, content: @Composable () -> Unit) {
    val p = pal
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Box {
            content()
            if (badge > 0) Text(
                "$badge", style = type.micro.copy(color = p.onAccent, letterSpacing = 0.sp()),
                modifier = Modifier.align(Alignment.TopEnd).offset(x = 4.dp, y = (-2).dp).clip(CircleShape).background(p.accentInk.copy(alpha = if (p.dark) 1f else 1f)).padding(horizontal = 6.dp, vertical = 2.dp),
            )
        }
        Spacer(Modifier.height(6.dp))
        Text(label, style = type.caption.copy(color = p.textDim))
    }
}

private fun Int.sp() = androidx.compose.ui.unit.TextUnit(this.toFloat(), androidx.compose.ui.unit.TextUnitType.Sp)

/** Stopping a meeting is final, so it takes a deliberate hold: a ring fills, then it lets go. */
@Composable
private fun HoldToStop(enabled: Boolean, onStop: () -> Unit) {
    val p = pal
    val haptic = LocalHapticFeedback.current
    val scope = rememberCoroutineScope()
    val progress = remember { Animatable(0f) }
    var hint by remember { mutableStateOf(false) }
    LaunchedEffect(hint) { if (hint) { delay(1600); hint = false } }
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Box(contentAlignment = Alignment.Center) {
            val press by animateFloatAsState(if (progress.value > 0f) 0.94f else 1f, Motion.snappy(), label = "hold")
            Box(
                Modifier.size(88.dp).graphicsLayer { scaleX = press; scaleY = press }
                    .semantics { role = androidx.compose.ui.semantics.Role.Button; if (enabled) onLongClick("Stop recording") { onStop(); true } }
                    .pointerInput(enabled) {
                        if (!enabled) return@pointerInput
                        detectTapGestures(onPress = {
                            val job = scope.launch {
                                var tick = 0
                                progress.animateTo(1f, androidx.compose.animation.core.tween(700, easing = androidx.compose.animation.core.LinearEasing)) {
                                    val t = (value * 3).toInt()
                                    if (t > tick) { tick = t; haptic.performHapticFeedback(HapticFeedbackType.SegmentFrequentTick) }
                                }
                                haptic.performHapticFeedback(HapticFeedbackType.Confirm)
                                onStop()
                            }
                            val released = tryAwaitRelease()
                            if (progress.value < 1f) {
                                job.cancel()
                                scope.launch { progress.animateTo(0f, Motion.snappy()) }
                                if (released) hint = true
                            }
                        })
                    },
                contentAlignment = Alignment.Center,
            ) {
                Canvas(Modifier.fillMaxSize()) {
                    val w = 5.dp.toPx()
                    drawCircle(p.dangerFill.copy(alpha = 0.22f), style = Stroke(w))
                    drawArc(p.danger, -90f, 360f * progress.value, false, topLeft = Offset(w / 2, w / 2), size = androidx.compose.ui.geometry.Size(size.width - w, size.height - w), style = Stroke(w, cap = StrokeCap.Round))
                }
                Box(Modifier.size(68.dp).shadow(10.dp, CircleShape, ambientColor = p.dangerFill, spotColor = p.dangerFill).clip(CircleShape).background(p.dangerFill), contentAlignment = Alignment.Center) {
                    Icon(Icons.Rounded.Stop, "Stop recording. Press and hold.", tint = Color.White, modifier = Modifier.size(34.dp))
                }
            }
        }
        AnimatedContent(hint, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "hold-hint") { h ->
            Text(if (h) "Keep holding to stop" else "Hold to stop", style = type.caption.copy(color = if (h) p.text else p.textDim))
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Wrap-up
// ------------------------------------------------------------------------------------------------

private val steps = listOf(
    "finalizing" to "Finalizing the transcript",
    "improving" to "Improving the transcript",
    "jots" to "Polishing your notes",
    "photos" to "Reading your photos",
    "summarizing" to "Writing the summary",
    "titling" to "Naming the meeting",
    "indexing" to "Making it searchable",
)

@Composable
private fun WrapUp(s: LiveState, modifier: Modifier = Modifier) {
    val p = pal
    val w = s.wrap ?: return
    val current = steps.indexOfFirst { it.first == w.step }.coerceAtLeast(0)
    val prog by animateFloatAsState(w.progress / 100f, Motion.soft(), label = "wrap")
    // The step with a compute node only shows when it ran, and stays once it has.
    var improved by remember(s.meetingId) { mutableStateOf(false) }
    if (w.step == "improving") improved = true
    // Centred when it fits, and it scrolls when the window is too short for all the steps.
    BoxWithConstraints(modifier, contentAlignment = Alignment.TopCenter) {
    val minH = maxHeight
    Column(
        Modifier.widthIn(max = 520.dp).verticalScroll(rememberScrollState()).heightIn(min = minH).padding(horizontal = 28.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("Putting it together", style = type.title)
        Spacer(Modifier.height(6.dp))
        Text("Everything happens on this phone", style = type.caption)
        Spacer(Modifier.height(28.dp))
        Box(Modifier.fillMaxWidth().height(6.dp).clip(CircleShape).background(p.surface4)) {
            Box(Modifier.fillMaxWidth(prog.coerceIn(0.04f, 1f)).fillMaxHeight().clip(CircleShape).background(p.accentBrush))
        }
        Spacer(Modifier.height(26.dp))
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            steps.forEachIndexed { i, (k, label) ->
                if (k == "improving" && !improved) return@forEachIndexed
                if (k == "photos" && s.photos.isEmpty()) return@forEachIndexed
                if (k == "jots" && s.jots.isEmpty()) return@forEachIndexed
                val done = i < current
                val now = i == current
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
                        when {
                            done -> Icon(Icons.Rounded.CheckCircle, null, tint = p.success, modifier = Modifier.size(22.dp))
                            now -> PulseDot(p.accentInk, 10.dp)
                            else -> Box(Modifier.size(8.dp).clip(CircleShape).background(p.lineStrong))
                        }
                    }
                    Spacer(Modifier.width(12.dp))
                    Text(if (now) w.label.ifBlank { label } else label, style = type.body.copy(color = if (now) p.text else if (done) p.textDim else p.textFaint))
                }
            }
        }
        if (s.summary.isNotBlank() && s.summaryStreaming) {
            Spacer(Modifier.height(24.dp))
            Column(
                Modifier.fillMaxWidth().heightIn(max = 180.dp).clip(RoundedCornerShape(p.rMid)).background(p.surface3).border(1.dp, p.line, RoundedCornerShape(p.rMid)).verticalScroll(rememberScrollState()).padding(14.dp),
            ) { Text(s.summary.takeLast(600), style = type.callout) }
        }
    }
    }
}
