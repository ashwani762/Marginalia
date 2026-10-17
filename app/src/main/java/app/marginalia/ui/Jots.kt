// Jot cards, the swipe to keep gesture and the activity strip shown while recording.

package app.marginalia.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.AutoAwesome
import androidx.compose.material.icons.rounded.BookmarkAdd
import androidx.compose.material.icons.rounded.Bookmark
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import app.marginalia.Jot
import app.marginalia.formatClock
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

val jotKinds = listOf("note" to "Note", "action" to "Action", "decision" to "Decision", "question" to "Question")

@Composable
fun kindColor(kind: String): Color = when (kind) {
    "action" -> pal.accentInk
    "decision" -> pal.success
    "question" -> pal.warning
    "keep" -> pal.accent3
    else -> pal.textFaint
}

fun kindLabel(kind: String) = when (kind) {
    "action" -> "ACTION"
    "decision" -> "DECISION"
    "question" -> "QUESTION"
    "keep" -> "KEPT FROM TRANSCRIPT"
    else -> "NOTE"
}

/** A grey bar that shimmers while something is being written. */
@Composable
fun ShimmerLine(modifier: Modifier = Modifier, height: androidx.compose.ui.unit.Dp = 10.dp) {
    val p = pal
    val t = rememberInfiniteTransition(label = "shimmer")
    val x by t.animateFloat(-1f, 2f, infiniteRepeatable(tween(1300, easing = LinearEasing), RepeatMode.Restart), label = "x")
    Box(
        modifier.height(height).clip(CircleShape).background(
            Brush.linearGradient(
                listOf(p.surface4, p.lineStrong, p.surface4),
                start = Offset(x * 600f, 0f), end = Offset(x * 600f + 300f, 0f),
            ),
        ),
    )
}

/**
 * One jot. While Gemma is working the card shows what you wrote with a shimmer beneath it; when
 * the clean version arrives the text swaps in place, the card flashes once and you feel a tick.
 */
@Composable
fun JotCard(
    jot: Jot,
    modifier: Modifier = Modifier,
    onEdit: (() -> Unit)? = null,
    flat: Boolean = false,
) {
    val p = pal
    val shape = RoundedCornerShape(p.rMid)
    val haptic = LocalHapticFeedback.current
    var showRaw by remember { mutableStateOf(false) }
    val flash = remember { Animatable(0f) }
    var wasPending by remember { mutableStateOf(!jot.ready) }
    LaunchedEffect(jot.ready, jot.polished) {
        if (jot.ready && wasPending) {
            wasPending = false
            haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
            flash.snapTo(1f)
            flash.animateTo(0f, tween(1400, easing = Motion.Emphasized))
        } else if (!jot.ready) wasPending = true
    }
    val accent = kindColor(jot.kind)
    Row(
        modifier.fillMaxWidth().then(if (flat) Modifier else Modifier.padding(horizontal = 16.dp, vertical = 6.dp)).height(IntrinsicSize.Min)
            .clip(shape).background(androidx.compose.ui.graphics.lerp(p.surface3, p.tonal, flash.value)).border(1.dp, p.line, shape)
            .pressable(pressedScale = 0.99f, onLongClick = onEdit, onClick = { showRaw = !showRaw }),
    ) {
        Box(Modifier.width(4.dp).fillMaxHeight().background(accent))
        Column(Modifier.weight(1f).padding(start = 14.dp, end = 14.dp, top = 12.dp, bottom = 13.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    kindLabel(jot.kind) + (jot.t?.let { "  ·  ${formatClock(it)}" } ?: "") + (if (jot.updated) "  ·  Updated" else ""),
                    style = type.micro.copy(color = accent), modifier = Modifier.weight(1f),
                )
                AnimatedContent(jot.ready && jot.polished.isNotBlank() && jot.kind != "keep", transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "state") { done ->
                    if (done) Icon(Icons.Rounded.AutoAwesome, "Polished", tint = p.accentInk, modifier = Modifier.size(15.dp))
                    else if (!jot.ready) ThinkingDots(color = p.accentInk)
                    else Box(Modifier.size(15.dp))
                }
            }
            Spacer(Modifier.height(6.dp))
            AnimatedContent(
                targetState = jot.ready,
                transitionSpec = { (fadeIn(tween(320)) + slideInVertically(tween(320)) { it / 6 }) togetherWith fadeOut(tween(160)) },
                label = "jot-text",
            ) { ready ->
                if (!ready) {
                    Column {
                        Text(jot.raw, style = type.body.copy(color = p.textDim, fontStyle = FontStyle.Italic))
                        Spacer(Modifier.height(9.dp))
                        ShimmerLine(Modifier.fillMaxWidth(0.82f))
                        Spacer(Modifier.height(5.dp))
                        ShimmerLine(Modifier.fillMaxWidth(0.5f))
                        Spacer(Modifier.height(8.dp))
                        Text("Polishing with what was said", style = type.caption.copy(color = p.textFaint))
                    }
                } else {
                    Text(if (showRaw) jot.raw else jot.text, style = type.body)
                }
            }
            if (jot.ready && jot.polished.isNotBlank() && jot.polished != jot.raw && jot.kind != "keep") {
                Spacer(Modifier.height(6.dp))
                Text(if (showRaw) "Showing what you wrote. Tap for the clean version" else "Tap to see what you wrote", style = type.caption.copy(color = p.textFaint))
            }
        }
    }
}

