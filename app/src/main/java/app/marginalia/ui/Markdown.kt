// A small Markdown renderer for summaries and notes.

package app.marginalia.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.offset
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.CheckBox
import androidx.compose.material.icons.rounded.CheckBoxOutlineBlank
import androidx.compose.material3.Icon
import androidx.compose.ui.Alignment
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp

private sealed interface Block {
    data class Heading(val level: Int, val text: String) : Block
    /** [level] is the nesting depth (0 is top level). [number] is null for a plain bullet, [checked] is null when there is no checkbox. */
    data class Bullet(val level: Int, val number: Int?, val checked: Boolean?, val text: String) : Block
    data class Para(val text: String) : Block
}

private val bulletRe = Regex("^([-*•+])\\s+(.*)")
private val numberRe = Regex("^(\\d{1,4})[.)]\\s+(.*)")
private val checkRe = Regex("^\\[([ xX])]\\s+(.*)")

private fun parse(md: String): List<Block> {
    val out = mutableListOf<Block>()
    val para = StringBuilder()
    // Indent widths of the list items seen so far, so 2 or 4 spaces (or a tab) all give consistent levels.
    val indents = mutableListOf<Int>()
    var afterBlank = false
    fun flush() {
        if (para.isNotBlank()) out += Block.Para(para.toString().trim())
        para.clear()
    }
    fun levelFor(width: Int): Int {
        while (indents.isNotEmpty() && indents.last() > width) indents.removeAt(indents.lastIndex)
        if (indents.isEmpty() || indents.last() < width) indents += width
        return indents.size - 1
    }
    for (raw in md.lines()) {
        val line = raw.replace("\t", "    ").trimEnd()
        val t = line.trimStart()
        val width = line.length - t.length
        val bullet = bulletRe.find(t)
        val number = numberRe.find(t)
        when {
            t.isEmpty() -> { flush(); afterBlank = true; continue }
            t.startsWith("### ") -> { flush(); indents.clear(); out += Block.Heading(3, t.removePrefix("### ")) }
            t.startsWith("## ") -> { flush(); indents.clear(); out += Block.Heading(2, t.removePrefix("## ")) }
            t.startsWith("# ") -> { flush(); indents.clear(); out += Block.Heading(1, t.removePrefix("# ")) }
            bullet != null -> {
                flush()
                var body = bullet.groupValues[2]
                var checked: Boolean? = null
                checkRe.find(body)?.let { checked = it.groupValues[1] != " "; body = it.groupValues[2] }
                out += Block.Bullet(levelFor(width), null, checked, body)
            }
            number != null -> {
                flush()
                out += Block.Bullet(levelFor(width), number.groupValues[1].toInt(), null, number.groupValues[2])
            }
            else -> {
                // A wrapped line under a list item belongs to that item.
                val last = out.lastOrNull()
                if (para.isEmpty() && last is Block.Bullet && !afterBlank && width > 0) {
                    out[out.lastIndex] = last.copy(text = last.text + " " + t)
                } else {
                    if (para.isEmpty()) indents.clear()
                    if (para.isNotEmpty()) para.append(' ')
                    para.append(t)
                }
            }
        }
        afterBlank = false
    }
    flush()
    return out
}

/**
 * Inline markdown: **bold**, *italic*, `code`, and [n] citation markers. When [onCite] is
 * given, a citation becomes a tappable tag.
 */
@Composable
private fun inline(text: String, onCite: ((Int) -> Unit)?): AnnotatedString {
    val p = pal
    return remember(text, p, onCite != null) {
        buildAnnotatedString {
            val re = Regex("""\*\*(.+?)\*\*|\*(.+?)\*|_(.+?)_|`(.+?)`|\[(\d{1,2})]""")
            var i = 0
            for (m in re.findAll(text)) {
                if (m.range.first > i) append(text.substring(i, m.range.first))
                val g = m.groupValues
                when {
                    g[1].isNotEmpty() -> withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append(g[1]) }
                    g[2].isNotEmpty() -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(g[2]) }
                    g[3].isNotEmpty() -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(g[3]) }
                    g[4].isNotEmpty() -> withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = p.surface4)) { append(g[4]) }
                    g[5].isNotEmpty() -> {
                        val n = g[5].toInt()
                        val style = SpanStyle(color = p.accentInk, fontWeight = FontWeight.SemiBold, background = p.tonal)
                        if (onCite != null) {
                            withLink(LinkAnnotation.Clickable("cite$n", TextLinkStyles(style)) { onCite(n) }) { append(" $n ") }
                        } else {
                            withStyle(style) { append(" $n ") }
                        }
                    }
                }
                i = m.range.last + 1
            }
            if (i < text.length) append(text.substring(i))
        }
    }
}

