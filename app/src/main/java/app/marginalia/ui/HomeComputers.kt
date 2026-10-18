// Settings and prompts for home computers that can do the AI work faster than the phone.

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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
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
import androidx.compose.material.icons.rounded.ExpandLess
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.marginalia.AppVm
import app.marginalia.FoundComputer
import app.marginalia.CORRECTION_WINDOWS_S
import app.marginalia.HomeComputer
import app.marginalia.HomeComputers
import app.marginalia.Prefs
import app.marginalia.canCorrectLive
import app.marginalia.transcribeNode
import app.marginalia.windowLimitNote
import app.marginalia.offerTitle
import app.marginalia.NodeChip
import app.marginalia.NodeLight
import app.marginalia.nodeChip
import app.marginalia.unpairedFound
import kotlin.math.roundToInt

/** Words a second, worked out from the AI's tokens a second (a word is a little over one token). */
internal fun wordsPerSecond(tokPerSec: Double): Int = (tokPerSec * 0.75).roundToInt()

/** What the phone says about one paired computer: a status line, and a detail line if there is one. */
internal fun statusOf(c: HomeComputer): Pair<String, String?> {
    if (c.state == "waiting") return "Waiting" to null
    if (c.state == "refused") return "Not allowed by this compute node" to null
    val status = when {
        !c.online -> "Not reachable on this network"
        c.paused -> "Paused on the node"
        c.chatTokPerSec >= 20.0 -> "Online, fast"
        else -> "Online"
    }
    val words = wordsPerSecond(c.chatTokPerSec)
    val detail = buildList {
        if (c.online && !c.paused && words > 0) add("Answers at $words words a second")
        if (c.online && !c.paused && c.busy > 0) add("Busy")
    }.joinToString(". ").ifBlank { null }
    return status to detail
}

internal const val IMPROVE_HELP = "After a meeting, a compute node re-transcribes the recording with a larger model. Nothing is kept there."

internal const val CORRECT_TITLE = "Correct the transcript while recording"
internal const val CORRECT_HELP = "While recording, a compute node listens again in short windows and fixes the captions a few seconds later."
internal const val CORRECT_NEEDS_NODE = "Needs a compute node with the speech model"
internal const val CORRECT_WINDOW_HELP = "Longer windows are more accurate and show up later."

/** The switch for live correction and, under it, the length of a correction window. Both are off and grey without a compute node that can listen. */
@Composable
private fun LiveCorrectionRows(prefs: Prefs, nodes: HomeComputers, vm: AppVm) {
    val usable = canCorrectLive(nodes)
    val on = prefs.liveCorrection && usable
    Column(Modifier.alpha(if (usable) 1f else 0.5f)) {
        SettingRow(CORRECT_TITLE, if (usable) CORRECT_HELP else CORRECT_NEEDS_NODE) {
            NodeSwitch(on, enabled = usable) { vm.setPref("live_correction", it) }
        }
    }
    Column(Modifier.alpha(if (on) 1f else 0.5f).padding(bottom = 6.dp)) {
        Text("Correction window", style = type.body)
        Spacer(Modifier.height(6.dp))
        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            CORRECTION_WINDOWS_S.forEach { s ->
                Chip("$s s", selected = prefs.liveCorrectionWindowS == s) { if (on) vm.setPref("live_correction_window_s", s) }
            }
        }
        Text(CORRECT_WINDOW_HELP, style = type.caption, modifier = Modifier.padding(top = 4.dp))
        val limit = transcribeNode(nodes)?.let { windowLimitNote(prefs.liveCorrectionWindowS, it.windowMaxS) }
        if (on && limit != null) Text(limit, style = type.caption)
    }
}

internal const val WAITING_HELP = "Waiting for approval. Open the app on a phone that already uses these compute nodes, or run marginalia-node approve on the compute node."

/** "falcon", "falcon and hawk", "a, b and c". */
internal fun joinNames(names: List<String>): String = when (names.size) {
    0 -> ""
    1 -> names[0]
    else -> names.dropLast(1).joinToString(", ") + " and " + names.last()
}

