package app.marginalia.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.RecordVoiceOver
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.border
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import app.marginalia.AppVm
import app.marginalia.MeetingDetail

private val speakerColors = listOf(
    Color(0xFF8B7CF6), Color(0xFF2FB5A0), Color(0xFFE08A3C), Color(0xFFD9627A), Color(0xFF4F8DDB), Color(0xFF9AA64B),
)

fun speakerColor(idx: Int): Color = speakerColors[idx % speakerColors.size]

fun speakerName(d: MeetingDetail, idx: Int): String = d.speakers[idx]?.takeIf { it.isNotBlank() } ?: "Speaker ${idx + 1}"

/**
 * Above the transcript: the button that tells speakers apart, its progress, and once it has run, one chip
 * per voice. A chip renames the voice, or says it was the same person as another one.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun SpeakerHeader(vm: AppVm, d: MeetingDetail) {
    val p = pal
    val working by vm.separating.collectAsState()
    val progress = working[d.meeting.id]
    val voices = d.lines.mapNotNull { it.speaker }.distinct().sorted()
    var editing by remember { mutableStateOf<Int?>(null) }
    val hasAudio = d.audio.any { it.exists }

    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 6.dp)) {
        when {
            progress != null -> Row(verticalAlignment = Alignment.CenterVertically) {
                ThinkingDots(color = p.accentInk)
                Spacer(Modifier.width(10.dp))
                Text("Telling speakers apart  ${(progress * 100).toInt()} percent", style = type.caption.copy(color = p.accentInk))
            }
            voices.isEmpty() && hasAudio -> {
                TonalButton("Tell speakers apart", icon = Icons.Rounded.RecordVoiceOver) { vm.separateSpeakers(d.meeting.id) }
                Spacer(Modifier.height(4.dp))
                Text("Listens to the recording on this phone. The first time it fetches a small speaker model.", style = type.caption.copy(color = p.textFaint))
            }
            voices.isNotEmpty() -> {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    voices.forEach { v ->
                        Row(
                            Modifier.clip(RoundedCornerShape(50)).background(p.surface3).border(1.dp, p.line, RoundedCornerShape(50))
                                .pressable(onClick = { editing = v }).padding(horizontal = 12.dp, vertical = 8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            androidx.compose.foundation.layout.Box(Modifier.size(10.dp).clip(CircleShape).background(speakerColor(v)))
                            Spacer(Modifier.width(8.dp))
                            Text(speakerName(d, v), style = type.caption.copy(color = p.text))
                        }
                    }
                }
            }
        }
    }

    val e = editing
    if (e != null) {
        var name by remember(e) { mutableStateOf(d.speakers[e].orEmpty()) }
        AlertDialog(
            onDismissRequest = { editing = null },
            containerColor = p.surface2,
            title = { Text("Who is this?", style = type.title) },
            text = {
                Column {
                    val shape = RoundedCornerShape(p.rMid)
                    androidx.compose.foundation.layout.Box(Modifier.fillMaxWidth().clip(shape).background(p.surface3).border(1.dp, p.line, shape).padding(14.dp)) {
                        if (name.isEmpty()) Text(speakerName(d, e), style = type.body.copy(color = p.textFaint))
                        BasicTextField(value = name, onValueChange = { name = it }, singleLine = true, textStyle = type.body, cursorBrush = SolidColor(p.accentInk), modifier = Modifier.fillMaxWidth())
                    }
                    val others = voices.filter { it != e }
                    if (others.isNotEmpty()) {
                        Spacer(Modifier.height(14.dp))
                        Text("OR THE SAME PERSON AS", style = type.micro)
                        Spacer(Modifier.height(6.dp))
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            others.forEach { o -> Chip(speakerName(d, o), selected = false) { vm.mergeSpeakers(d.meeting.id, e, o); editing = null } }
                        }
                    }
                    Spacer(Modifier.height(10.dp))
                    TextButton(onClick = { vm.clearSpeakers(d.meeting.id); editing = null }) { Text("Remove all speaker labels", color = p.danger) }
                }
            },
            confirmButton = { TextButton(onClick = { vm.renameSpeaker(d.meeting.id, e, name); editing = null }) { Text("Save", color = p.accentInk) } },
            dismissButton = { TextButton(onClick = { editing = null }) { Text("Cancel", color = p.textDim) } },
        )
    }
}
