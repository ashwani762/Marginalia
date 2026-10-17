// The app shell: tabs, navigation, the floating bar, toasts and the screens that sit on top.

@file:OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)

package app.marginalia.ui

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.animation.core.spring
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandHorizontally
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.AutoStories
import androidx.compose.material.icons.rounded.ChatBubbleOutline
import androidx.compose.material.icons.rounded.Mic
import androidx.compose.material.icons.rounded.Person
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.imePadding
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.unit.Velocity
import kotlin.math.abs
import androidx.core.content.ContextCompat
import app.marginalia.AppVm
import app.marginalia.Boot
import app.marginalia.RecorderBus
import app.marginalia.Toast
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

private enum class Tab(val label: String) { Meetings("Meetings"), Knowledge("Knowledge"), Ask("Ask") }

@Composable
fun App(vm: AppVm) {
    val prefs by vm.prefs.collectAsState()
    MarginaliaTheme(prefs.appearance) {
        val boot by vm.boot.collectAsState()
        androidx.compose.runtime.CompositionLocalProvider(LocalWin provides rememberWin()) {
            KeyboardGuard(Modifier.fillMaxSize().background(pal.surface)) {
                // On its side a phone has a cutout or the navigation bar at one edge: keep the app clear of both.
                Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.displayCutout.union(WindowInsets.navigationBars).only(WindowInsetsSides.Horizontal))) {
                    val introduced by vm.introduced.collectAsState()
                    when (val b = boot) {
                        is Boot.Unpacking, is Boot.Failed -> SetupScreen(b, onRetry = vm::retryBoot)
                        else -> if (introduced) Shell(vm) else Onboarding(onDone = vm::finishIntro)
                    }
                    // While Gemma loads at app start the main screen is covered by a still loading screen with a bar.
                    val warm by vm.aiWarm.collectAsState()
                    androidx.compose.animation.AnimatedVisibility(
                        visible = warm != null && boot !is Boot.Unpacking && boot !is Boot.Failed && introduced,
                        enter = androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(150)),
                        exit = androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(250)),
                    ) {
                        warm?.let { AiWarmScreen(it, onSkip = vm::skipAiWarm) }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Keyboard: one place that lets it get out of the way on every screen
// ------------------------------------------------------------------------------------------------

/**
 * The root of the app. A tap on empty space, or dragging a list, hides the keyboard, and a small
 * button above the keyboard hides it too. Taps that a text field or a button handles are left alone.
 * Bottom sheets live in their own window, so they keep the system behaviour.
 */
@Composable
private fun KeyboardGuard(modifier: Modifier, content: @Composable BoxScope.() -> Unit) {
    val p = pal
    val keyboard = LocalSoftwareKeyboardController.current
    val focus = LocalFocusManager.current
    val density = LocalDensity.current
    val imeVisible = WindowInsets.ime.getBottom(density) > 0
    val visible by rememberUpdatedState(imeVisible)
    val hide by rememberUpdatedState({ keyboard?.hide(); focus.clearFocus() })
    val slop = with(density) { 16.dp.toPx() }
    val connection = remember(slop) {
        object : NestedScrollConnection {
            var travelled = 0f
            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                if (source == NestedScrollSource.UserInput && visible) {
                    travelled += abs(available.y)
                    if (travelled > slop) { travelled = 0f; keyboard?.hide() }
                }
                return Offset.Zero
            }
            override suspend fun onPreFling(available: Velocity): Velocity { travelled = 0f; return Velocity.Zero }
        }
    }
    Box(
        modifier
            .nestedScroll(connection)
            .pointerInput(Unit) {
                awaitEachGesture {
                    // Last in line: only a touch that nothing below claimed counts as a tap on empty space.
                    val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Final)
                    if (down.isConsumed || !visible) return@awaitEachGesture
                    val up = waitForUpOrCancellation(PointerEventPass.Final)
                    if (up != null && !up.isConsumed) hide()
                }
            },
    ) {
        content()
        AnimatedVisibility(
            visible = imeVisible,
            enter = fadeIn() + scaleIn(),
            exit = fadeOut() + scaleOut(),
            // Clear of the note box and the Ask box, which sit right on the keyboard.
            modifier = Modifier.align(Alignment.BottomEnd).imePadding().padding(end = 8.dp, bottom = 128.dp),
        ) {
            Box(Modifier.size(48.dp).pressable(pressedScale = 0.88f, onClick = { hide() }), contentAlignment = Alignment.Center) {
                Box(
                    Modifier.size(36.dp).shadow(4.dp, CircleShape).clip(CircleShape).background(p.surface3.copy(alpha = 0.96f)).border(1.dp, p.line, CircleShape),
                    contentAlignment = Alignment.Center,
                ) { Icon(Icons.Rounded.KeyboardArrowDown, "Hide keyboard", tint = p.textDim, modifier = Modifier.size(22.dp)) }
            }
        }
    }
}