/**
 * Drag a transcript line to the right to keep it as a note. Past the threshold you feel a tick,
 * and on release the line springs back while a "kept" jot appears in the timeline.
 */
@Composable
fun SwipeToKeep(onKeep: () -> Unit, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val p = pal
    val scope = rememberCoroutineScope()
    val offset = remember { Animatable(0f) }
    var width by remember { mutableFloatStateOf(1f) }
    var armed by remember { mutableStateOf(false) }
    val haptic = LocalHapticFeedback.current
    val threshold = { width * 0.28f }
    val progress = (offset.value / threshold()).coerceIn(0f, 1f)
    Box(modifier.onSizeChanged { width = it.width.toFloat().coerceAtLeast(1f) }) {
        Box(Modifier.matchParentSize().alpha(progress), contentAlignment = Alignment.CenterStart) {
            Row(Modifier.padding(start = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(if (armed) Icons.Rounded.Bookmark else Icons.Rounded.BookmarkAdd, null, tint = p.accent3, modifier = Modifier.size(22.dp).graphicsLayer { val s = 0.7f + 0.4f * progress; scaleX = s; scaleY = s })
                Spacer(Modifier.width(8.dp))
                Text(if (armed) "Release to keep" else "Keep as a note", style = type.caption.copy(color = p.accent3))
            }
        }
        Box(
            Modifier.offset { IntOffset(offset.value.roundToInt(), 0) }.pointerInput(Unit) {
                detectHorizontalDragGestures(
                    onDragEnd = {
                        scope.launch {
                            if (offset.value > threshold()) { haptic.performHapticFeedback(HapticFeedbackType.Confirm); onKeep() }
                            armed = false
                            offset.animateTo(0f, Motion.bouncy())
                        }
                    },
                    onDragCancel = { scope.launch { offset.animateTo(0f, Motion.bouncy()) } },
                ) { change, dx ->
                    change.consume()
                    val cur = offset.value
                    val next = (cur + dx * (if (cur > threshold()) 0.5f else 1f)).coerceIn(0f, width * 0.5f)
                    scope.launch { offset.snapTo(next) }
                    val a = next > threshold()
                    if (a != armed) { armed = a; haptic.performHapticFeedback(if (a) HapticFeedbackType.GestureThresholdActivate else HapticFeedbackType.SegmentTick) }
                }
            },
        ) { content() }
    }
}

/**
 * One calm line that says what the app is doing right now ("Polishing your note", "Looking up an
 * answer"), so nothing happens out of sight. It rests on "Listening" when there is no work.
 */
@Composable
fun ActivityStrip(items: List<String>, recording: Boolean, modifier: Modifier = Modifier, note: String? = null) {
    val p = pal
    val label = items.firstOrNull() ?: if (recording) "Listening" else "Working"
    val busy = items.isNotEmpty()
    // The three moving dots need room, the quiet green dot does not: the text sits right next to whichever is shown.
    val markWidth by androidx.compose.animation.core.animateDpAsState(if (busy) 44.dp else 17.dp, tween(220), label = "mark")
    Column(modifier.fillMaxWidth()) {
    Row(Modifier.fillMaxWidth().height(34.dp).padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(width = markWidth, height = 12.dp), contentAlignment = Alignment.CenterStart) {
            if (busy) ThinkingDots(color = p.accentInk) else Box(Modifier.size(7.dp).clip(CircleShape).background(p.success.copy(alpha = 0.8f)))
        }
        // The line fades and drifts in place; it must never be clipped to its own box (descenders!).
        AnimatedContent(
            targetState = label,
            transitionSpec = {
                ((fadeIn(tween(220)) + slideInVertically(tween(220)) { it / 3 }) togetherWith (fadeOut(tween(140)) + slideOutVertically(tween(140)) { -it / 3 }))
                    .using(androidx.compose.animation.SizeTransform(clip = false))
            },
            contentAlignment = Alignment.CenterStart,
            label = "activity",
        ) { l -> Text(l, style = type.caption.copy(color = if (busy) p.text else p.textFaint, lineHeight = 20.sp()), maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis) }
        if (items.size > 1) Text("  +${items.size - 1}", style = type.caption.copy(color = p.textFaint))
    }
    // A line about what a compute node is doing (checking the captions) sits under the label, at the label's own left edge.
    androidx.compose.animation.AnimatedVisibility(visible = note != null, enter = fadeIn(tween(250)) + expandVertically(), exit = fadeOut(tween(200)) + shrinkVertically()) {
        Text(
            note.orEmpty(), style = type.caption.copy(color = p.textFaint), maxLines = 1,
            overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            modifier = Modifier.fillMaxWidth().padding(start = 20.dp + markWidth, end = 20.dp, bottom = 4.dp, top = 0.dp),
        )
    }
    }
}

private fun Int.sp() = androidx.compose.ui.unit.TextUnit(this.toFloat(), androidx.compose.ui.unit.TextUnitType.Sp)
