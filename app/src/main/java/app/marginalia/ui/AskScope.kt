// The control and sheet that choose what Ask draws from.

package app.marginalia.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.DeleteSweep
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.marginalia.Folder

/** What Ask can draw from: everything, or only meetings, or only documents. */
private val scopes = listOf(
    Triple("all", "Everything", "Your meetings and your documents together"),
    Triple("meetings", "Meetings only", "What was said and noted in the meetings you recorded"),
    Triple("documents", "Documents only", "The files, notes and web pages you added to Knowledge"),
)

fun scopeLabel(scope: String, folder: Folder?): String {
    val what = scopes.firstOrNull { it.first == scope }?.second ?: "Everything"
    return if (folder != null) "${what.removeSuffix(" only")} in ${folder.name}" else what
}

/** The pill under the title that says where answers come from, and a button for starting over. */
@Composable
fun AskScopeBar(label: String, onScope: () -> Unit, showClear: Boolean, onClear: () -> Unit) {
    // The scope pill takes whatever room "New chat" leaves, so a label like "Everything" is never cut short.
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = androidx.compose.foundation.layout.Arrangement.SpaceBetween,
    ) {
        Pill(Icons.Rounded.Search, label, trailing = Icons.Rounded.ExpandMore, onClick = onScope, modifier = Modifier.weight(1f, fill = false))
        if (showClear) {
            Spacer(Modifier.width(10.dp))
            Pill(Icons.Rounded.DeleteSweep, "New chat", onClick = onClear)
        }
    }
}

@Composable
private fun Pill(icon: ImageVector, text: String, onClick: () -> Unit, modifier: Modifier = Modifier, trailing: ImageVector? = null) {
    val p = pal
    val shape = RoundedCornerShape(50)
    Row(
        modifier.height(40.dp).clip(shape).background(p.surface3).border(1.dp, p.line, shape).pressable(pressedScale = 0.97f, onClick = onClick).padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, null, tint = p.accentInk, modifier = Modifier.size(18.dp))
        Spacer(Modifier.width(8.dp))
        Text(text, style = type.label.copy(color = p.text), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
        if (trailing != null) {
            Spacer(Modifier.width(4.dp))
            Icon(trailing, null, tint = p.textDim, modifier = Modifier.size(18.dp))
        }
    }
}

/** Two plain questions: what to search, and where. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AskScopeSheet(
    scope: String,
    folders: List<Folder>,
    folder: Long?,
    onScope: (String) -> Unit,
    onFolder: (Long?) -> Unit,
    onDismiss: () -> Unit,
) {
    val p = pal
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = p.surface2, contentColor = p.text, scrimColor = p.scrim,
        shape = RoundedCornerShape(topStart = p.rLarge, topEnd = p.rLarge),
        dragHandle = { SheetHandle() },
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).navigationBarsPadding().padding(bottom = 20.dp)) {
            Text("Ask from", style = type.title)
            Spacer(Modifier.height(4.dp))
            Text("Pick what the answer may use. Every answer lists the passages it came from.", style = type.caption)
            Spacer(Modifier.height(14.dp))
            SectionLabel("What", flush = true)
            scopes.forEach { (key, name, hint) -> ChoiceRow(name, hint, scope == key, null) { onScope(key) } }
            Spacer(Modifier.height(8.dp))
            SectionLabel("Which folder", flush = true)
            ChoiceRow("All folders", "Everything, filed or not", folder == null, null) { onFolder(null) }
            folders.forEach { f ->
                ChoiceRow(f.name, "${f.meetings} ${if (f.meetings == 1) "meeting" else "meetings"}  ·  ${f.docs} ${if (f.docs == 1) "document" else "documents"}", folder == f.id, f.color) { onFolder(f.id) }
            }
            if (folders.isEmpty()) Text("Folders you make in Meetings or Knowledge show up here.", style = type.caption.copy(color = p.textFaint), modifier = Modifier.padding(top = 4.dp))
            Spacer(Modifier.height(14.dp))
            PrimaryButton("Done", Modifier.fillMaxWidth(), onClick = onDismiss)
        }
    }
}

@Composable
private fun ChoiceRow(title: String, hint: String, selected: Boolean, color: Int?, onClick: () -> Unit) {
    val p = pal
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(p.rMid)).background(if (selected) p.tonal else Color.Transparent)
            .pressable(pressedScale = 0.98f, onClick = onClick).padding(vertical = 12.dp, horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (color != null) {
            FolderDot(color, 12.dp)
            Spacer(Modifier.width(12.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(title, style = type.headline, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(hint, style = type.caption)
        }
        if (selected) Icon(Icons.Rounded.Check, "Selected", tint = p.accentInk, modifier = Modifier.size(22.dp))
    }
}