@Composable
private fun Shell(vm: AppVm) {
    val p = pal
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val haptic = LocalHapticFeedback.current
    val boot by vm.boot.collectAsState()
    val live by vm.live.collectAsState()
    val expanded by vm.liveExpanded.collectAsState()
    val review by vm.review.collectAsState()
    val openDoc by vm.doc.collectAsState()
    var tab by rememberSaveable { mutableIntStateOf(0) }
    var showYou by remember { mutableStateOf(false) }
    var showHome by remember { mutableStateOf(false) }
    var citeSheet by remember { mutableStateOf<app.marginalia.Source?>(null) }
    val ready = boot is Boot.Ready
    val imeVisible = WindowInsets.ime.getBottom(androidx.compose.ui.platform.LocalDensity.current) > 0
    val w = win
    val rail = w.rail
    // Side by side, the review lives in the Meetings screen; on one pane it covers everything.
    val reviewOverlay = review != null && !w.twoPane

    // Microphone permission, then record.
    var pendingStart by remember { mutableStateOf(false) }
    val notifPerm = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    val micPerm = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted && pendingStart) vm.startRecording()
        else if (!granted) vm.toast("warning", "Marginalia needs the microphone to record. You can allow it in Settings.")
        pendingStart = false
    }
    val startRecording = {
        when {
            !ready -> vm.toast("info", "The AI is still waking up. One moment.")
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED -> {
                if (Build.VERSION.SDK_INT >= 33 &&
                    ContextCompat.checkSelfPermission(ctx, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
                ) notifPerm.launch(Manifest.permission.POST_NOTIFICATIONS)
                vm.startRecording()
            }
            else -> { pendingStart = true; micPerm.launch(Manifest.permission.RECORD_AUDIO) }
        }
    }

    LaunchedEffect(Unit) { vm.navigate.collect { tab = it } }
    val liveOpen = live != null && expanded
    BackHandler(enabled = liveOpen) { vm.liveExpanded.value = false }
    BackHandler(enabled = !liveOpen && review == null && tab != 0) { tab = 0 }
    // A meeting opened from a notice or a toast shows in the Meetings tab when it sits beside the list.
    LaunchedEffect(review?.meeting?.id, w.twoPane) { if (w.twoPane && review != null) tab = 0 }

    val onRecord = { if (live != null) vm.liveExpanded.value = true else startRecording() }
    val onSelect = { it: Int ->
        if (it != tab) haptic.performHapticFeedback(HapticFeedbackType.SegmentTick)
        tab = it
    }
    Box(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxSize()) {
            // ----- navigation rail, when the window is short or wide
            if (rail) {
                AnimatedVisibility(
                    visible = !imeVisible,
                    enter = fadeIn(tween(200)) + expandHorizontally(tween(220, easing = Motion.Emphasized)),
                    exit = fadeOut(tween(150)) + shrinkHorizontally(tween(180, easing = Motion.Emphasized)),
                ) {
                    NavRail(selected = tab, recording = live != null, warming = !ready, onSelect = onSelect, onRecord = onRecord, onYou = { showYou = true })
                }
            }
            Box(Modifier.weight(1f).fillMaxHeight()) {
                // ----- tabs
                // While the live or review screen covers this one, it is hidden from screen readers.
                val covered = liveOpen || reviewOverlay
                AnimatedContent(
                    targetState = tab,
                    transitionSpec = {
                        val dir = if (targetState > initialState) 1 else -1
                        (fadeIn(tween(220)) + slideInHorizontally(Motion.soft()) { it / 10 * dir }) togetherWith
                            (fadeOut(tween(120)) + slideOutHorizontally(Motion.soft()) { -it / 10 * dir })
                    },
                    label = "tabs",
                    modifier = Modifier.fillMaxSize().then(if (covered) Modifier.clearAndSetSemantics { } else Modifier),
                ) { t ->
                    when (Tab.entries[t]) {
                        Tab.Meetings -> MeetingsHost(
                            vm, review, onRecord = startRecording, sideBySide = w.twoPane, backEnabled = !liveOpen,
                            onCite = { citeSheet = it },
                            onAsk = { q -> vm.closeReview(); tab = Tab.Ask.ordinal; vm.ask(q) },
                            onOpenHome = { showHome = true; showYou = true },
                        )
                        Tab.Knowledge -> KnowledgeScreen(vm, onCite = { citeSheet = it })
                        Tab.Ask -> AskScreen(vm, onCite = { citeSheet = it })
                    }
                }

                // ----- floating bar + mini recorder (a side rail replaces the bar)
                val barHidden = imeVisible || reviewOverlay || liveOpen
                MiniBar.visible = !barHidden && !rail && live != null && !expanded
                androidx.compose.animation.AnimatedVisibility(
                    visible = !barHidden,
                    enter = slideInVertically(Motion.soft()) { it } + fadeIn(),
                    exit = slideOutVertically(Motion.soft()) { it } + fadeOut(),
                    modifier = Modifier.align(Alignment.BottomCenter),
                ) {
                    androidx.compose.foundation.layout.Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        androidx.compose.animation.AnimatedVisibility(
                            visible = live != null && !expanded,
                            enter = slideInVertically(Motion.bouncy()) { it / 2 } + fadeIn(),
                            exit = slideOutVertically(Motion.snappy()) { it / 2 } + fadeOut(),
                        ) {
                            Box(if (rail) Modifier.navigationBarsPadding().padding(bottom = 10.dp) else Modifier) {
                                MiniRecorder(vm, onExpand = { vm.liveExpanded.value = true })
                            }
                        }
                        if (!rail) {
                            FloatingBar(
                                selected = tab,
                                recording = live != null,
                                warming = !ready,
                                onSelect = onSelect,
                                onRecord = onRecord,
                                onYou = { showYou = true },
                            )
                        }
                    }
                }
            }
        }

        // ----- review on a single pane (pushes in from the right)
        AnimatedVisibility(
            visible = reviewOverlay,
            enter = slideInHorizontally(Motion.soft()) { it } + fadeIn(tween(120)),
            exit = slideOutHorizontally(tween(220)) { it } + fadeOut(tween(200)),
            modifier = Modifier.zIndex(2f),
        ) {
            val d = review
            if (d != null) ReviewScreen(vm, d, onCite = { citeSheet = it }, onAsk = { q -> vm.closeReview(); tab = Tab.Ask.ordinal; vm.ask(q) }, backEnabled = !liveOpen)
        }

        // ----- a document from Knowledge (pushes in from the right)
        AnimatedVisibility(
            visible = openDoc != null && !liveOpen,
            enter = slideInHorizontally(Motion.soft()) { it } + fadeIn(tween(120)),
            exit = slideOutHorizontally(tween(220)) { it } + fadeOut(tween(200)),
            modifier = Modifier.zIndex(2.5f),
        ) {
            val dd = openDoc
            if (dd != null) DocumentScreen(vm, dd, onAsk = { q -> vm.closeDoc(); tab = Tab.Ask.ordinal; vm.ask(q) })
        }

        // ----- live meeting (rises from the bottom)
        AnimatedVisibility(
            visible = liveOpen,
            enter = slideInVertically(Motion.soft()) { it } + fadeIn(tween(160)),
            exit = ExitTransition.None,
            modifier = Modifier.zIndex(3f),
        ) {
            LiveScreen(vm, onMinimize = { vm.liveExpanded.value = false }, onCite = { citeSheet = it })
        }

        // ----- toasts
        ToastHost(vm, modifier = Modifier.align(Alignment.TopCenter).zIndex(5f))
    }

    if (showYou) YouSheet(vm, onDismiss = { showYou = false; showHome = false }, scrollToHome = showHome)
    ApprovalPrompt(vm)
    citeSheet?.let { SourceSheet(it, onDismiss = { citeSheet = null }) }
}

