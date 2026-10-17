// Colours, type and spacing for the light and dark looks.

package app.marginalia.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.marginalia.R

/**
 * Marginalia's two moods: Graphite (deep blue-black with a violet
 * accent) and Paper (warm ivory with ink). Every text role clears 4.5:1 on every surface.
 */
@Immutable
class Pal(
    val dark: Boolean,
    val accent: Color,       // fill behind a white label
    val accentInk: Color,    // accent used as text or a glyph
    val accent3: Color,
    val surface: Color,
    val surface2: Color,
    val surface3: Color,
    val surface4: Color,
    val line: Color,
    val lineStrong: Color,
    val text: Color,
    val textDim: Color,
    val textFaint: Color,
    val success: Color,
    val warning: Color,
    val danger: Color,
    val dangerFill: Color,
    val onAccent: Color,
    val glow: Color,
) {
    val tonal: Color get() = accentInk.copy(alpha = if (dark) 0.16f else 0.12f)
    val scrim: Color get() = Color.Black.copy(alpha = if (dark) 0.55f else 0.35f)
    val accentBrush: Brush
        get() = if (dark) Brush.linearGradient(listOf(Color(0xFF6A58F0), Color(0xFF8B6CF6))) else Brush.linearGradient(listOf(accent, accent))
    /** Corner radii: Graphite is soft, Paper is crisp. */
    val rSmall get() = if (dark) 12.dp else 6.dp
    val rMid get() = if (dark) 18.dp else 10.dp
    val rLarge get() = if (dark) 28.dp else 14.dp
}

private fun c(hex: Long) = Color(0xFF000000 or hex)

val Graphite = Pal(
    dark = true,
    accent = c(0x6A58F0), accentInk = c(0xA78BFA), accent3 = c(0x22D3EE),
    surface = c(0x080B12), surface2 = c(0x0D1119), surface3 = c(0x131826), surface4 = c(0x1A2130),
    line = Color.White.copy(alpha = 0.08f), lineStrong = Color.White.copy(alpha = 0.16f),
    text = c(0xEDF0F7), textDim = c(0x9AA4B8), textFaint = c(0x8794AB),
    success = c(0x34D399), warning = c(0xFBBF24), danger = c(0xF87171), dangerFill = c(0xD33A3A),
    onAccent = Color.White, glow = c(0x7C6CF6).copy(alpha = 0.5f),
)

val Paper = Pal(
    dark = false,
    accent = c(0x151413), accentInk = c(0x9C442D), accent3 = c(0x1F3FBF),
    surface = c(0xF6F3EE), surface2 = c(0xFBFAF7), surface3 = c(0xEDE7DD), surface4 = c(0xE4DCCF),
    line = c(0xDAD2C5), lineStrong = c(0xC9BFAF),
    text = c(0x151413), textDim = c(0x4A4642), textFaint = c(0x645E55),
    success = c(0x55624A), warning = c(0x80590E), danger = c(0xA23B2A), dangerFill = c(0xA23B2A),
    onAccent = Color.White, glow = c(0xB4533A).copy(alpha = 0.18f),
)

val LocalPal = staticCompositionLocalOf { Graphite }

/** Inter, as a variable font so any weight between 100 and 900 is available. */
@OptIn(ExperimentalTextApi::class)
private fun inter(weight: Int) = Font(
    R.font.inter_variable,
    weight = FontWeight(weight),
    variationSettings = FontVariation.Settings(FontVariation.weight(weight)),
)

private val Inter = FontFamily(inter(300), inter(400), inter(500), inter(600), inter(700))

/** Instrument Serif: an elegant display face for titles. It has one weight, plus an italic. */
private val Serif = FontFamily(
    Font(R.font.instrument_serif, FontWeight.Normal),
    Font(R.font.instrument_serif, FontWeight.Medium),
    Font(R.font.instrument_serif, FontWeight.SemiBold),
    Font(R.font.instrument_serif, FontWeight.Bold),
    Font(R.font.instrument_serif_italic, FontWeight.Normal, FontStyle.Italic),
)

