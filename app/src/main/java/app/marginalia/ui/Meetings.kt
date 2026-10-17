// The Meetings list and the date and duration labels it uses.

package app.marginalia.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.marginalia.AppVm
import app.marginalia.Boot
import app.marginalia.MeetingItem
import androidx.compose.animation.togetherWith
import kotlinx.coroutines.launch
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter

@Composable
fun MeetingsScreen(vm: AppVm, onRecord: () -> Unit, selectedId: Long? = null, onOpenHome: () -> Unit = {}) {
    val p = pal
    val all by vm.meetings.collectAsState()
    val boot by vm.boot.collectAsState()
    val folders by vm.folders.collectAsState()
    val filter by vm.meetingsFilter.collectAsState()
    val finishing by vm.finishing.collectAsState()
    val hold by vm.holdFinishing.collectAsState()
    val folderUi = rememberFolderUi()
    var moving by remember { mutableStateOf<MeetingItem?>(null) }
    val list = rememberLazyListState()
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    var searching by rememberSaveable { mutableStateOf(false) }
    var query by rememberSaveable { mutableStateOf("") }
    val shown = remember(all, query, filter) {
        val q = query.trim().lowercase()
        all.filter { m ->
            (filter == null || (filter == -1L && m.folderId == null) || m.folderId == filter) &&
                (q.isEmpty() || m.title.lowercase().contains(q) || m.summary.lowercase().contains(q) || m.notesEnhanced.lowercase().contains(q))
        }
    }
    val groups = remember(shown) { shown.groupBy { dayOf(it.startedAt) }.toList() }

    CappedPane {
    Box(Modifier.fillMaxSize()) {
        LazyColumn(state = list, contentPadding = PaddingValues(bottom = bottomClearance()), modifier = Modifier.fillMaxSize()) {
            item(key = "title") { LargeTitle("Meetings", null, showSubtitle = false) }
            item(key = "sub") {
                // The search icon sits under the title, level with the count; tapping turns the row into the field.
                androidx.compose.animation.AnimatedContent(
                    targetState = searching,
                    transitionSpec = { fadeIn(Motion.fade(180)) togetherWith fadeOut(Motion.fade(120)) },
                    label = "search-row",
                ) { s ->
                    if (s) {
                        SearchField(
                            query, { query = it }, "Search meetings",
                            Modifier.padding(horizontal = 20.dp).height(48.dp), autoFocus = true,
                            onClose = { searching = false; query = "" },
                        )
                    } else {
                        SubtitleRow(if (all.isEmpty()) "Nothing recorded yet" else "${all.size} ${if (all.size == 1) "meeting" else "meetings"}") {
                            // Nothing to search until there is something recorded.
                            if (all.isNotEmpty()) IconAction(Icons.Rounded.Search, "Search meetings") { searching = true }
                        }
                    }
                }
            }
            item(key = "connect") { ConnectBanner(vm) }
            item(key = "homechip") { HomeComputerChip(vm, onClick = onOpenHome) }
            item(key = "warm") {
                AnimatedVisibility(
                    visible = boot is Boot.Loading,
                    enter = fadeIn() + expandVertically(Motion.soft()),
                    exit = fadeOut() + shrinkVertically(Motion.soft()),
                ) {
                    val b = boot as? Boot.Loading
                    Row(
                        Modifier.padding(horizontal = 20.dp).padding(bottom = 12.dp).fillMaxWidth().clip(RoundedCornerShape(p.rSmall)).background(p.tonal).padding(horizontal = 14.dp, vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        ThinkingDots(color = p.accentInk)
                        Spacer(Modifier.width(10.dp))
                        Text(
                            (b?.label?.takeIf { it.isNotBlank() && it != "Starting" } ?: "Waking up on-device AI") + (if (b != null && b.step > 0) "  ${b.step}/${b.steps}" else ""),
                            style = type.caption.copy(color = p.accentInk),
                        )
                    }
                }
            }
            item(key = "folders") {
                FolderBar(
                    folders, filter, onSelect = { vm.meetingsFilter.value = it },
                    onNew = { folderUi.creating = true }, onEdit = { folderUi.editing = it },
                    modifier = Modifier.padding(bottom = 10.dp),
                )
            }
            if (shown.isEmpty()) {
                item(key = "empty") {
                    if (all.isEmpty()) {
                        EmptyState(
                            "Your meetings live here",
                            "Tap the mic to record. Marginalia transcribes, writes your notes and answers questions without ever leaving this phone.",
                        )
                    } else if (query.isBlank()) {
                        EmptyState("Nothing in this folder yet", "Press and hold a meeting to move it here, or record a new one while this folder is selected.")
                    } else {
                        EmptyState("No matches", "Nothing in your meetings matches “$query”.")
                    }
                }
            }
            groups.forEach { (day, meetings) ->
                item(key = "h$day") { SectionLabel(dayLabel(day)) }
                items(meetings, key = { it.id }) { m ->
                    SwipeToDelete(
                        onDelete = { vm.deleteMeeting(m) },
                        modifier = Modifier.animateItem().padding(horizontal = 16.dp, vertical = 5.dp),
                    ) {
                        MeetingCard(
                            m, wrap = finishing[m.id]?.let { if (hold) it.copy(label = "Waiting until your recording ends") else it }, folder = folders.firstOrNull { it.id == m.folderId }.takeIf { filter == null },
                            onLongClick = { moving = m }, onClick = { vm.openReview(m.id) }, selected = m.id == selectedId,
                        )
                    }
                }
            }
        }
        moving?.let { m ->
            FolderPicker(
                title = "Move to folder", folders = folders, current = m.folderId, noneLabel = "No folder",
                onPick = { vm.moveMeeting(m.id, it); moving = null },
                onNew = { folderUi.afterCreate = { id -> vm.moveMeeting(m.id, id) }; moving = null; folderUi.creating = true },
                onDismiss = { moving = null },
            )
        }
        FolderSheets(vm, folderUi)
        TopBarOverlay("Meetings", list, Modifier.align(Alignment.TopCenter)) {
            if (all.isNotEmpty()) IconAction(Icons.Rounded.Search, "Search meetings") {
                searching = true
                scope.launch { list.animateScrollToItem(0) }
            }
        }
        // With nothing recorded yet, point at the record button in the floating bar (its top edge is 90dp above the bottom).
        val recording by vm.live.collectAsState()
        val keyboardUp = androidx.compose.foundation.layout.WindowInsets.Companion.ime.getBottom(androidx.compose.ui.platform.LocalDensity.current) > 0
        if (all.isEmpty() && recording == null && !keyboardUp && !win.rail) {
            PointerArrow(Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 96.dp))
        }
    }
    }
}

