package app.marginalia.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * The size of the window, in the dp the system reports (not the proportional dp the layout draws in), so a
 * phone keeps the same class whichever way it is held. Window size classes in spirit:
 *
 *  - [shortH]: under 480 dp tall. A phone on its side. One pane, a navigation rail, tighter headers.
 *  - [twoPane]: 600 dp wide and 480 dp tall or more. A tablet or an unfolded foldable. List and detail side by side.
 *  - [rail]: the navigation moves to the side when the window is short or 840 dp wide or more. Otherwise the floating bar stays.
 *  - [split]: wider than tall (and at least 640 dp wide). The live screen puts the controls beside the transcript.
 */
@Immutable
class Win(val w: Int, val h: Int) {
    val shortH: Boolean get() = h < 480
    val twoPane: Boolean get() = w >= 600 && h >= 480
    val rail: Boolean get() = shortH || w >= 840
    val split: Boolean get() = w >= 640 && w > h
}

val LocalWin = staticCompositionLocalOf { Win(411, 900) }

val win: Win @Composable get() = LocalWin.current

@Composable
fun rememberWin(): Win {
    val c = LocalConfiguration.current
    return androidx.compose.runtime.remember(c.screenWidthDp, c.screenHeightDp) { Win(c.screenWidthDp, c.screenHeightDp) }
}

/** Width of the navigation rail, including its margin. */
val RailWidth = 92.dp

/** Longest comfortable line for reading text. */
val ReadingWidth = 680.dp

/**
 * Space a scrolling list leaves at its end so the last row clears the floating bar. With a rail on the side
 * there is no bar underneath, only the system navigation bar.
 */
@Composable
fun bottomClearance(): Dp {
    val nav = with(LocalDensity.current) { WindowInsets.navigationBars.getBottom(this).toDp() }
    return if (win.rail) 24.dp + nav else 170.dp + MiniBar.height
}

/** Whether the small "recording" card is shown above the floating bar, so screens can leave room for it. */
object MiniBar {
    var visible by androidx.compose.runtime.mutableStateOf(false)
    /** How much taller the stack above the screen gets while the card is up. */
    val height: Dp get() = if (visible) 76.dp else 0.dp
}

/** Centres a screen in the window and stops it growing wider than [max], so lines stay readable. */
@Composable
fun CappedPane(max: Dp = ReadingWidth, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
        Box(Modifier.widthIn(max = max).fillMaxSize()) { content() }
    }
}
