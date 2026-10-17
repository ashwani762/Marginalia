// Small animated visuals: brand mark, waveform, timer, pulse and thinking dots.

package app.marginalia.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.animation.togetherWith
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlin.math.PI
import kotlin.math.exp
import kotlin.math.max
import kotlin.math.sin

/** The mark, same as the launcher icon: a note card with a margin rule, three lines of notes and a spark. */
@Composable
fun BrandMark(size: Dp, modifier: Modifier = Modifier) {
    val p = pal
    Canvas(modifier.size(size)) {
        val s = this.size.minDimension
        val u = s / 108f
        drawRoundRect(
            brush = Brush.linearGradient(listOf(Color(0xFF8C7CF9), Color(0xFF6A58F0), Color(0xFF3B2CB8)), Offset.Zero, Offset(s, s)),
            cornerRadius = CornerRadius(s * 0.26f),
        )
        drawRoundRect(
            brush = Brush.radialGradient(listOf(Color.White.copy(alpha = 0.33f), Color.Transparent), Offset(26 * u, 20 * u), 78 * u),
            cornerRadius = CornerRadius(s * 0.26f),
        )
        drawRoundRect(Color.White.copy(alpha = 0.18f), Offset(31 * u, 29 * u), Size(46 * u, 50 * u), CornerRadius(10 * u))
        drawLine(Color.White.copy(alpha = 0.7f), Offset(41.5f * u, 38 * u), Offset(41.5f * u, 70 * u), 4f * u, StrokeCap.Round)
        val w = 4.4f * u
        drawLine(Color.White, Offset(50.5f * u, 42 * u), Offset(67 * u, 42 * u), w, StrokeCap.Round)
        drawLine(Color.White, Offset(50.5f * u, 54 * u), Offset(62 * u, 54 * u), w, StrokeCap.Round)
        drawLine(Color.White, Offset(50.5f * u, 66 * u), Offset(65 * u, 66 * u), w, StrokeCap.Round)
        val spark = androidx.compose.ui.graphics.Path().apply {
            moveTo(72 * u, 23 * u)
            cubicTo(72.7f * u, 27.8f * u, 74.4f * u, 29.5f * u, 79.2f * u, 30.2f * u)
            cubicTo(74.4f * u, 30.9f * u, 72.7f * u, 32.6f * u, 72 * u, 37.4f * u)
            cubicTo(71.3f * u, 32.6f * u, 69.6f * u, 30.9f * u, 64.8f * u, 30.2f * u)
            cubicTo(69.6f * u, 29.5f * u, 71.3f * u, 27.8f * u, 72 * u, 23 * u)
            close()
        }
        drawPath(spark, Color(0xFF22D3EE))
    }
}

/**
 * Scrolling input waveform. The newest sample enters on the right; attack is instant and
 * release is slow, so speech reads as a living thing instead of a jittery bar chart.
 */
@Composable
fun LiveWaveform(
    level: () -> Float,
    active: Boolean,
    modifier: Modifier = Modifier,
    bars: Int = 48,
    color: Color = pal.accentInk,
) {
    val samples = remember(bars) { FloatArray(bars) }
    var tick by remember { mutableIntStateOf(0) }
    val levelNow by rememberUpdatedState(level)
    val activeNow by rememberUpdatedState(active)
    LaunchedEffect(Unit) {
        var smooth = 0f
        var last = 0L
        var prev = 0L
        while (true) {
            androidx.compose.runtime.withFrameNanos { now ->
                if (last == 0L) { last = now; prev = now }
                if (now - last >= 33_000_000L) { // ~30 steps/s
                    last = now
                    val target = if (activeNow) levelNow() else 0f
                    // Fast attack (about 60 ms) and slow release (about 250 ms): a quiet room stays still,
                    // and speech lifts the bars at once and lets them settle.
                    val tau = if (target > smooth) 60f else 250f
                    smooth += (target - smooth) * (1f - exp(-(1000f * dtSec(now, prev)) / tau))
                    prev = now
                    val idle = if (activeNow) 0.03f else 0.02f
                    System.arraycopy(samples, 1, samples, 0, bars - 1)
                    samples[bars - 1] = max(smooth, idle)
                    tick++
                }
            }
        }
    }
    // On the dark surface the bars glow, so a faint tail reads well. On the paper surface they need more
    // weight: a darker base, a deeper spark colour, a firmer tail and slightly fatter bars.
    val dark = pal.dark
    val base = if (dark) color else lerp(color, pal.text, 0.45f)
    val sparkColor = if (dark) Color(0xFF22D3EE) else Color(0xFF0A6C80)
    val floorAlpha = if (dark) 0.12f else 0.32f
    Canvas(modifier) {
        tick // read to invalidate
        val gap = size.width / bars
        val bw = gap * (if (dark) 0.52f else 0.62f)
        val mid = size.height / 2f
        for (i in 0 until bars) {
            val age = i / (bars - 1f)
            val v = samples[i]
            val h = max(bw, v * size.height * 0.95f)
            // Louder bars warm toward cyan, the spark of the brand mark.
            val k = ((v - 0.1f) / 0.18f).coerceIn(0f, 1f)
            val spark = k * k * (3f - 2f * k)
            drawRoundRect(
                color = lerp(base, sparkColor, spark).copy(alpha = floorAlpha + (1f - floorAlpha) * age * age),
                topLeft = Offset(i * gap + (gap - bw) / 2f, mid - h / 2f),
                size = Size(bw, h),
                cornerRadius = CornerRadius(bw / 2f),
            )
        }
    }
}