/** The first 8 characters of a compute node's id in two groups, like "76a8 ae41". Empty when there is no id. */
internal fun shortId(id: String): String = id.trim().take(8).chunked(4).joinToString(" ")

/** The code the person types may have any case and a dash. The compute node wants 8 letters and digits. */
internal fun cleanCode(raw: String): String = raw.filter { it.isLetterOrDigit() }.uppercase().take(8)

/** Text from the compute node goes on screen, so it is kept to the house style. */
private fun plain(s: String): String = s.replace("—", ", ").replace(";", ".").trim()

@Composable
private fun NodeSwitch(checked: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    val p = pal
    val haptic = LocalHapticFeedback.current
    Switch(
        checked = checked,
        enabled = enabled,
        onCheckedChange = { haptic.performHapticFeedback(if (it) HapticFeedbackType.ToggleOn else HapticFeedbackType.ToggleOff); onChange(it) },
        colors = SwitchDefaults.colors(checkedTrackColor = p.accent, checkedThumbColor = Color.White, uncheckedTrackColor = p.surface4, uncheckedThumbColor = p.textDim, uncheckedBorderColor = p.lineStrong),
    )
}

/** The "Compute nodes" part of the settings sheet. It keeps the list fresh for as long as it is on screen. */
@Composable
fun HomeComputersSection(vm: AppVm, modifier: Modifier = Modifier) {
    val p = pal
    val nodes by vm.nodes.collectAsState()
    var adding by remember { mutableStateOf(false) }
    var removing by remember { mutableStateOf<HomeComputer?>(null) }
    LaunchedEffect(Unit) { vm.watchNodes() }

    Column(modifier) {
        SectionLabel("Compute nodes", Modifier.padding(top = 22.dp), flush = true)
        Text("Use a compute node on your network to make the AI faster. The phone keeps working on its own when none is around.", style = type.caption)
        Spacer(Modifier.height(4.dp))
        SettingRow("Use compute nodes when available") { NodeSwitch(nodes.enabled) { vm.setNodesEnabled(it) } }
        val prefs by vm.prefs.collectAsState()
        SettingRow("Improve transcripts on a compute node", IMPROVE_HELP) { NodeSwitch(prefs.improveTranscript) { vm.setPref("improve_transcript", it) } }
        LiveCorrectionRows(prefs, nodes, vm)

        nodes.paired.forEach { c ->
            Spacer(Modifier.height(8.dp))
            ComputerRow(c, onRemove = { removing = c })
        }

        if (nodes.paired.any { it.state == "waiting" }) {
            Spacer(Modifier.height(8.dp))
            Text(WAITING_HELP, style = type.caption)
        }

        val newFound = unpairedFound(nodes)
        if (newFound.isNotEmpty()) {
            Spacer(Modifier.height(8.dp))
            val shape = RoundedCornerShape(p.rMid)
            Row(
                Modifier.fillMaxWidth().clip(shape).background(p.surface3).border(1.dp, p.line, shape).padding(start = 14.dp, top = 8.dp, bottom = 8.dp, end = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(if (newFound.size == 1) "1 compute node found" else "${newFound.size} compute nodes found on your network", style = type.headline, modifier = Modifier.weight(1f))
                TonalButton("Review") { adding = true }
            }
        }

        Spacer(Modifier.height(12.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            TonalButton("Add a compute node", icon = Icons.Rounded.Add) { adding = true }
            if (nodes.paired.isNotEmpty()) TonalButton("Check again") { vm.refreshNodes() }
        }
    }

    if (adding) AddComputerSheet(vm, onDismiss = { adding = false })

    removing?.let { c ->
        AlertDialog(
            onDismissRequest = { removing = null },
            containerColor = p.surface2,
            title = { Text("Remove ${c.name.ifBlank { "this compute node" }}?", style = type.title) },
            text = { Text("The phone will stop using it. You can add it again later.", style = type.callout) },
            confirmButton = { TextButton(onClick = { vm.forgetNode(c.fp); removing = null }) { Text("Remove", color = p.danger) } },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel", color = p.textDim) } },
        )
    }
}

/** Green running, yellow paused, red stopped or not reachable. */
@Composable
internal fun lightColor(light: NodeLight): Color = when (light) {
    NodeLight.Running -> pal.success
    NodeLight.Paused -> pal.warning
    NodeLight.Stopped -> pal.danger
}

@Composable
private fun ComputerRow(c: HomeComputer, onRemove: () -> Unit) {
    val p = pal
    val (status, detail) = statusOf(c)
    val shape = RoundedCornerShape(p.rMid)
    Row(
        Modifier.fillMaxWidth().clip(shape).background(p.surface3).border(1.dp, p.line, shape).padding(start = 14.dp, top = 8.dp, bottom = 8.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val answered = c.state != "waiting" && c.state != "refused"
        if (answered) {
            Box(Modifier.size(10.dp).clip(CircleShape).background(lightColor(c.light)))
            Spacer(Modifier.width(12.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(c.name.ifBlank { "Compute node" }, style = type.headline, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(status, style = type.caption.copy(color = if (answered) lightColor(c.light) else p.textDim))
            if (detail != null) Text(detail, style = type.caption)
            if (c.problem.isNotBlank() && plain(c.problem) != status) Text(plain(c.problem), style = type.caption.copy(color = p.warning))
        }
        TextButton(onClick = onRemove) { Text("Remove", color = p.danger) }
    }
}

@Composable
private fun Field(value: String, onChange: (String) -> Unit, hint: String, modifier: Modifier = Modifier, keyboard: KeyboardOptions = KeyboardOptions.Default, onDone: (() -> Unit)? = null) {
    val p = pal
    val shape = RoundedCornerShape(p.rMid)
    Box(modifier.fillMaxWidth().height(54.dp).clip(shape).background(p.surface3).border(1.dp, p.line, shape).padding(horizontal = 16.dp), contentAlignment = Alignment.CenterStart) {
        if (value.isEmpty()) Text(hint, style = type.body.copy(color = p.textFaint))
        BasicTextField(
            value = value, onValueChange = onChange, singleLine = true, textStyle = type.body, cursorBrush = SolidColor(p.accentInk),
            keyboardOptions = keyboard, keyboardActions = KeyboardActions(onDone = { onDone?.invoke() }),
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

/** A compute node the person is being asked to trust. [id] is empty when only an address is known. */
internal data class TrustTarget(val name: String, val id: String, val addr: String)

/** Pick a compute node the phone has seen, or type its address, then say yes to trusting it. A code is the other way in. */
@Composable
fun AddComputerSheet(vm: AppVm, onDismiss: () -> Unit) {
    val p = pal
    val nodes by vm.nodes.collectAsState()
    val pairing by vm.pairing.collectAsState()
    var asking by remember { mutableStateOf<TrustTarget?>(null) }
    var picked by remember { mutableStateOf<FoundComputer?>(null) }
    var addr by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var codeOpen by remember { mutableStateOf(false) }
    val found = unpairedFound(nodes)
    var askingAll by remember { mutableStateOf(false) }
    // With one compute node on the list there is no need to pick it for the code route.
    val target = picked ?: found.singleOrNull()
    val codeReady = code.length == 8 && (target != null || addr.isNotBlank()) && !pairing
    fun submitCode() {
        if (!codeReady) return
        vm.pairNode(addr, if (addr.isBlank()) target?.id.orEmpty() else "", code) { ok -> if (ok) onDismiss() }
    }

    ModalBottomSheet(
        onDismissRequest = { if (!pairing) onDismiss() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = p.surface2, contentColor = p.text, scrimColor = p.scrim,
        shape = RoundedCornerShape(topStart = p.rLarge, topEnd = p.rLarge),
        dragHandle = { SheetHandle() },
    ) {
        Column(Modifier.imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).navigationBarsPadding().padding(bottom = 20.dp)) {
            Text("Add a compute node", style = type.title)
            Spacer(Modifier.height(6.dp))
            Text("Tap a compute node on your network to connect to it.", style = type.callout)

            if (found.size > 1) {
                Spacer(Modifier.height(14.dp))
                PrimaryButton("Connect to all ${found.size} compute nodes", Modifier.fillMaxWidth(), enabled = !pairing) { askingAll = true }
            }

            SectionLabel(if (found.size > 1) "Or choose one" else "Found on this network", Modifier.padding(top = 14.dp), flush = true)
            if (found.isEmpty()) {
                Text("Nothing found yet. Make sure the compute node is on and uses the same Wi-Fi. You can also type its address below.", style = type.caption)
            } else {
                found.forEach { f ->
                    Row(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(p.rMid))
                            .pressable(pressedScale = 0.98f, haptic = HapticFeedbackType.SegmentTick, enabled = !pairing) { asking = TrustTarget(f.name, f.id, f.addrs.firstOrNull().orEmpty()) }
                            .semantics { role = Role.Button }
                            .padding(vertical = 12.dp, horizontal = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(Modifier.weight(1f)) {
                            Text(f.name.ifBlank { "Compute node" }, style = type.headline, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (f.addrs.isNotEmpty()) Text(f.addrs.first(), style = type.caption)
                        }
                    }
                }
            }

            SectionLabel("Or type its address", Modifier.padding(top = 14.dp), flush = true)
            Field(
                addr, { addr = it.take(60) }, "Like 192.168.1.20",
                keyboard = KeyboardOptions(keyboardType = KeyboardType.Uri, capitalization = KeyboardCapitalization.None, imeAction = ImeAction.Go),
                onDone = { if (addr.isNotBlank() && !pairing) asking = TrustTarget("the compute node at ${addr.trim()}", "", addr.trim()) },
            )
            Spacer(Modifier.height(10.dp))
            val canContinue = addr.isNotBlank() && !pairing
            TonalButton("Continue", Modifier.alpha(if (canContinue) 1f else 0.4f)) { if (canContinue) asking = TrustTarget("the compute node at ${addr.trim()}", "", addr.trim()) }

            if (pairing) {
                Spacer(Modifier.height(16.dp))
                Text("Connecting. This takes a few seconds.", style = type.caption)
                Spacer(Modifier.height(6.dp))
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth(), color = p.accent, trackColor = p.surface4)
            }

            Spacer(Modifier.height(18.dp))
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(p.rMid))
                    .pressable(pressedScale = 0.98f, haptic = HapticFeedbackType.SegmentTick) { codeOpen = !codeOpen }
                    .semantics { role = Role.Button }
                    .padding(vertical = 12.dp, horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("Pair with a code instead", style = type.headline, modifier = Modifier.weight(1f))
                Icon(if (codeOpen) Icons.Rounded.ExpandLess else Icons.Rounded.ExpandMore, if (codeOpen) "Hide" else "Show", tint = p.textDim, modifier = Modifier.size(22.dp))
            }
            AnimatedVisibility(codeOpen) {
                Column {
                    Text("On the compute node run: marginalia-node pair", style = type.callout)
                    if (found.size > 1) {
                        SectionLabel("Which compute node", Modifier.padding(top = 12.dp), flush = true)
                        found.forEach { f ->
                            val sel = picked?.id == f.id && picked?.name == f.name
                            Row(
                                Modifier.fillMaxWidth().clip(RoundedCornerShape(p.rMid)).background(if (sel) p.tonal else Color.Transparent)
                                    .pressable(pressedScale = 0.98f, haptic = HapticFeedbackType.SegmentTick) { picked = if (sel) null else f }
                                    .semantics { role = Role.RadioButton; selected = sel }
                                    .padding(vertical = 10.dp, horizontal = 8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Text(f.name.ifBlank { "Compute node" }, style = type.body, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                                if (sel) Icon(Icons.Rounded.Check, "Selected", tint = p.accentInk, modifier = Modifier.size(22.dp))
                            }
                        }
                    }
                    SectionLabel("Code shown on the compute node", Modifier.padding(top = 12.dp), flush = true)
                    Field(
                        code, { code = cleanCode(it) }, "8 letters and numbers",
                        keyboard = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, keyboardType = KeyboardType.Ascii, imeAction = ImeAction.Done),
                        onDone = ::submitCode,
                    )
                    Spacer(Modifier.height(14.dp))
                    if (!pairing && target == null && addr.isBlank()) {
                        Text("Pick a compute node above or type its address.", style = type.caption)
                        Spacer(Modifier.height(8.dp))
                    }
                    PrimaryButton(if (pairing) "Pairing" else "Pair", Modifier.fillMaxWidth(), enabled = codeReady) { submitCode() }
                }
            }
        }
    }

    if (askingAll) {
        TrustTheseDialog(found, onCancel = { askingAll = false }) {
            askingAll = false
            vm.trustAll(found) { ok -> if (ok) onDismiss() }
        }
    }

    asking?.let { t ->
        TrustThisDialog(t, onCancel = { asking = null }) {
            asking = null
            vm.trustNode(if (t.id.isBlank()) t.addr else "", t.id) { ok -> if (ok) onDismiss() }
        }
    }
}

/** The question for several compute nodes at once. */
@Composable
internal fun TrustTheseDialog(found: List<FoundComputer>, onCancel: () -> Unit, onTrust: () -> Unit) {
    val p = pal
    val names = found.map { f -> listOf(f.name.ifBlank { "Compute node" }, shortId(f.id).takeIf { it.isNotBlank() }?.let { "ID $it" }).filterNotNull().joinToString(", ") }
    AlertDialog(
        onDismissRequest = onCancel,
        containerColor = p.surface2,
        title = { Text("Connect to these compute nodes?", style = type.title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                names.forEach { Text(it, style = type.headline) }
                Text("They will help the AI work faster while you are on this network. Your notes and questions are used for the job and are not kept.", style = type.callout)
            }
        },
        confirmButton = { TextButton(onClick = onTrust) { Text("Connect all", color = p.accentInk) } },
        dismissButton = { TextButton(onClick = onCancel) { Text("Not now", color = p.textDim) } },
    )
}

/** The question for one compute node. */
@Composable
internal fun TrustThisDialog(t: TrustTarget, onCancel: () -> Unit, onTrust: () -> Unit) {
    val p = pal
    AlertDialog(
        onDismissRequest = onCancel,
        containerColor = p.surface2,
        title = { Text("Connect to this compute node?", style = type.title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(t.name.ifBlank { "Compute node" }, style = type.headline)
                val where = listOf(t.addr, shortId(t.id).takeIf { it.isNotBlank() }?.let { "ID $it" }).filterNotNull().filter { it.isNotBlank() }.joinToString("  ")
                if (where.isNotBlank()) Text(where, style = type.caption)
                Text("It will help the AI work faster while you are on this network. Your notes and questions are used for the job and are not kept.", style = type.callout)
            }
        },
        confirmButton = { TextButton(onClick = onTrust) { Text("Connect", color = p.accentInk) } },
        dismissButton = { TextButton(onClick = onCancel) { Text("Not now", color = p.textDim) } },
    )
}

/**
 * The banner on the Meetings screen: compute nodes were found and the person has not answered yet.
 * Connect opens the same questions as Settings. It never blocks anything.
 */
@Composable
fun ConnectBanner(vm: AppVm, modifier: Modifier = Modifier) {
    val p = pal
    val offer by vm.connectOffer.collectAsState()
    val pairing by vm.pairing.collectAsState()
    var asking by remember { mutableStateOf(false) }
    // Keep showing the last offer while it fades out, so the text does not jump to empty.
    var last by remember { mutableStateOf(offer) }
    if (offer.isNotEmpty()) last = offer
    AnimatedVisibility(
        visible = offer.isNotEmpty(),
        enter = androidx.compose.animation.fadeIn() + androidx.compose.animation.expandVertically(Motion.soft()),
        exit = androidx.compose.animation.fadeOut() + androidx.compose.animation.shrinkVertically(Motion.soft()),
        modifier = modifier,
    ) {
        val shape = RoundedCornerShape(p.rMid)
        Column(
            Modifier.padding(horizontal = 16.dp).padding(bottom = 10.dp).fillMaxWidth().clip(shape).background(p.surface3).border(1.dp, p.line, shape)
                .padding(start = 14.dp, top = 12.dp, end = 8.dp, bottom = 6.dp),
        ) {
            Text(offerTitle(last.size), style = type.headline)
            Spacer(Modifier.height(2.dp))
            Text("Connect to make the AI faster.", style = type.caption)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = { vm.dismissOffer(last) }) { Text("Not now", color = p.textDim) }
                TextButton(onClick = { asking = true }, enabled = !pairing) { Text("Connect", color = p.accentInk) }
            }
        }
    }
    if (asking) {
        val list = offer
        if (list.isEmpty()) asking = false
        else if (list.size == 1) {
            val f = list[0]
            val t = TrustTarget(f.name, f.id, f.addrs.firstOrNull().orEmpty())
            TrustThisDialog(t, onCancel = { asking = false }) {
                asking = false
                vm.trustNode(if (t.id.isBlank()) t.addr else "", t.id)
            }
        } else {
            TrustTheseDialog(list, onCancel = { asking = false }) {
                asking = false
                vm.trustAll(list)
            }
        }
    }
}

/** A small calm line that says a home compute node is helping. Nothing at all when none is online. */
@Composable
fun HomeComputerChip(vm: AppVm, onClick: () -> Unit, modifier: Modifier = Modifier) {
    val p = pal
    val nodes by vm.nodes.collectAsState()
    val chip = nodeChip(nodes.paired, nodes.enabled)
    var last by remember { mutableStateOf(NodeChip("", NodeLight.Running)) }
    if (chip != null) last = chip
    AnimatedVisibility(
        visible = chip != null,
        enter = androidx.compose.animation.fadeIn(),
        exit = androidx.compose.animation.fadeOut(),
        modifier = modifier,
    ) {
        Row(
            Modifier.padding(horizontal = 20.dp).padding(bottom = 10.dp).height(32.dp).clip(CircleShape).background(p.surface3).border(1.dp, p.line, CircleShape)
                .pressable(onClick = onClick).semantics { role = Role.Button }
                .padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(8.dp).clip(CircleShape).background(lightColor(last.light)))
            Spacer(Modifier.width(8.dp))
            Text(last.text, style = type.caption.copy(color = p.textDim), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** Asks whether another phone may use home compute nodes. One question per phone, wherever the person is in the app. */
@Composable
fun ApprovalPrompt(vm: AppVm) {
    val p = pal
    val ask by vm.approvalAsk.collectAsState()
    ask?.let { a ->
        val phone = a.phone.ifBlank { "A phone" }
        val names = joinNames(a.computers.map { plain(it.name.ifBlank { "this compute node" }) })
        AlertDialog(
            onDismissRequest = { vm.decideNode(a, false) },
            containerColor = p.surface2,
            title = { Text("Let ${plain(phone)} use $names?", style = type.title) },
            text = { Text("It will send notes and questions to ${if (a.computers.size == 1) "this compute node" else "these compute nodes"} so the AI can work faster. Nothing is kept there.", style = type.callout) },
            confirmButton = { TextButton(onClick = { vm.decideNode(a, true) }) { Text("Allow", color = p.accentInk) } },
            dismissButton = { TextButton(onClick = { vm.decideNode(a, false) }) { Text("Not now", color = p.textDim) } },
        )
    }
}