// ------------------------------------------------------------------------------------------------
// Floating bottom bar
// ------------------------------------------------------------------------------------------------

@Composable
private fun FloatingBar(
    selected: Int,
    recording: Boolean,
    warming: Boolean,
    onSelect: (Int) -> Unit,
    onRecord: () -> Unit,
    onYou: () -> Unit,
) {
    val p = pal
    Box(
        Modifier
            .widthIn(max = 560.dp)
            .fillMaxWidth()
            .navigationBarsPadding()
            .padding(horizontal = 16.dp)
            .padding(bottom = 10.dp, top = 8.dp)
            .height(76.dp),
        contentAlignment = Alignment.BottomCenter,
    ) {
        // The bar itself.
        Row(
            Modifier
                .fillMaxWidth()
                .height(64.dp)
                .shadow(if (p.dark) 18.dp else 10.dp, RoundedCornerShape(32.dp), ambientColor = Color.Black, spotColor = Color.Black)
                .clip(RoundedCornerShape(32.dp))
                .background(if (p.dark) p.surface3.copy(alpha = 0.96f) else p.surface2.copy(alpha = 0.97f))
                .border(1.dp, p.line, RoundedCornerShape(32.dp)),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            BarItem(Icons.Rounded.GraphicEq, "Meetings", selected == 0, Modifier.weight(1f)) { onSelect(0) }
            BarItem(Icons.Rounded.AutoStories, "Knowledge", selected == 1, Modifier.weight(1f)) { onSelect(1) }
            Box(Modifier.width(84.dp))
            BarItem(Icons.Rounded.ChatBubbleOutline, "Ask", selected == 2, Modifier.weight(1f)) { onSelect(2) }
            BarItem(Icons.Rounded.Person, "You", false, Modifier.weight(1f)) { onYou() }
        }
        // The record button sits in the middle, rising out of the bar.
        RecordButton(recording = recording, warming = warming, onClick = onRecord, modifier = Modifier.align(Alignment.TopCenter).offset(y = (-4).dp))
    }
}