@Composable
private fun MeetingCard(m: MeetingItem, wrap: app.marginalia.WrapState?, folder: app.marginalia.Folder?, onLongClick: () -> Unit, onClick: () -> Unit, selected: Boolean = false) {
    val p = pal
    val shape = RoundedCornerShape(p.rMid)
    Column(
        Modifier.fillMaxWidth().clip(shape).background(if (selected) p.tonal.compositeOver(p.surface3) else p.surface3)
            .border(if (selected) 1.5.dp else 1.dp, if (selected) p.accentInk.copy(alpha = 0.7f) else if (wrap != null) p.accentInk.copy(alpha = 0.45f) else p.line, shape)
            .pressable(pressedScale = 0.98f, onLongClick = onLongClick, onClick = onClick).padding(16.dp),
    ) {
        Text(
            m.title.ifBlank { "Untitled meeting" },
            style = type.headline, maxLines = 2, overflow = TextOverflow.Ellipsis,
        )
        Spacer(Modifier.height(3.dp))
        Text(
            buildString {
                append(timeOf(m.startedAt))
                if (m.durationSecs > 0) append("  ·  ").append(durationLabel(m.durationSecs))
            },
            style = type.time,
        )
        if (wrap != null) {
            // Being written up in the background: say how far along it is.
            Spacer(Modifier.height(12.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                ThinkingDots(color = p.accentInk)
                Spacer(Modifier.width(10.dp))
                Text(wrap.label, style = type.caption.copy(color = p.accentInk), modifier = Modifier.weight(1f))
            }
            Spacer(Modifier.height(8.dp))
            val prog by androidx.compose.animation.core.animateFloatAsState((wrap.progress / 100f).coerceIn(0.03f, 1f), Motion.soft(), label = "card-wrap")
            Box(Modifier.fillMaxWidth().height(4.dp).clip(androidx.compose.foundation.shape.CircleShape).background(p.surface4)) {
                Box(Modifier.fillMaxWidth(prog).fillMaxHeight().clip(androidx.compose.foundation.shape.CircleShape).background(p.accentBrush))
            }
        }
        val preview = preview(m)
        if (preview.isNotBlank() && wrap == null) {
            Spacer(Modifier.height(8.dp))
            Text(preview, style = type.callout, maxLines = 2, overflow = TextOverflow.Ellipsis)
        } else if (m.segmentCount == 0 && wrap == null) {
            Spacer(Modifier.height(8.dp))
            Text("Just notes", style = type.callout.copy(color = p.textFaint))
        }
        if (folder != null) {
            Spacer(Modifier.height(10.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                FolderDot(folder.color, 8.dp)
                Spacer(Modifier.width(7.dp))
                Text(folder.name, style = type.caption.copy(color = p.textFaint))
            }
        }
    }
}

private fun preview(m: MeetingItem): String {
    val src = m.summary.ifBlank { m.notesEnhanced.ifBlank { m.notesRaw } }
    return src.lines().map { it.trim() }.filter { it.isNotEmpty() && !it.startsWith("#") }
        .joinToString(" ") { it.trimStart('-', '*', '•', ' ') }.replace("**", "").take(220)
}

private val zone = ZoneId.systemDefault()
fun dayOf(epoch: Long): LocalDate = Instant.ofEpochSecond(epoch).atZone(zone).toLocalDate()
fun timeOf(epoch: Long): String = Instant.ofEpochSecond(epoch).atZone(zone).format(DateTimeFormatter.ofPattern("h:mm a"))
fun dayLabel(d: LocalDate): String {
    val today = LocalDate.now(zone)
    return when {
        d == today -> "Today"
        d == today.minusDays(1) -> "Yesterday"
        d.year == today.year -> d.format(DateTimeFormatter.ofPattern("EEEE, MMM d"))
        else -> d.format(DateTimeFormatter.ofPattern("MMM d, yyyy"))
    }
}

fun durationLabel(secs: Long): String = when {
    secs >= 3600 -> "${secs / 3600} h ${(secs / 60) % 60} min"
    secs >= 60 -> "${secs / 60} min"
    else -> "$secs s"
}
