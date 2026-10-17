package app.marginalia.ui

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.AutoAwesome
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.marginalia.Card
import app.marginalia.CardState
import app.marginalia.Line
import app.marginalia.Photo
import app.marginalia.Source
import app.marginalia.formatClock
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/** Decodes a photo near the size it is shown at, off the main thread. */
@Composable
fun rememberBitmap(path: String, targetPx: Int = 900): ImageBitmap? {
    val bmp by produceState<ImageBitmap?>(null, path, targetPx) {
        value = withContext(Dispatchers.IO) {
            val f = File(path)
            if (!f.isFile) return@withContext null
            val b = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeFile(path, b)
            var s = 1
            while (maxOf(b.outWidth, b.outHeight) / (s * 2) >= targetPx) s *= 2
            BitmapFactory.decodeFile(path, BitmapFactory.Options().apply { inSampleSize = s })?.asImageBitmap()
        }
    }
    return bmp
}

/** A line of transcript with its timecode; new lines ease in. */
@Composable
fun TranscriptRow(
    line: Line, modifier: Modifier = Modifier, animateIn: Boolean = false, dim: Float = 1f, active: Boolean = false, onPlay: (() -> Unit)? = null,
    speaker: String? = null, speakerColor: androidx.compose.ui.graphics.Color? = null,
) {
    val appear = remember { Animatable(if (animateIn) 0f else 1f) }
    LaunchedEffect(Unit) { if (animateIn) appear.animateTo(1f, Motion.fade(380)) }
    val tap = if (onPlay != null) Modifier.clickable(onClickLabel = "Play from here", onClick = onPlay) else Modifier
    Row(
        modifier.fillMaxWidth().then(tap).then(if (active) Modifier.background(pal.tonal) else Modifier)
            .padding(horizontal = 20.dp, vertical = 7.dp)
            .graphicsLayer { alpha = appear.value * dim; translationY = (1f - appear.value) * 14.dp.toPx() },
    ) {
        Text(formatClock(line.t0), style = type.time.copy(color = if (active) pal.accentInk else pal.textFaint), modifier = Modifier.width(48.dp).padding(top = 5.dp))
        Column(Modifier.weight(1f)) {
            if (speaker != null) Text(speaker, style = type.micro.copy(color = speakerColor ?: pal.accentInk), modifier = Modifier.padding(bottom = 2.dp))
            Text(line.text, style = type.reading)
        }
    }
}

private fun Int.sp() = androidx.compose.ui.unit.TextUnit(this.toFloat(), androidx.compose.ui.unit.TextUnitType.Sp)

/** The words being recognised right now, still settling. */
@Composable
fun PartialRow(text: String, modifier: Modifier = Modifier) {
    val t = rememberInfiniteTransition(label = "partial")
    val a by t.animateFloat(0.55f, 1f, infiniteRepeatable(tween(700), RepeatMode.Reverse), label = "a")
    Row(modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 7.dp)) {
        Box(Modifier.width(48.dp))
        Text(
            if (text.isBlank()) "…" else "$text …",
            style = type.reading.copy(color = pal.textDim, fontStyle = androidx.compose.ui.text.font.FontStyle.Italic),
            modifier = Modifier.alpha(a).weight(1f),
        )
    }
}