private fun dtSec(now: Long, prev: Long) = ((now - prev).coerceAtLeast(1_000_000L)) / 1_000_000_000f

/**
 * A running clock whose digits roll one by one, so the width never jumps. Each character keeps its
 * place counted from the right, so the minutes do not move when an hour appears.
 */
@Composable
fun RollingTimer(text: String, style: androidx.compose.ui.text.TextStyle, modifier: Modifier = Modifier) {
    val still = reducedMotion()
    val ms = if (still) 0 else 180
    androidx.compose.foundation.layout.Row(modifier.androidx_clearAndDescribe(text)) {
        text.forEachIndexed { i, ch ->
            androidx.compose.runtime.key(text.length - i) {
                androidx.compose.animation.AnimatedContent(
                    targetState = ch,
                    transitionSpec = {
                        (androidx.compose.animation.slideInVertically(tween(ms, easing = Motion.Emphasized)) { it } + androidx.compose.animation.fadeIn(tween(ms))) togetherWith
                            (androidx.compose.animation.slideOutVertically(tween(ms, easing = Motion.Emphasized)) { -it } + androidx.compose.animation.fadeOut(tween(ms / 2)))
                    },
                    label = "digit",
                ) { c -> androidx.compose.material3.Text(c.toString(), style = style) }
            }
        }
    }
}

private fun Modifier.androidx_clearAndDescribe(text: String): Modifier =
    this.clearAndSetSemantics { contentDescription = text }

/** A small pulsing dot: "recording" or "working". */
@Composable
fun PulseDot(color: Color, size: Dp = 10.dp, modifier: Modifier = Modifier) {
    val t = rememberInfiniteTransition(label = "pulse")
    val s by t.animateFloat(0.85f, 1.35f, infiniteRepeatable(tween(900, easing = LinearEasing), RepeatMode.Reverse), label = "s")
    val a by t.animateFloat(1f, 0.35f, infiniteRepeatable(tween(900, easing = LinearEasing), RepeatMode.Reverse), label = "a")
    Canvas(modifier.size(size * 1.6f)) {
        val r = this.size.minDimension / 2f
        drawCircle(color.copy(alpha = 0.25f * a), radius = r * s * 0.9f)
        drawCircle(color, radius = r * 0.55f)
    }
}

/** Three dots that breathe in sequence while the model is thinking. */
@Composable
fun ThinkingDots(modifier: Modifier = Modifier, color: Color = pal.textFaint) {
    val t = rememberInfiniteTransition(label = "dots")
    val phase by t.animateFloat(0f, 3f, infiniteRepeatable(tween(1200, easing = LinearEasing)), label = "p")
    Canvas(modifier.size(width = 36.dp, height = 12.dp)) {
        val r = size.height / 2f
        for (i in 0..2) {
            val x = (phase - i + 3f) % 3f
            val k = if (x < 1f) sin(x * PI.toFloat()) else 0f
            drawCircle(color.copy(alpha = 0.35f + 0.65f * k), radius = r * (0.6f + 0.4f * k), center = Offset(r + i * (size.width - 2 * r) / 2f, r))
        }
    }
}

/** Soft aurora behind the live screen; its glow follows the voice. */
@Composable
fun Aurora(level: () -> Float, modifier: Modifier = Modifier) {
    val p = pal
    val t = rememberInfiniteTransition(label = "aurora")
    val drift by t.animateFloat(0f, 1f, infiniteRepeatable(tween(14000, easing = LinearEasing), RepeatMode.Reverse), label = "d")
    val glow = remember { Animatable(0f) }
    LaunchedEffect(Unit) {
        while (true) {
            androidx.compose.runtime.withFrameNanos { }
            val target = level().coerceIn(0f, 1f)
            glow.snapTo(glow.value + (target - glow.value) * 0.08f)
        }
    }
    Canvas(modifier) {
        val g = glow.value
        val base = if (p.dark) 0.28f else 0.16f
        drawCircle(
            brush = Brush.radialGradient(
                listOf(p.accent.copy(alpha = base + 0.25f * g), Color.Transparent),
                center = Offset(size.width * (0.2f + 0.6f * drift), size.height * 0.12f),
                radius = size.width * (0.85f + 0.25f * g),
            ),
            radius = size.width * 1.1f,
            center = Offset(size.width * (0.2f + 0.6f * drift), size.height * 0.12f),
        )
        drawCircle(
            brush = Brush.radialGradient(
                listOf(p.accent3.copy(alpha = (base * 0.55f) + 0.12f * g), Color.Transparent),
                center = Offset(size.width * (0.9f - 0.6f * drift), size.height * 0.7f),
                radius = size.width * 0.8f,
            ),
            radius = size.width,
            center = Offset(size.width * (0.9f - 0.6f * drift), size.height * 0.7f),
        )
    }
}
