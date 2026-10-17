@file:OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)

package app.marginalia.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Lock
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.remember
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import app.marginalia.AppVm
import app.marginalia.Boot
import app.marginalia.Source

@Composable
fun SetupScreen(boot: Boot, onRetry: () -> Unit) {
    val p = pal
    Box(Modifier.fillMaxSize().background(p.surface), contentAlignment = Alignment.Center) {
        Aurora(level = { 0.2f }, modifier = Modifier.fillMaxSize())
        Column(Modifier.padding(horizontal = 36.dp).navigationBarsPadding(), horizontalAlignment = Alignment.CenterHorizontally) {
            BrandMark(72.dp)
            Spacer(Modifier.height(28.dp))
            when (boot) {
                is Boot.Unpacking -> {
                    Text("Setting up your private AI", style = type.title, textAlign = TextAlign.Center)
                    Spacer(Modifier.height(10.dp))
                    Text(
                        "Unpacking the speech, search and language models onto this phone. This happens once.",
                        style = type.callout, textAlign = TextAlign.Center,
                    )
                    Spacer(Modifier.height(32.dp))
                    val frac = if (boot.total > 0) (boot.done.toFloat() / boot.total).coerceIn(0f, 1f) else 0f
                    val animated by animateFloatAsState(frac, Motion.soft(), label = "setup")
                    Box(Modifier.fillMaxWidth().height(8.dp).clip(CircleShape).background(p.surface4)) {
                        Box(Modifier.fillMaxWidth(animated).fillMaxSize().clip(CircleShape).background(p.accentBrush))
                    }
                    Spacer(Modifier.height(12.dp))
                    Text("%.1f of %.1f GB".format(boot.done / 1e9, boot.total / 1e9), style = type.time)
                }
                is Boot.Failed -> {
                    Text("Marginalia could not start", style = type.title, textAlign = TextAlign.Center)
                    Spacer(Modifier.height(10.dp))
                    Text(boot.message, style = type.callout, textAlign = TextAlign.Center)
                    Spacer(Modifier.height(28.dp))
                    PrimaryButton("Try again", onClick = onRetry)
                }
                else -> {}
            }
            Spacer(Modifier.height(36.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Rounded.Lock, null, tint = p.textFaint, modifier = Modifier.size(14.dp))
                Spacer(Modifier.width(6.dp))
                Text("Nothing leaves this phone", style = type.caption.copy(color = p.textFaint))
            }
        }
    }
}

/** Sliding-pill segmented control. The pill follows the selection with a spring. */
@Composable
fun Segmented(options: List<String>, selected: String, onSelect: (String) -> Unit, modifier: Modifier = Modifier) {
    val p = pal
    val haptic = LocalHapticFeedback.current
    val idx = options.indexOf(selected).coerceAtLeast(0)
    BoxWithConstraints(
        modifier.fillMaxWidth().height(44.dp).clip(RoundedCornerShape(p.rSmall + 2.dp)).background(p.surface3)
            .border(1.dp, p.line, RoundedCornerShape(p.rSmall + 2.dp)).padding(3.dp),
    ) {
        val seg = maxWidth / options.size
        val x by animateFloatAsState(idx.toFloat(), Motion.snappy(), label = "seg")
        Box(
            Modifier.offset(x = seg * x).width(seg).fillMaxSize().shadow(if (p.dark) 0.dp else 2.dp, RoundedCornerShape(p.rSmall))
                .clip(RoundedCornerShape(p.rSmall)).background(if (p.dark) p.surface4 else p.surface2),
        )
        Row(Modifier.fillMaxSize()) {
            options.forEach { o ->
                Box(
                    Modifier.weight(1f).fillMaxSize().pressable(pressedScale = 0.97f) {
                        if (o != selected) haptic.performHapticFeedback(HapticFeedbackType.SegmentTick)
                        onSelect(o)
                    },
                    contentAlignment = Alignment.Center,
                ) { Text(o, style = type.label.copy(color = if (o == selected) p.text else p.textDim)) }
            }
        }
    }
}

@Composable
private fun SettingRow(title: String, subtitle: String? = null, trailing: @Composable () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, style = type.body)
            if (subtitle != null) Text(subtitle, style = type.caption, modifier = Modifier.padding(top = 2.dp))
        }
        trailing()
    }
}

private val languages = listOf(
    "auto" to "Auto-detect", "en" to "English", "hi" to "हिन्दी", "es" to "Español", "fr" to "Français",
    "de" to "Deutsch", "pt" to "Português", "it" to "Italiano", "ru" to "Русский", "ar" to "العربية",
    "zh" to "中文", "ja" to "日本語", "ko" to "한국어",
)