/** A photo in the timeline, with what Gemma saw in it. */
@Composable
fun PhotoCard(photo: Photo, modifier: Modifier = Modifier, onReadNow: (() -> Unit)? = null, onOpen: () -> Unit) {
    val p = pal
    val shape = RoundedCornerShape(p.rMid)
    val bmp = rememberBitmap(photo.path, 900)
    Column(modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp).clip(shape).background(p.surface3).border(1.dp, p.line, shape)) {
        Box(Modifier.fillMaxWidth().aspectRatio(16f / 10f).background(p.surface4).pressable(pressedScale = 0.99f, onClick = onOpen)) {
            bmp?.let { Image(it, "Photo", contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize()) }
            if (photo.t != null) {
                Text(
                    formatClock(photo.t),
                    style = type.time.copy(color = Color.White),
                    modifier = Modifier.align(Alignment.TopStart).padding(10.dp).clip(RoundedCornerShape(8.dp)).background(Color.Black.copy(alpha = 0.55f)).padding(horizontal = 8.dp, vertical = 3.dp),
                )
            }
        }
        Column(Modifier.padding(14.dp)) {
            if (photo.queued) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("Saved", style = type.label)
                        Text("The AI reads it after the meeting so the transcript keeps up", style = type.caption)
                    }
                    if (onReadNow != null) {
                        Spacer(Modifier.width(10.dp))
                        TonalButton("Read now", onClick = onReadNow)
                    }
                }
            } else if (photo.describing) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    ThinkingDots(color = p.accentInk)
                    Spacer(Modifier.width(10.dp))
                    Text("The AI is reading this photo", style = type.caption)
                }
            } else if (photo.caption.isNotBlank()) {
                Text(photo.caption, style = type.callout.copy(color = p.text), maxLines = 6, overflow = TextOverflow.Ellipsis)
            } else {
                Text("No description available", style = type.caption.copy(color = p.textFaint))
            }
        }
    }
}

/**
 * A larger look at a photo while a finger is held on its thumbnail. It floats above everything,
 * does not take touches, and fades away on release.
 */
@Composable
fun PhotoPeek(photo: Photo, show: Boolean) {
    val still = reducedMotion()
    val a by androidx.compose.animation.core.animateFloatAsState(if (show) 1f else 0f, tween(if (still) 0 else 160), label = "peek")
    if (!show && a == 0f) return
    val bmp = rememberBitmap(photo.path, 1400)
    androidx.compose.ui.window.Popup(
        popupPositionProvider = object : androidx.compose.ui.window.PopupPositionProvider {
            override fun calculatePosition(anchorBounds: androidx.compose.ui.unit.IntRect, windowSize: androidx.compose.ui.unit.IntSize, layoutDirection: androidx.compose.ui.unit.LayoutDirection, popupContentSize: androidx.compose.ui.unit.IntSize) = androidx.compose.ui.unit.IntOffset.Zero
        },
        properties = androidx.compose.ui.window.PopupProperties(focusable = false, dismissOnClickOutside = false, dismissOnBackPress = false, clippingEnabled = false),
    ) {
        androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxSize().alpha(a).background(Color.Black.copy(alpha = 0.55f)), contentAlignment = Alignment.Center) {
            // Never taller than the window, even on a phone held sideways.
            val maxImage = (maxHeight - 48.dp).coerceIn(120.dp, 520.dp)
            Box(
                Modifier.padding(24.dp).widthIn(max = 640.dp).graphicsLayer { val s = 0.94f + 0.06f * a; scaleX = s; scaleY = s }
                    .shadow(24.dp, RoundedCornerShape(20.dp)).clip(RoundedCornerShape(20.dp)).background(pal.surface4),
            ) {
                bmp?.let { Image(it, null, contentScale = ContentScale.Fit, modifier = Modifier.fillMaxWidth().heightIn(max = maxImage)) }
            }
        }
    }
}

/** Full-screen photo with pinch to zoom, drag to pan, and a delete action. */
@Composable
fun PhotoViewer(photo: Photo, onClose: () -> Unit, onDelete: () -> Unit) {
    val p = pal
    val bmp = rememberBitmap(photo.path, 2000)
    var scale by remember { mutableFloatStateOf(1f) }
    var ox by remember { mutableFloatStateOf(0f) }
    var oy by remember { mutableFloatStateOf(0f) }
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.96f)).pointerInput(Unit) {
        detectTransformGestures { _, pan, zoom, _ ->
            scale = (scale * zoom).coerceIn(1f, 5f)
            if (scale > 1f) { ox += pan.x; oy += pan.y } else { ox = 0f; oy = 0f }
        }
    }) {
        bmp?.let {
            Image(
                it, "Photo", contentScale = ContentScale.Fit,
                modifier = Modifier.fillMaxSize().graphicsLayer { scaleX = scale; scaleY = scale; translationX = ox; translationY = oy },
            )
        }
        Row(Modifier.fillMaxWidth().statusBarsPadding().padding(8.dp)) {
            IconAction(Icons.Rounded.Close, "Close", tint = Color.White, background = Color.White.copy(alpha = 0.14f), onClick = onClose)
            Spacer(Modifier.weight(1f))
            IconAction(Icons.Rounded.DeleteOutline, "Delete photo", tint = Color.White, background = Color.White.copy(alpha = 0.14f), onClick = onDelete)
        }
        if (photo.caption.isNotBlank()) {
            Text(
                photo.caption, style = type.callout.copy(color = Color.White),
                modifier = Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(16.dp)
                    .clip(RoundedCornerShape(p.rMid)).background(Color.Black.copy(alpha = 0.6f)).padding(14.dp),
            )
        }
    }
}

