// Shared building blocks: press feedback, chips, buttons, toasts and similar.

@file:OptIn(ExperimentalFoundationApi::class)

package app.marginalia.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.Info
import androidx.compose.material.icons.rounded.Warning
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.dismiss
import androidx.compose.ui.semantics.onClick
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.foundation.layout.heightIn
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.graphics.vector.ImageVector
import kotlinx.coroutines.launch
import kotlin.math.min
import kotlin.math.roundToInt

/** Press feedback that feels physical: the control sinks under the finger and springs back. */
fun Modifier.pressable(
    enabled: Boolean = true,
    pressedScale: Float = 0.96f,
    haptic: HapticFeedbackType? = null,
    onLongClick: (() -> Unit)? = null,
    onClick: () -> Unit,
): Modifier = composed {
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val s by animateFloatAsState(if (pressed && enabled) pressedScale else 1f, Motion.snappy(), label = "press")
    val h = LocalHapticFeedback.current
    this
        .graphicsLayer { scaleX = s; scaleY = s }
        .combinedClickable(
            interactionSource = source, indication = null, enabled = enabled,
            onLongClick = onLongClick?.let { lc -> { h.performHapticFeedback(HapticFeedbackType.LongPress); lc() } },
            onClick = { if (haptic != null) h.performHapticFeedback(haptic); onClick() },
        )
}

