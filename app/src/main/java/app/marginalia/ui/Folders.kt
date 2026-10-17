// Folder colours and the folder chip.

@file:OptIn(ExperimentalMaterial3Api::class)

package app.marginalia.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material.icons.rounded.FolderOpen
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.marginalia.Folder

/** Folder colours: pigments that read on both Graphite and Paper. */
val FolderColors = listOf(
    Color(0xFFD9694A), // terracotta
    Color(0xFFD9A441), // ochre
    Color(0xFF7FA377), // sage
    Color(0xFF5878E8), // klein
    Color(0xFFA66BD6), // plum
    Color(0xFF2FB0A6), // teal
    Color(0xFFD8688A), // rose
    Color(0xFF8793A8), // slate
)

fun folderColor(index: Int): Color = FolderColors[index.mod(FolderColors.size)]

@Composable
fun FolderDot(color: Int, size: androidx.compose.ui.unit.Dp = 10.dp) {
    Box(Modifier.size(size).clip(CircleShape).background(folderColor(color)))
}

/**
 * The folder filter. Tap to filter, long-press a folder to rename or delete it. With no folders yet
 * it is a single quiet invitation to make the first one.
 */
@Composable
fun FolderBar(
    folders: List<Folder>,
    selected: Long?,
    onSelect: (Long?) -> Unit,
    onNew: () -> Unit,
    onEdit: (Folder) -> Unit,
    modifier: Modifier = Modifier,
    allLabel: String = "All",
    showUnfiled: Boolean = true,
) {
    val p = pal
    val haptic = LocalHapticFeedback.current
    Row(modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 20.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        if (folders.isNotEmpty()) {
            Chip(allLabel, selected = selected == null) { onSelect(null) }
            folders.forEach { f ->
                FolderChip(f, selected == f.id, onClick = { onSelect(f.id) }, onLongClick = { haptic.performHapticFeedback(HapticFeedbackType.LongPress); onEdit(f) })
            }
            if (showUnfiled) Chip("Unfiled", selected = selected == -1L) { onSelect(-1L) }
        }
        Row(
            Modifier.height(36.dp).clip(CircleShape).border(1.dp, p.lineStrong, CircleShape).pressable(onClick = onNew).padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Rounded.Add, if (folders.isEmpty()) null else "New folder", tint = p.textDim, modifier = Modifier.size(16.dp))
            if (folders.isEmpty()) {
                Spacer(Modifier.width(6.dp))
                Text("New folder", style = type.label.copy(color = p.textDim))
            }
        }
    }
}