@Composable
private fun BarItem(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, selected: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val p = pal
    val tint by animateFloatAsState(if (selected) 1f else 0f, tween(if (reducedMotion()) 0 else 160, easing = Motion.Emphasized), label = "tint")
    Box(
        modifier.height(64.dp).pressable(pressedScale = 0.9f, onClick = onClick)
            .semantics { role = androidx.compose.ui.semantics.Role.Tab; this.selected = selected },
        contentAlignment = Alignment.Center,
    ) {
        androidx.compose.foundation.layout.Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Icon(
                icon, null,
                tint = androidx.compose.ui.graphics.lerp(p.textFaint, p.accentInk, tint),
                modifier = Modifier.size(24.dp).graphicsLayer { val s = 1f + 0.12f * tint; scaleX = s; scaleY = s },
            )
            Text(label, style = type.micro.copy(color = androidx.compose.ui.graphics.lerp(p.textFaint, p.accentInk, tint), letterSpacing = 0.2.sp()), maxLines = 1)
        }
    }
}

private fun Double.sp() = androidx.compose.ui.unit.TextUnit(this.toFloat(), androidx.compose.ui.unit.TextUnitType.Sp)

@Composable
private fun RecordButton(recording: Boolean, warming: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier, diameter: androidx.compose.ui.unit.Dp = 72.dp) {
    val p = pal
    val breathe = rememberInfiniteTransition(label = "breathe")
    val glow by breathe.animateFloat(0.55f, 1f, infiniteRepeatable(tween(2200), RepeatMode.Reverse), label = "glow")
    val spin by breathe.animateFloat(0f, 360f, infiniteRepeatable(tween(1100, easing = LinearEasing)), label = "spin")
    // While recording the button breathes: 1.0 to 1.03 over 2 seconds.
    val still = reducedMotion()
    val swell by breathe.animateFloat(1f, 1.03f, infiniteRepeatable(tween(2000, easing = androidx.compose.animation.core.FastOutSlowInEasing), RepeatMode.Reverse), label = "swell")
    Box(
        modifier
            .size(diameter)
            .graphicsLayer { if (recording && !still) { scaleX = swell; scaleY = swell } }
            .shadow(16.dp, CircleShape, ambientColor = p.accent, spotColor = p.accent)
            .clip(CircleShape)
            .background(if (recording) Brush.linearGradient(listOf(p.dangerFill, p.dangerFill)) else p.accentBrush)
            .border(3.dp, if (p.dark) p.surface else p.surface2, CircleShape)
            .pressable(pressedScale = 0.9f, haptic = HapticFeedbackType.ContextClick, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        if (!recording && !warming) {
            Canvas(Modifier.fillMaxSize()) {
                drawCircle(Brush.radialGradient(listOf(Color.White.copy(alpha = 0.22f * glow), Color.Transparent)), radius = size.minDimension / 2f)
            }
        }
        if (warming) {
            Canvas(Modifier.size(34.dp)) {
                drawArc(Color.White.copy(alpha = 0.25f), 0f, 360f, false, style = Stroke(3.dp.toPx()))
                drawArc(Color.White, spin, 100f, false, style = Stroke(3.dp.toPx(), cap = StrokeCap.Round))
            }
        } else {
            Icon(
                if (recording) Icons.Rounded.GraphicEq else Icons.Rounded.Mic,
                if (recording) "Open recording" else "Start recording",
                tint = Color.White, modifier = Modifier.size(32.dp),
            )
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Navigation rail: the bar's place on a phone held sideways or on a wide window
// ------------------------------------------------------------------------------------------------

@Composable
private fun NavRail(
    selected: Int,
    recording: Boolean,
    warming: Boolean,
    onSelect: (Int) -> Unit,
    onRecord: () -> Unit,
    onYou: () -> Unit,
) {
    val p = pal
    val shape = RoundedCornerShape(32.dp)
    Box(
        Modifier.width(RailWidth).fillMaxHeight().windowInsetsPadding(WindowInsets.systemBars.only(WindowInsetsSides.Vertical)).padding(vertical = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        androidx.compose.foundation.layout.Column(
            Modifier
                .padding(start = 12.dp, end = 8.dp)
                .width(72.dp)
                .shadow(if (p.dark) 18.dp else 10.dp, shape, ambientColor = Color.Black, spotColor = Color.Black)
                .clip(shape)
                .background(if (p.dark) p.surface3.copy(alpha = 0.96f) else p.surface2.copy(alpha = 0.97f))
                .border(1.dp, p.line, shape)
                .verticalScroll(rememberScrollState())
                .padding(vertical = 6.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            BarItem(Icons.Rounded.GraphicEq, "Meetings", selected == 0, Modifier.fillMaxWidth()) { onSelect(0) }
            BarItem(Icons.Rounded.AutoStories, "Knowledge", selected == 1, Modifier.fillMaxWidth()) { onSelect(1) }
            RecordButton(recording = recording, warming = warming, onClick = onRecord, diameter = 60.dp, modifier = Modifier.padding(vertical = 4.dp))
            BarItem(Icons.Rounded.ChatBubbleOutline, "Ask", selected == 2, Modifier.fillMaxWidth()) { onSelect(2) }
            BarItem(Icons.Rounded.Person, "You", false, Modifier.fillMaxWidth()) { onYou() }
        }
    }
}

/**
 * The Meetings tab. On one pane it is the list. Side by side it is the list with the chosen meeting's
 * review next to it, so opening a meeting never covers the list.
 */
@Composable
private fun MeetingsHost(
    vm: AppVm,
    review: app.marginalia.MeetingDetail?,
    onRecord: () -> Unit,
    sideBySide: Boolean,
    backEnabled: Boolean,
    onCite: (app.marginalia.Source) -> Unit,
    onAsk: (String) -> Unit,
    onOpenHome: () -> Unit,
) {
    if (!sideBySide) {
        MeetingsScreen(vm, onRecord = onRecord, onOpenHome = onOpenHome)
        return
    }
    val p = pal
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val listW = (maxWidth * 0.38f).coerceIn(290.dp, 400.dp)
        Row(Modifier.fillMaxSize()) {
            Box(Modifier.width(listW).fillMaxHeight()) { MeetingsScreen(vm, onRecord = onRecord, selectedId = review?.meeting?.id, onOpenHome = onOpenHome) }
            Box(Modifier.width(1.dp).fillMaxHeight().background(p.line))
            androidx.compose.animation.AnimatedContent(
                targetState = review,
                // A different meeting fades in over the old one. The same meeting updating (notes arriving) changes in place.
                contentKey = { it?.meeting?.id },
                transitionSpec = { fadeIn(tween(200)) togetherWith fadeOut(tween(150)) },
                label = "review-pane",
                modifier = Modifier.weight(1f).fillMaxHeight(),
            ) { d ->
                if (d != null) ReviewScreen(vm, d, onCite = onCite, onAsk = onAsk, embedded = true, backEnabled = backEnabled)
                else NoMeetingChosen()
            }
        }
    }
}

@Composable
private fun NoMeetingChosen() {
    Box(Modifier.fillMaxSize().padding(40.dp), contentAlignment = Alignment.Center) {
        androidx.compose.foundation.layout.Column(horizontalAlignment = Alignment.CenterHorizontally) {
            BrandMark(56.dp)
            androidx.compose.foundation.layout.Spacer(Modifier.height(18.dp))
            Text("Pick a meeting", style = type.title)
            androidx.compose.foundation.layout.Spacer(Modifier.height(6.dp))
            Text("Its summary, notes and transcript open here.", style = type.callout, textAlign = androidx.compose.ui.text.style.TextAlign.Center)
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Mini recorder: a recording you have stepped away from
// ------------------------------------------------------------------------------------------------

@Composable
private fun MiniRecorder(vm: AppVm, onExpand: () -> Unit) {
    val p = pal
    val live by vm.live.collectAsState()
    val s = live ?: return
    val scope = rememberCoroutineScope()
    val lift = remember { Animatable(0f) }
    val haptic = LocalHapticFeedback.current
    Row(
        Modifier
            .widthIn(max = 560.dp)
            .padding(horizontal = 16.dp)
            .offset { IntOffset(0, lift.value.roundToInt()) }
            .fillMaxWidth()
            .height(56.dp)
            .shadow(10.dp, RoundedCornerShape(p.rMid))
            .clip(RoundedCornerShape(p.rMid))
            .background(p.surface4.takeIf { p.dark } ?: p.surface2)
            .border(1.dp, p.line, RoundedCornerShape(p.rMid))
            .draggable(
                orientation = Orientation.Vertical,
                state = rememberDraggableState { dy -> scope.launch { lift.snapTo((lift.value + dy).coerceAtMost(0f)) } },
                onDragStopped = {
                    if (lift.value < -60f) { haptic.performHapticFeedback(HapticFeedbackType.GestureEnd); onExpand() }
                    scope.launch { lift.animateTo(0f, Motion.bouncy()) }
                },
            )
            .pressable(pressedScale = 0.98f, onClick = onExpand)
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (s.recording) PulseDot(p.danger) else ThinkingDots()
        Box(Modifier.width(10.dp))
        if (s.recording) RollingTimer(formatTimer(s.secs), type.label.copy(fontFeatureSettings = "tnum"))
        else Text(s.wrap?.label ?: "Finishing up", style = type.label)
        Box(Modifier.weight(1f).height(28.dp).padding(horizontal = 12.dp)) {
            val level by RecorderBus.level.collectAsState()
            LiveWaveform(level = { level }, active = s.recording, bars = 28, modifier = Modifier.fillMaxSize())
        }
        Text("Open", style = type.label.copy(color = p.accentInk))
    }
}

fun formatTimer(secs: Long): String {
    val h = secs / 3600; val m = (secs / 60) % 60; val s = secs % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, s) else "%02d:%02d".format(m, s)
}

// ------------------------------------------------------------------------------------------------
// Toasts
// ------------------------------------------------------------------------------------------------

/**
 * One place for every toast: a pill centred under the status bar and the camera cutout, the same
 * on every screen including the live one. It slides down and fades in, and slides back up and out.
 */
@Composable
private fun ToastHost(vm: AppVm, modifier: Modifier = Modifier) {
    var current by remember { mutableStateOf<Toast?>(null) }
    var shown by remember { mutableStateOf<Toast?>(null) }
    var token by remember { mutableIntStateOf(0) }
    LaunchedEffect(Unit) {
        vm.toasts.collect { current = it; shown = it; token++ }
    }
    LaunchedEffect(token) {
        val t = current ?: return@LaunchedEffect
        delay(if (t.action != null) 6500L else (2800L + t.message.length * 25L).coerceAtMost(5500L))
        if (current === t) current = null
    }
    val still = reducedMotion()
    val outMs = if (still) 0 else 180
    BoxWithConstraints(
        modifier
            .fillMaxWidth()
            .windowInsetsPadding(WindowInsets.statusBars.union(WindowInsets.displayCutout).only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal))
            .padding(top = 4.dp),
        contentAlignment = Alignment.TopCenter,
    ) {
        val maxW = maxWidth
        AnimatedVisibility(
            visible = current != null,
            // A bubble that drops from the top edge, overshoots a little and settles.
            enter = if (still) fadeIn(tween(0)) else
                slideInVertically(spring(dampingRatio = 0.58f, stiffness = 340f)) { -it - 40 } +
                    scaleIn(spring(dampingRatio = 0.62f, stiffness = 380f), initialScale = 0.78f, transformOrigin = TransformOrigin(0.5f, 0f)) +
                    fadeIn(tween(140)),
            exit = if (still) fadeOut(tween(0)) else
                slideOutVertically(tween(outMs, easing = Motion.Emphasized)) { -it } +
                    scaleOut(tween(outMs), targetScale = 0.86f, transformOrigin = TransformOrigin(0.5f, 0f)) +
                    fadeOut(tween(outMs)),
        ) {
            val t = shown
            if (t != null) {
                // The padding gives the shadow room, since the enter and exit layer clips to its bounds.
                Box(Modifier.padding(start = 12.dp, end = 12.dp, top = 4.dp, bottom = 16.dp)) {
                    ToastView(
                        message = t.message, kind = t.kind, action = t.action,
                        onAction = { t.onAction?.invoke(); current = null },
                        onDismiss = { current = null },
                        modifier = Modifier.widthIn(max = minOf(maxW * 0.92f, 560.dp)),
                    )
                }
            }
        }
    }
}