@Composable
fun Chip(
    text: String,
    modifier: Modifier = Modifier,
    selected: Boolean = false,
    icon: ImageVector? = null,
    onClick: () -> Unit,
) {
    val p = pal
    val bg = if (selected) p.tonal else p.surface3
    val fg = if (selected) p.accentInk else p.textDim
    Row(
        modifier
            .height(36.dp)
            .clip(CircleShape)
            .background(bg)
            .border(1.dp, if (selected) p.accentInk.copy(alpha = 0.4f) else p.line, CircleShape)
            .pressable(onClick = onClick)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) {
            Icon(icon, null, tint = fg, modifier = Modifier.size(16.dp))
            Box(Modifier.width(6.dp))
        }
        Text(text, style = type.label.copy(color = fg), maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
fun SectionLabel(text: String, modifier: Modifier = Modifier, flush: Boolean = false) {
    // Lists give the label its own side margin. Inside a sheet the content already has one, so it sits flush.
    Text(text.uppercase(), style = type.micro, modifier = modifier.padding(horizontal = if (flush) 0.dp else 20.dp, vertical = 8.dp))
}

/** Round icon-only button with a 48 dp touch target. */
@Composable
fun IconAction(
    icon: ImageVector,
    description: String,
    modifier: Modifier = Modifier,
    tint: Color = pal.textDim,
    background: Color = Color.Transparent,
    onClick: () -> Unit,
) {
    Box(
        modifier
            .size(48.dp)
            .clip(CircleShape)
            .background(background)
            .pressable(pressedScale = 0.88f, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { Icon(icon, description, tint = tint, modifier = Modifier.size(24.dp)) }
}

@Composable
fun PrimaryButton(
    text: String,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    enabled: Boolean = true,
    onClick: () -> Unit,
) {
    val p = pal
    Row(
        modifier
            .height(52.dp)
            .clip(RoundedCornerShape(p.rMid))
            .background(p.accentBrush)
            .alpha(if (enabled) 1f else 0.45f)
            .pressable(enabled = enabled, haptic = HapticFeedbackType.ContextClick, onClick = onClick)
            .padding(horizontal = 22.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = androidx.compose.foundation.layout.Arrangement.Center,
    ) {
        if (icon != null) {
            Icon(icon, null, tint = p.onAccent, modifier = Modifier.size(20.dp))
            Box(Modifier.width(8.dp))
        }
        Text(text, style = type.headline.copy(color = p.onAccent))
    }
}

@Composable
fun TonalButton(text: String, modifier: Modifier = Modifier, icon: ImageVector? = null, onClick: () -> Unit) {
    val p = pal
    Row(
        modifier
            .height(44.dp)
            .clip(RoundedCornerShape(p.rSmall))
            .background(p.tonal)
            .pressable(onClick = onClick)
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) {
            Icon(icon, null, tint = p.accentInk, modifier = Modifier.size(18.dp))
            Box(Modifier.width(8.dp))
        }
        Text(text, style = type.label.copy(color = p.accentInk))
    }
}

// ---------------------------------------------------------------------------------------------
// Swipe to delete
// ---------------------------------------------------------------------------------------------

/**
 * A row that follows the finger to the left. Release past a third of the way and it flies
 * off and [onDelete] fires; release earlier and it springs back. A tick of haptic feedback
 * marks the point of no return.
 */
@Composable
fun SwipeToDelete(
    onDelete: () -> Unit,
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(pal.rMid),
    content: @Composable () -> Unit,
) {
    val p = pal
    val scope = rememberCoroutineScope()
    val offset = remember { Animatable(0f) }
    var width by remember { mutableFloatStateOf(1f) }
    val haptic = LocalHapticFeedback.current
    var armed by remember { mutableStateOf(false) }
    val threshold = { width * 0.33f }
    val progress = (-offset.value / threshold()).coerceIn(0f, 1f)

    Box(modifier.onSizeChanged { width = it.width.toFloat().coerceAtLeast(1f) }) {
        // What the swipe reveals.
        Box(
            Modifier
                .matchParentSize()
                .clip(shape)
                .background(p.dangerFill.copy(alpha = 0.18f + 0.82f * progress)),
            contentAlignment = Alignment.CenterEnd,
        ) {
            Icon(
                Icons.Rounded.DeleteOutline, "Delete", tint = Color.White,
                modifier = Modifier.padding(end = 24.dp).size(26.dp).graphicsLayer {
                    val s = 0.6f + 0.4f * progress + (if (armed) 0.15f else 0f)
                    scaleX = s; scaleY = s; alpha = progress
                },
            )
        }
        Box(
            Modifier
                .offset { IntOffset(offset.value.roundToInt(), 0) }
                .pointerInput(Unit) {
                    detectHorizontalDragGestures(
                        onDragStart = { armed = false },
                        onDragEnd = {
                            scope.launch {
                                if (offset.value < -threshold()) {
                                    offset.animateTo(-width, Motion.fade(160))
                                    onDelete()
                                    offset.snapTo(0f)
                                } else {
                                    offset.animateTo(0f, Motion.bouncy())
                                }
                                armed = false
                            }
                        },
                        onDragCancel = { scope.launch { offset.animateTo(0f, Motion.bouncy()) } },
                    ) { change, dx ->
                        change.consume()
                        // Rubber band once past the threshold; never drags to the right.
                        val cur = offset.value
                        val resist = if (cur < -threshold()) 0.55f else 1f
                        val next = (cur + dx * resist).coerceIn(-width, 0f)
                        scope.launch { offset.snapTo(next) }
                        val nowArmed = next < -threshold()
                        if (nowArmed != armed) {
                            armed = nowArmed
                            haptic.performHapticFeedback(if (nowArmed) HapticFeedbackType.GestureThresholdActivate else HapticFeedbackType.SegmentTick)
                        }
                    }
                },
        ) { content() }
    }
}

// ---------------------------------------------------------------------------------------------
// Toasts
// ---------------------------------------------------------------------------------------------

/** True when the person has turned animations off in the system settings. */
@Composable
fun reducedMotion(): Boolean {
    val ctx = androidx.compose.ui.platform.LocalContext.current
    return remember {
        try {
            android.provider.Settings.Global.getFloat(ctx.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
        } catch (_: Exception) { false }
    }
}

/**
 * A compact pill: status icon, one or two lines of text, and an optional action (Undo, Open).
 * It is announced politely to screen readers. Swipe it up to dismiss, tap it to close it.
 * Placement and timing belong to the host.
 */
@Composable
fun ToastView(
    message: String,
    kind: String,
    action: String?,
    onAction: () -> Unit,
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val p = pal
    val scope = rememberCoroutineScope()
    val dy = remember(message) { Animatable(0f) }
    val tint = when (kind) {
        "success" -> p.success
        "warning" -> p.warning
        "error" -> p.danger
        else -> p.accentInk
    }
    val icon = when (kind) {
        "success" -> Icons.Rounded.CheckCircle
        "warning" -> Icons.Rounded.Warning
        "error" -> Icons.Rounded.ErrorOutline
        else -> Icons.Rounded.Info
    }
    val dismissPx = with(LocalDensity.current) { 28.dp.toPx() }
    val shape = RoundedCornerShape(24.dp)
    Row(
        modifier
            .offset { IntOffset(0, dy.value.roundToInt()) }
            .alpha((1f + dy.value / (dismissPx * 4f)).coerceIn(0f, 1f))
            .pointerInput(message) {
                detectVerticalDragGestures(
                    onDragEnd = { scope.launch { if (dy.value < -dismissPx) onDismiss() else dy.animateTo(0f, Motion.snappy()) } },
                    onDragCancel = { scope.launch { dy.animateTo(0f, Motion.snappy()) } },
                ) { c, d -> c.consume(); scope.launch { dy.snapTo((dy.value + d).coerceIn(-dismissPx * 3f, 10f)) } }
            }
            .shadow(8.dp, shape, ambientColor = Color.Black, spotColor = Color.Black)
            .clip(shape)
            .background(if (p.dark) p.surface4 else p.surface2)
            .border(1.dp, p.lineStrong, shape)
            .pressable(pressedScale = 0.985f, onClick = onDismiss)
            .heightIn(min = 48.dp)
            .androidSemantics(onDismiss),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, null, tint = tint, modifier = Modifier.padding(start = 14.dp).size(20.dp))
        Text(
            message,
            style = type.caption.copy(color = p.text, fontSize = 14.sp, lineHeight = 19.sp),
            modifier = Modifier
                .weight(1f, fill = false)
                .padding(start = 10.dp, end = if (action != null) 4.dp else 18.dp, top = 8.dp, bottom = 8.dp),
            maxLines = 2, overflow = TextOverflow.Ellipsis,
        )
        if (action != null) {
            Box(
                Modifier
                    .heightIn(min = 48.dp)
                    .pressable(onClick = onAction)
                    .padding(start = 10.dp, end = 18.dp)
                    .androidButton(),
                contentAlignment = Alignment.Center,
            ) {
                Text(action, style = type.label.copy(color = p.accentInk, fontSize = 14.sp), maxLines = 1)
            }
        }
    }
}

private fun Modifier.androidSemantics(onDismiss: () -> Unit): Modifier = this.semantics {
    liveRegion = androidx.compose.ui.semantics.LiveRegionMode.Polite
    dismiss { onDismiss(); true }
}

private fun Modifier.androidButton(): Modifier = this.semantics { role = androidx.compose.ui.semantics.Role.Button }

/**
 * Tap opens. Press and hold for a moment shows a peek ([onPeek] true) that goes away on release
 * ([onPeek] false). Screen reader users get open and preview as actions.
 */
fun Modifier.peekable(label: String, onPeek: (Boolean) -> Unit, open: () -> Unit): Modifier = composed {
    val h = LocalHapticFeedback.current
    this
        .pointerInput(Unit) {
            awaitEachGesture {
                awaitFirstDown()
                var timedOut = true
                val up = withTimeoutOrNull(viewConfiguration.longPressTimeoutMillis) {
                    val r = waitForUpOrCancellation()
                    timedOut = false
                    r
                }
                if (up != null) { up.consume(); open() }
                else if (timedOut) {
                    h.performHapticFeedback(HapticFeedbackType.LongPress)
                    onPeek(true)
                    waitForUpOrCancellation()
                    onPeek(false)
                }
            }
        }
        .semantics {
            role = androidx.compose.ui.semantics.Role.Button
            contentDescription = label
            onClick("Open") { open(); true }
            customActions = listOf(androidx.compose.ui.semantics.CustomAccessibilityAction("Preview") { onPeek(true); true })
        }
}


/**
 * The grab handle at the top of every sheet. A sheet is a window of its own, so it also tells the system
 * whether the status bar icons should be dark (on the paper theme) or light, which the page behind cannot do.
 */
@Composable
fun SheetHandle() {
    val p = pal
    val view = androidx.compose.ui.platform.LocalView.current
    androidx.compose.runtime.SideEffect {
        val window = (view.parent as? androidx.compose.ui.window.DialogWindowProvider)?.window
        if (window != null) androidx.core.view.WindowCompat.getInsetsController(window, view).isAppearanceLightStatusBars = !p.dark
    }
    Box(Modifier.padding(top = 10.dp, bottom = 6.dp).width(40.dp).height(4.dp).clip(CircleShape).background(p.lineStrong))
}