private val LEVEL_INDENT = 20.dp
private val BULLET_BOX = 20.dp

/** Width of the marker box for a numbered item: wide enough for the longest number in its run, so "9." and "10." line up. */
private fun numberBox(digits: Int) = (14 + 10 * digits).dp

@Composable
fun Markdown(
    text: String,
    modifier: Modifier = Modifier,
    onCite: ((Int) -> Unit)? = null,
) {
    val t = type
    val p = pal
    val blocks = remember(text) { parse(text) }
    // For each numbered item, the digit count of the longest number among its neighbours at the same level.
    val digits = remember(blocks) {
        val d = IntArray(blocks.size)
        var i = 0
        while (i < blocks.size) {
            if (blocks[i] !is Block.Bullet) { i++; continue }
            var j = i
            while (j < blocks.size && blocks[j] is Block.Bullet) j++
            val run = blocks.subList(i, j).map { it as Block.Bullet }
            for (k in run.indices) {
                val n = run[k].number ?: continue
                d[i + k] = run.filter { it.level == run[k].level && it.number != null }.maxOf { it.number.toString().length }
                    .coerceAtLeast(n.toString().length)
            }
            i = j
        }
        d
    }
    Column(modifier) {
        blocks.forEachIndexed { idx, b ->
            val prev = blocks.getOrNull(idx - 1)
            val gap = when {
                idx == 0 -> 0.dp
                b is Block.Heading -> 16.dp
                b is Block.Bullet && prev is Block.Bullet -> if (b.level > prev.level) 4.dp else 6.dp
                prev is Block.Heading -> 6.dp
                else -> 10.dp
            }
            when (b) {
                is Block.Heading -> Text(
                    inline(b.text, onCite),
                    style = if (b.level == 1) t.title else t.headline,
                    modifier = Modifier.padding(top = gap),
                )
                is Block.Bullet -> {
                    val body = if (b.checked == true) t.body.copy(color = p.textDim) else t.body
                    if (b.checked != null) {
                        Row(Modifier.padding(top = gap, start = LEVEL_INDENT * b.level)) {
                            // The line is 25dp tall and the box is centred in it, so it sits on the first line.
                            Box(Modifier.width(BULLET_BOX + 6.dp).height(25.dp), contentAlignment = Alignment.CenterStart) {
                                Icon(
                                    if (b.checked) Icons.Rounded.CheckBox else Icons.Rounded.CheckBoxOutlineBlank,
                                    if (b.checked) "Done" else "Not done",
                                    tint = if (b.checked) p.accentInk else p.textFaint,
                                    modifier = Modifier.size(22.dp).offset(x = (-3).dp, y = (-1).dp),
                                )
                            }
                            Text(inline(b.text, onCite), style = body, modifier = Modifier.weight(1f))
                        }
                    } else {
                        val marker = if (b.number != null) "${b.number}." else "\u2022"
                        val box = if (b.number != null) numberBox(digits[idx]) else BULLET_BOX
                        Row(Modifier.padding(top = gap, start = LEVEL_INDENT * b.level)) {
                            Text(
                                marker,
                                style = body.copy(color = p.textFaint, fontFeatureSettings = "tnum"),
                                textAlign = TextAlign.End,
                                maxLines = 1,
                                modifier = Modifier.width(box).padding(end = 6.dp).alignByBaseline().then(if (b.number == null) Modifier.clearAndSetSemantics { } else Modifier),
                            )
                            Text(inline(b.text, onCite), style = body, modifier = Modifier.weight(1f).alignByBaseline())
                        }
                    }
                }
                is Block.Para -> Text(inline(b.text, onCite), style = t.body, modifier = Modifier.padding(top = gap))
            }
        }
    }
}