/** Type scale: Instrument Serif for titles, Inter for everything you read or tap. */
@Immutable
class Type(val dark: Boolean, val p: Pal) {
    // Titles are set in the serif at regular weight; its large x-height and fine strokes do the work.
    val largeTitle = TextStyle(fontFamily = Serif, fontWeight = FontWeight.Normal, fontSize = 42.sp, lineHeight = 46.sp, letterSpacing = (-0.8).sp, color = p.text)
    val title = TextStyle(fontFamily = Serif, fontWeight = FontWeight.Normal, fontSize = 30.sp, lineHeight = 34.sp, letterSpacing = (-0.4).sp, color = p.text)
    val headline = TextStyle(fontFamily = Inter, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, lineHeight = 22.sp, letterSpacing = (-0.1).sp, color = p.text)
    val body = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Normal, fontSize = 16.sp, lineHeight = 25.sp, letterSpacing = (-0.05).sp, color = p.text)
    val callout = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Normal, fontSize = 15.sp, lineHeight = 22.sp, color = p.textDim)
    val label = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Medium, fontSize = 14.sp, lineHeight = 18.sp, letterSpacing = 0.1.sp, color = p.text)
    val caption = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Normal, fontSize = 13.sp, lineHeight = 18.sp, color = p.textDim)
    val micro = TextStyle(fontFamily = Inter, fontWeight = FontWeight.SemiBold, fontSize = 11.sp, lineHeight = 14.sp, letterSpacing = 1.1.sp, color = p.textFaint)
    /** Studio timecode: tabular figures so digits do not jitter. */
    val time = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Medium, fontSize = 12.sp, lineHeight = 16.sp, fontFeatureSettings = "tnum", color = p.textFaint)
    /** Transcript text: a touch larger than body for reading at arm's length. */
    val reading = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Normal, fontSize = 18.sp, lineHeight = 28.sp, letterSpacing = (-0.1).sp, color = p.text)
}

val LocalType = staticCompositionLocalOf { Type(true, Graphite) }

val pal: Pal @Composable get() = LocalPal.current
val type: Type @Composable get() = LocalType.current

/** Motion: springs for anything a finger moves, short eased fades for everything else. */
object Motion {
    val Emphasized = CubicBezierEasing(0.2f, 0f, 0f, 1f)
    fun <T> snappy() = spring<T>(dampingRatio = 0.82f, stiffness = 520f)
    fun <T> soft() = spring<T>(dampingRatio = 0.9f, stiffness = 260f)
    fun <T> bouncy() = spring<T>(dampingRatio = 0.62f, stiffness = 380f)
    fun <T> fade(ms: Int = 220) = tween<T>(ms, easing = Emphasized)
}

@Composable
fun isDark(appearance: String): Boolean = when (appearance) {
    "Light" -> false
    "Dark" -> true
    else -> isSystemInDarkTheme()
}

@Composable
fun MarginaliaTheme(appearance: String, content: @Composable () -> Unit) {
    val p = if (isDark(appearance)) Graphite else Paper
    val scheme: ColorScheme = if (p.dark) {
        darkColorScheme(
            primary = p.accent, onPrimary = p.onAccent, background = p.surface, onBackground = p.text,
            surface = p.surface2, onSurface = p.text, surfaceVariant = p.surface3, onSurfaceVariant = p.textDim,
            outline = p.lineStrong, error = p.danger,
        )
    } else {
        lightColorScheme(
            primary = p.accent, onPrimary = p.onAccent, background = p.surface, onBackground = p.text,
            surface = p.surface2, onSurface = p.text, surfaceVariant = p.surface3, onSurfaceVariant = p.textDim,
            outline = p.lineStrong, error = p.danger,
        )
    }
    CompositionLocalProvider(LocalPal provides p, LocalType provides Type(p.dark, p)) {
        ProportionalDensity { MaterialTheme(colorScheme = scheme, content = content) }
    }
}

/** The width the layout was designed on. */
private const val REFERENCE_WIDTH_DP = 411f

/**
 * Keeps the interface in the same proportion to the screen on every phone. A phone whose display
 * size setting makes it 461 dp wide would otherwise show everything about 11% smaller; here dp
 * follow the short side instead, clamped so small phones and tablets stay sensible. The font scale is
 * left alone, so the person's text-size accessibility setting still applies on top.
 */
@Composable
private fun ProportionalDensity(content: @Composable () -> Unit) {
    val base = androidx.compose.ui.platform.LocalDensity.current
    // The smaller side of the screen, so turning a phone on its side does not rescale everything.
    val shortSide = androidx.compose.ui.platform.LocalConfiguration.current.smallestScreenWidthDp.toFloat().coerceAtLeast(1f)
    // Phones follow their width. From 480 dp up the factor eases back to 1 at 600 dp, where tablets and
    // unfolded foldables keep true size and gain space instead of larger controls.
    val phone = (minOf(shortSide, 600f) / REFERENCE_WIDTH_DP).coerceIn(0.9f, 1.2f)
    val scale = when {
        shortSide >= 600f -> 1f
        shortSide > 480f -> phone + (1f - phone) * ((shortSide - 480f) / 120f)
        else -> phone
    }
    val scaled = androidx.compose.ui.unit.Density(base.density * scale, base.fontScale)
    CompositionLocalProvider(androidx.compose.ui.platform.LocalDensity provides scaled) { content() }
}
