// Large screen titles that scroll away with the content.

package app.marginalia.ui

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ArrowDownward
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp

/** Big title that scrolls away with the content. Place it as the first item of a LazyColumn. */
@Composable
fun LargeTitle(title: String, subtitle: String?, modifier: Modifier = Modifier, showSubtitle: Boolean = true) {
    // On a phone held sideways there is no height to spare, so the title sits higher.
    val top = if (win.shortH) 18.dp else 56.dp
    Column(modifier.fillMaxWidth().statusBarsPadding().padding(top = top, start = 20.dp, end = 20.dp, bottom = if (showSubtitle) 12.dp else 2.dp)) {
        Text(title, style = type.largeTitle)
        if (showSubtitle && subtitle != null) Text(subtitle, style = type.callout, modifier = Modifier.padding(top = 2.dp))
    }
}

/**
 * The line under a large title: a quiet caption on the left and the screen's one action on the
 * right, level with it. Search lives here, so the field opens right where the icon was.
 */
@Composable
fun SubtitleRow(caption: String?, modifier: Modifier = Modifier, action: @Composable () -> Unit = {}) {
    Row(modifier.fillMaxWidth().height(48.dp).padding(start = 20.dp, end = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(caption.orEmpty(), style = type.callout, modifier = Modifier.weight(1f), maxLines = 1)
        action()
    }
}

/**
 * Fixed bar over the list: transparent at rest, and as the large title scrolls away a small
 * centred title fades in over a frosted surface. Actions stay put.
 */
@Composable
fun TopBarOverlay(
    title: String,
    list: LazyListState,
    modifier: Modifier = Modifier,
    actions: @Composable RowScope.() -> Unit = {},
) {
    val p = pal
    val titlePx = with(LocalDensity.current) { 64.dp.toPx() }
    val progress by remember(list) {
        derivedStateOf {
            if (list.firstVisibleItemIndex > 0) 1f else (list.firstVisibleItemScrollOffset / titlePx).coerceIn(0f, 1f)
        }
    }
    Box(modifier.fillMaxWidth()) {
        Box(Modifier.matchParentSize().alpha(progress).background(p.surface.copy(alpha = 0.94f)))
        Box(Modifier.matchParentSize().alpha(progress)) {
            Box(Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(1.dp).background(p.line))
        }
        Row(Modifier.statusBarsPadding().height(56.dp).fillMaxWidth().padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Spacer(Modifier.weight(1f))
            // At rest the action sits under the large title; it moves up here once that scrolls away.
            if (progress > 0.6f) Row(Modifier.alpha(((progress - 0.6f) / 0.4f).coerceIn(0f, 1f))) { actions() }
        }
        Text(
            title,
            style = type.headline,
            modifier = Modifier.align(Alignment.TopCenter).statusBarsPadding().padding(top = 15.dp).alpha(progress),
        )
    }
}

@Composable
fun SearchField(
    value: String,
    onChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    autoFocus: Boolean = false,
    onSearch: (() -> Unit)? = null,
    onClose: (() -> Unit)? = null,
) {
    val p = pal
    val focus = remember { FocusRequester() }
    androidx.compose.runtime.LaunchedEffect(autoFocus) { if (autoFocus) runCatching { focus.requestFocus() } }
    Row(
        modifier.fillMaxWidth().height(48.dp).clip(RoundedCornerShape(p.rMid)).background(p.surface3).border(1.dp, p.line, RoundedCornerShape(p.rMid)).padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Rounded.Search, null, tint = p.textFaint, modifier = Modifier.size(20.dp))
        Spacer(Modifier.width(10.dp))
        Box(Modifier.weight(1f)) {
            if (value.isEmpty()) Text(placeholder, style = type.body.copy(color = p.textFaint))
            BasicTextField(
                value = value,
                onValueChange = onChange,
                singleLine = true,
                textStyle = type.body,
                cursorBrush = SolidColor(p.accentInk),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { onSearch?.invoke() }),
                modifier = Modifier.fillMaxWidth().focusRequester(focus),
            )
        }
        if (value.isNotEmpty() || onClose != null) {
            Box(Modifier.size(32.dp).clip(CircleShape).pressable(pressedScale = 0.85f) { if (value.isNotEmpty()) onChange("") else onClose?.invoke() }, contentAlignment = Alignment.Center) {
                Icon(Icons.Rounded.Close, if (value.isNotEmpty()) "Clear" else "Close search", tint = p.textFaint, modifier = Modifier.size(18.dp))
            }
        }
    }
}

/** A friendly blank slate. Point at the control that fixes it with [PointerArrow] from the screen. */
@Composable
fun EmptyState(title: String, body: String, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth().padding(horizontal = 40.dp, vertical = 36.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        BrandMark(60.dp)
        Spacer(Modifier.height(20.dp))
        Text(title, style = type.title, textAlign = TextAlign.Center)
        Spacer(Modifier.height(8.dp))
        Text(body, style = type.callout, textAlign = TextAlign.Center)
    }
}

/**
 * A small bobbing arrow that sits just above a control in the floating layer and points down at it.
 * Place it in a full-screen Box, aligned like the control, with [bottomOffset] from the bottom edge
 * (above the navigation bar) to the control's top edge.
 */
@Composable
fun PointerArrow(modifier: Modifier = Modifier) {
    val still = reducedMotion()
    val bob = rememberInfiniteTransition(label = "bob")
    val dy by bob.animateFloat(0f, 8f, infiniteRepeatable(tween(900), RepeatMode.Reverse), label = "dy")
    Icon(
        Icons.Rounded.ArrowDownward, null, tint = pal.accentInk,
        modifier = modifier.size(32.dp).offset { androidx.compose.ui.unit.IntOffset(0, if (still) 0 else dy.dp.roundToPx()) },
    )
}