@Composable
fun FolderChip(f: Folder, selected: Boolean, onClick: () -> Unit, onLongClick: (() -> Unit)? = null) {
    val p = pal
    val bg = if (selected) p.tonal else p.surface3
    Row(
        Modifier.height(36.dp).clip(CircleShape).background(bg).border(1.dp, if (selected) p.accentInk.copy(alpha = 0.4f) else p.line, CircleShape)
            .pressable(onLongClick = onLongClick, onClick = onClick).padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        FolderDot(f.color, 9.dp)
        Spacer(Modifier.width(8.dp))
        Text(f.name, style = type.label.copy(color = if (selected) p.accentInk else p.textDim), maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** A small pill showing where something is filed; tap to change it. */
@Composable
fun FolderPill(folder: Folder?, onClick: () -> Unit, modifier: Modifier = Modifier, emptyLabel: String = "No folder") {
    val p = pal
    Row(
        modifier.height(32.dp).clip(CircleShape).background(p.surface3).border(1.dp, p.line, CircleShape).pressable(pressedScale = 0.95f, onClick = onClick).padding(start = 10.dp, end = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (folder != null) FolderDot(folder.color, 9.dp) else Icon(Icons.Rounded.FolderOpen, null, tint = p.textFaint, modifier = Modifier.size(15.dp))
        Spacer(Modifier.width(7.dp))
        Text(folder?.name ?: emptyLabel, style = type.caption.copy(color = if (folder != null) p.text else p.textDim), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(end = 2.dp))
        Icon(Icons.Rounded.ExpandMore, null, tint = p.textFaint, modifier = Modifier.size(16.dp))
    }
}

/** Create or edit a folder: a name and a colour. */
@Composable
fun FolderSheet(
    existing: Folder?,
    onDismiss: () -> Unit,
    onSave: (name: String, color: Int) -> Unit,
    onDelete: (() -> Unit)? = null,
) {
    val p = pal
    var name by remember { mutableStateOf(existing?.name.orEmpty()) }
    var color by remember { mutableStateOf(existing?.color ?: 3) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = p.surface2, contentColor = p.text, scrimColor = p.scrim,
        shape = RoundedCornerShape(topStart = p.rLarge, topEnd = p.rLarge),
        dragHandle = { SheetHandle() },
    ) {
        Column(Modifier.imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).navigationBarsPadding().padding(bottom = 20.dp)) {
            Text(if (existing == null) "New folder" else "Edit folder", style = type.title)
            Spacer(Modifier.height(16.dp))
            val shape = RoundedCornerShape(p.rMid)
            Row(Modifier.fillMaxWidth().height(54.dp).clip(shape).background(p.surface3).border(1.dp, p.line, shape).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                FolderDot(color, 14.dp)
                Spacer(Modifier.width(12.dp))
                Box(Modifier.weight(1f)) {
                    if (name.isEmpty()) Text("Name, like Product or Hiring", style = type.body.copy(color = p.textFaint))
                    BasicTextField(
                        value = name, onValueChange = { name = it.take(40) }, singleLine = true, textStyle = type.body, cursorBrush = SolidColor(p.accentInk),
                        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Words, imeAction = ImeAction.Done),
                        keyboardActions = KeyboardActions(onDone = { if (name.isNotBlank()) onSave(name, color) }),
                        modifier = Modifier.fillMaxWidth().focusRequester(focus),
                    )
                }
            }
            Spacer(Modifier.height(18.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                FolderColors.forEachIndexed { i, c ->
                    Box(
                        Modifier.size(38.dp).clip(CircleShape).background(c).pressable(pressedScale = 0.88f, haptic = HapticFeedbackType.SegmentTick) { color = i }
                            .semantics { role = Role.RadioButton; contentDescription = "Colour ${i + 1}"; selected = color == i },
                        contentAlignment = Alignment.Center,
                    ) { if (color == i) Icon(Icons.Rounded.Check, null, tint = Color.White, modifier = Modifier.size(20.dp)) }
                }
            }
            Spacer(Modifier.height(24.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                if (existing != null && onDelete != null) {
                    Box(Modifier.height(52.dp).clip(RoundedCornerShape(p.rMid)).border(1.dp, p.danger.copy(alpha = 0.5f), RoundedCornerShape(p.rMid)).pressable(onClick = onDelete).padding(horizontal = 20.dp), contentAlignment = Alignment.Center) {
                        Text("Delete", style = type.headline.copy(color = p.danger))
                    }
                }
                PrimaryButton(if (existing == null) "Create folder" else "Save", Modifier.weight(1f), enabled = name.isNotBlank()) { onSave(name, color) }
            }
            if (existing != null) Text("Deleting a folder keeps everything in it. Items become unfiled.", style = type.caption.copy(color = p.textFaint), modifier = Modifier.padding(top = 12.dp))
        }
    }
}

/** Choose where something is filed. */
@Composable
fun FolderPicker(
    title: String,
    folders: List<Folder>,
    current: Long?,
    onPick: (Long?) -> Unit,
    onNew: () -> Unit,
    onDismiss: () -> Unit,
    noneLabel: String = "No folder",
) {
    val p = pal
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = false),
        containerColor = p.surface2, contentColor = p.text, scrimColor = p.scrim,
        shape = RoundedCornerShape(topStart = p.rLarge, topEnd = p.rLarge),
        dragHandle = { SheetHandle() },
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).navigationBarsPadding().padding(bottom = 20.dp)) {
            Text(title, style = type.title)
            Spacer(Modifier.height(12.dp))
            PickRow(null, noneLabel, current == null) { onPick(null) }
            folders.forEach { f -> PickRow(f, f.name, current == f.id, sub = "${f.meetings} meetings · ${f.docs} files") { onPick(f.id) } }
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(p.rMid)).pressable(pressedScale = 0.98f, onClick = onNew).padding(vertical = 12.dp, horizontal = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.size(40.dp).clip(RoundedCornerShape(p.rSmall)).border(1.dp, p.lineStrong, RoundedCornerShape(p.rSmall)), contentAlignment = Alignment.Center) {
                    Icon(Icons.Rounded.Add, null, tint = p.accentInk, modifier = Modifier.size(22.dp))
                }
                Spacer(Modifier.width(14.dp))
                Text("New folder", style = type.headline.copy(color = p.accentInk))
            }
        }
    }
}

@Composable
private fun PickRow(folder: Folder?, label: String, selected: Boolean, sub: String? = null, onClick: () -> Unit) {
    val p = pal
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(p.rMid)).background(if (selected) p.tonal else Color.Transparent).pressable(pressedScale = 0.98f, haptic = HapticFeedbackType.SegmentTick, onClick = onClick).padding(vertical = 12.dp, horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(40.dp).clip(RoundedCornerShape(p.rSmall)).background(if (folder != null) folderColor(folder.color).copy(alpha = 0.16f) else p.surface3), contentAlignment = Alignment.Center) {
            if (folder != null) FolderDot(folder.color, 14.dp) else Icon(Icons.Rounded.FolderOpen, null, tint = p.textFaint, modifier = Modifier.size(20.dp))
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(label, style = type.headline)
            if (sub != null) Text(sub, style = type.caption)
        }
        if (selected) Icon(Icons.Rounded.Check, "Selected", tint = p.accentInk, modifier = Modifier.size(22.dp).padding(end = 4.dp))
    }
}

/** One place that owns the create/edit/pick sheets so every screen offers folders the same way. */
class FolderUi {
    var editing by mutableStateOf<Folder?>(null)
    var creating by mutableStateOf(false)
    /** Called with the new folder's id after "New folder" from a picker. */
    var afterCreate: ((Long) -> Unit)? = null
}

@Composable
fun rememberFolderUi() = remember { FolderUi() }

/** Renders whichever create/edit sheet [ui] has asked for. Put one on each screen that shows folders. */
@Composable
fun FolderSheets(vm: app.marginalia.AppVm, ui: FolderUi) {
    if (ui.creating) {
        FolderSheet(
            existing = null,
            onDismiss = { ui.creating = false },
            onSave = { n, c -> vm.createFolder(n, c) { id -> ui.afterCreate?.invoke(id); ui.afterCreate = null }; ui.creating = false },
        )
    }
    ui.editing?.let { f ->
        FolderSheet(
            existing = f,
            onDismiss = { ui.editing = null },
            onSave = { n, c -> vm.renameFolder(f, n, c); ui.editing = null },
            onDelete = { vm.deleteFolder(f); ui.editing = null },
        )
    }
}