@Composable
fun YouSheet(vm: AppVm, onDismiss: () -> Unit) {
    val p = pal
    val prefs by vm.prefs.collectAsState()
    val stats by vm.stats.collectAsState()
    val info by vm.modelInfo.collectAsState()
    val haptic = LocalHapticFeedback.current
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = p.surface2,
        contentColor = p.text,
        scrimColor = p.scrim,
        shape = RoundedCornerShape(topStart = p.rLarge, topEnd = p.rLarge),
        dragHandle = { SheetHandle() },
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).navigationBarsPadding()) {
            Text("You", style = type.largeTitle)
            Spacer(Modifier.height(18.dp))

            // numbers
            val hours = stats.seconds / 3600.0
            val grid = listOf(
                stats.meetings.toString() to "meetings",
                (if (hours >= 1) "%.1f h".format(hours) else "${stats.seconds / 60} min") to "transcribed",
                compact(stats.words) to "words heard",
                stats.documents.toString() to "documents",
                compact(stats.chunks) to "passages",
                stats.questions.toString() to "questions answered",
            )
            FlowRow(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(10.dp), maxItemsInEachRow = 3) {
                grid.forEach { (v, l) ->
                    Column(
                        Modifier.weight(1f).clip(RoundedCornerShape(p.rMid)).background(p.surface3).border(1.dp, p.line, RoundedCornerShape(p.rMid)).padding(14.dp),
                    ) {
                        Text(v, style = type.title)
                        Text(l, style = type.caption, maxLines = 1)
                    }
                }
            }

            SectionLabel("Appearance", Modifier.padding(top = 22.dp), flush = true)
            Segmented(listOf("System", "Light", "Dark"), prefs.appearance, vm::setAppearance)

            SectionLabel("Notes", Modifier.padding(top = 14.dp), flush = true)
            Segmented(listOf("Standard", "Concise"), prefs.noteStyle, vm::setStyle)

            Spacer(Modifier.height(8.dp))
            SettingRow("Live Assistance", "Answers questions raised in a meeting from your documents") {
                Switch(
                    checked = prefs.liveAssistance,
                    onCheckedChange = { haptic.performHapticFeedback(if (it) HapticFeedbackType.ToggleOn else HapticFeedbackType.ToggleOff); vm.setPref("live_assistance", it) },
                    colors = SwitchDefaults.colors(checkedTrackColor = p.accent, checkedThumbColor = Color.White, uncheckedTrackColor = p.surface4, uncheckedThumbColor = p.textDim, uncheckedBorderColor = p.lineStrong),
                )
            }

            val keep by vm.keepAudio.collectAsState()
            SettingRow("Keep the recording", "Saves the audio next to the transcript, about 11 MB an hour, so you can play any line back") {
                Switch(
                    checked = keep,
                    onCheckedChange = { haptic.performHapticFeedback(if (it) HapticFeedbackType.ToggleOn else HapticFeedbackType.ToggleOff); vm.setKeepAudio(it) },
                    colors = SwitchDefaults.colors(checkedTrackColor = p.accent, checkedThumbColor = Color.White, uncheckedTrackColor = p.surface4, uncheckedThumbColor = p.textDim, uncheckedBorderColor = p.lineStrong),
                )
            }

            SectionLabel("Gemma AI", Modifier.padding(top = 14.dp), flush = true)
            GemmaChooser(vm)
            val fast by vm.speedUp.collectAsState()
            SettingRow("Faster answers", "Gemma AI drafts a few words ahead and checks them. About a third quicker on the GPU. Turn it off if answers ever look odd") {
                Switch(
                    checked = fast,
                    onCheckedChange = { haptic.performHapticFeedback(if (it) HapticFeedbackType.ToggleOn else HapticFeedbackType.ToggleOff); vm.setSpeedUp(it) },
                    colors = SwitchDefaults.colors(checkedTrackColor = p.accent, checkedThumbColor = Color.White, uncheckedTrackColor = p.surface4, uncheckedThumbColor = p.textDim, uncheckedBorderColor = p.lineStrong),
                )
            }

            SectionLabel("Spoken language", Modifier.padding(top = 8.dp), flush = true)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                languages.forEach { (code, name) -> Chip(name, selected = prefs.language == code) { vm.setPref("language", code) } }
            }

            SectionLabel("Backup", Modifier.padding(top = 22.dp), flush = true)
            BackupRow(vm)

            Spacer(Modifier.height(26.dp))
            Column(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(p.rMid)).background(p.surface3).border(1.dp, p.line, RoundedCornerShape(p.rMid)).padding(16.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(Icons.Rounded.Lock, null, tint = p.success, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("Private by design", style = type.headline)
                }
                Spacer(Modifier.height(6.dp))
                Text("Transcription, notes, search and answers all run on this phone. Your audio and documents never leave it.", style = type.callout)
                if (info.isNotBlank()) {
                    Spacer(Modifier.height(10.dp))
                    Text(info, style = type.time)
                }
            }
            Spacer(Modifier.height(32.dp))
        }
    }
}