/** A Live Assistance card: a question heard in the meeting and its cited answer. */
@Composable
fun AssistCard(card: Card, modifier: Modifier = Modifier, onCite: (Source) -> Unit, onPick: ((String) -> Unit)? = null) {
    val p = pal
    val shape = RoundedCornerShape(p.rMid)
    if (card.kind == "ideas") {
        IdeasCard(card, modifier, onPick)
        return
    }
    Column(modifier.fillMaxWidth().clip(shape).background(p.surface3).border(1.dp, p.line, shape).padding(16.dp)) {
        Row(verticalAlignment = Alignment.Top) {
            Text(if (card.kind == "ask") card.question else "“${card.question}”", style = type.label.copy(color = p.accentInk), modifier = Modifier.weight(1f))
        }
        Spacer(Modifier.height(8.dp))
        when (card.state) {
            CardState.Retrieving -> Row(verticalAlignment = Alignment.CenterVertically) {
                ThinkingDots(color = p.accentInk); Spacer(Modifier.width(10.dp)); Text("Looking through your documents", style = type.caption)
            }
            CardState.NoMatch -> Text("Nothing in your documents covers this yet.", style = type.caption.copy(color = p.textFaint))
            else -> {
                if (card.answer.isBlank()) {
                    Row(verticalAlignment = Alignment.CenterVertically) { ThinkingDots(color = p.accentInk) }
                } else {
                    Markdown(card.answer, onCite = { n -> card.sources.getOrNull(n - 1)?.let(onCite) })
                }
                if (card.sources.isNotEmpty() && card.answer.isNotBlank()) {
                    Spacer(Modifier.height(12.dp))
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        card.sources.forEach { SourceChip(it) { onCite(it) } }
                    }
                }
            }
        }
    }
}


/** Questions the listener could put to the speaker. Tapping one keeps it as a Question in the notes. */
@Composable
private fun IdeasCard(card: Card, modifier: Modifier, onPick: ((String) -> Unit)?) {
    val p = pal
    val shape = RoundedCornerShape(p.rMid)
    val questions = card.answer.lines().map { it.trim().trimStart('-', '*', '•', ' ', '1', '2', '3', '.', ')').trim() }.filter { it.length > 6 }
    Column(modifier.fillMaxWidth().clip(shape).background(p.surface3).border(1.dp, p.accentInk.copy(alpha = 0.45f), shape).padding(16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Rounded.AutoAwesome, null, tint = p.accentInk, modifier = Modifier.size(16.dp))
            Spacer(Modifier.width(8.dp))
            Text("QUESTIONS YOU COULD ASK", style = type.micro.copy(color = p.accentInk))
        }
        Spacer(Modifier.height(10.dp))
        if (questions.isEmpty()) {
            ThinkingDots(color = p.accentInk)
        } else {
            questions.forEachIndexed { i, q ->
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(p.rSmall)).pressable(pressedScale = 0.98f, onClick = { onPick?.invoke(q) })
                        .padding(vertical = 8.dp),
                    verticalAlignment = Alignment.Top,
                ) {
                    Text("${i + 1}", style = type.label.copy(color = p.accentInk), modifier = Modifier.width(22.dp))
                    Text(q, style = type.body, modifier = Modifier.weight(1f))
                }
            }
            if (card.state != CardState.Done) { Spacer(Modifier.height(4.dp)); ThinkingDots(color = p.accentInk) }
            else if (onPick != null) { Spacer(Modifier.height(6.dp)); Text("Tap one to keep it in your notes.", style = type.caption.copy(color = p.textFaint)) }
        }
    }
}