@Composable
private fun GemmaChooser(vm: AppVm) {
    val p = pal
    val choice by vm.gemmaChoice.collectAsState()
    val e4b by vm.e4b.collectAsState()
    Column {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Chip("E2B  smaller and faster", selected = choice == "e2b") { vm.useGemma("e2b") }
            if (e4b is AppVm.E4b.Ready) Chip("E4B  larger", selected = choice == "e4b") { vm.useGemma("e4b") }
        }
        Spacer(Modifier.height(8.dp))
        when (val s = e4b) {
            is AppVm.E4b.Absent -> {
                Text("E4B is a bigger Gemma AI that reads a little more carefully. It is a 3.7 GB download, kept on this phone, and it needs more memory.", style = type.caption)
                Spacer(Modifier.height(8.dp))
                TonalButton("Download E4B") { vm.downloadE4b() }
            }
            is AppVm.E4b.Downloading -> {
                Text("Downloading  ${s.done shr 20} of ${s.total shr 20} MB", style = type.caption)
                Spacer(Modifier.height(6.dp))
                androidx.compose.material3.LinearProgressIndicator(progress = { if (s.total > 0) s.done.toFloat() / s.total else 0f }, modifier = Modifier.fillMaxWidth(), color = p.accent, trackColor = p.surface4)
                Spacer(Modifier.height(8.dp))
                TonalButton("Cancel") { vm.cancelE4b() }
            }
            is AppVm.E4b.Ready -> TonalButton("Remove E4B from this phone") { vm.removeE4b() }
            is AppVm.E4b.Failed -> {
                Text(s.message, style = type.caption.copy(color = p.danger))
                Spacer(Modifier.height(8.dp))
                TonalButton("Try again") { vm.downloadE4b() }
            }
        }
    }
}

@Composable
private fun BackupRow(vm: AppVm) {
    val p = pal
    val stamp = remember { java.text.SimpleDateFormat("yyyy-MM-dd", java.util.Locale.US).format(java.util.Date()) }
    val save = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.CreateDocument("application/zip")) { uri -> uri?.let(vm::exportBackup) }
    val open = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.OpenDocument()) { uri -> uri?.let(vm::restoreBackup) }
    Column {
        Text("Everything you made goes into one file you keep wherever you like. Nothing is sent anywhere.", style = type.caption)
        Spacer(Modifier.height(8.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            TonalButton("Save a backup") { save.launch("marginalia-$stamp.zip") }
            TonalButton("Restore") { open.launch(arrayOf("application/zip", "application/octet-stream")) }
        }
    }
}

private fun compact(n: Long): String = when {
    n >= 1_000_000 -> "%.1fM".format(n / 1e6)
    n >= 10_000 -> "${n / 1000}k"
    n >= 1_000 -> "%.1fk".format(n / 1e3)
    else -> n.toString()
}

/** The exact words an answer was drawn from. */
@Composable
fun SourceSheet(src: Source, onDismiss: () -> Unit) {
    val p = pal
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = false),
        containerColor = p.surface2,
        contentColor = p.text,
        scrimColor = p.scrim,
        shape = RoundedCornerShape(topStart = p.rLarge, topEnd = p.rLarge),
        dragHandle = { SheetHandle() },
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp).navigationBarsPadding()) {
            Text((if (src.kind == "meeting") "From a meeting" else "From a document").uppercase(), style = type.micro)
            Spacer(Modifier.height(6.dp))
            Text(src.title, style = type.title)
            if (src.locator.isNotBlank()) Text(src.locator, style = type.caption, modifier = Modifier.padding(top = 4.dp))
            Spacer(Modifier.height(20.dp))
            if (src.quote.isNotBlank()) {
                Row(Modifier.fillMaxWidth()) {
                    Box(Modifier.width(3.dp).height(IntrinsicHeight()).clip(CircleShape).background(p.accentInk))
                    Spacer(Modifier.width(14.dp))
                    Text("“${src.quote.trim()}”", style = type.body.copy(fontSize = 17.sp()), modifier = Modifier.weight(1f))
                }
                Spacer(Modifier.height(20.dp))
            }
            if (src.snippet.isNotBlank()) {
                Text("Context".uppercase(), style = type.micro)
                Spacer(Modifier.height(6.dp))
                Text(src.snippet, style = type.callout)
            }
            Spacer(Modifier.height(32.dp))
        }
    }
}

private fun Int.sp() = androidx.compose.ui.unit.TextUnit(this.toFloat(), androidx.compose.ui.unit.TextUnitType.Sp)
private fun IntrinsicHeight() = androidx.compose.ui.unit.Dp.Unspecified
