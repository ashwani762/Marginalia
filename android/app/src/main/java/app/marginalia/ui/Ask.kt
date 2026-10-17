@file:OptIn(ExperimentalLayoutApi::class)

package app.marginalia.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ArrowUpward
import androidx.compose.material.icons.rounded.AutoStories
import androidx.compose.material.icons.rounded.DeleteSweep
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material.icons.rounded.NorthEast
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.marginalia.AppVm
import app.marginalia.Boot
import app.marginalia.ChatTurn
import app.marginalia.Source

@Composable
fun AskScreen(vm: AppVm, onCite: (Source) -> Unit) {
    val p = pal
    val turns by vm.turns.collectAsState()
    val suggestions by vm.suggestions.collectAsState()
    val scope by vm.scope.collectAsState()
    val folders by vm.folders.collectAsState()
    val askFolder by vm.askFolder.collectAsState()
    val boot by vm.boot.collectAsState()
    val ready = boot is Boot.Ready
    val list = rememberLazyListState()
    val haptic = LocalHapticFeedback.current
    var input by rememberSaveable { mutableStateOf("") }
    var showScope by remember { mutableStateOf(false) }
    val busy = turns.any { it.busy }
    val imeVisible = WindowInsets.ime.getBottom(LocalDensity.current) > 0
    val barSpace by animateDpAsState(if (imeVisible || win.rail) 0.dp else 96.dp + MiniBar.height, Motion.soft(), label = "ask-bar")

    // Follow the answer as it streams, unless the reader has scrolled up.
    val last = turns.lastOrNull()
    LaunchedEffect(turns.size, last?.answer?.length?.div(60), last?.state) {
        if (turns.isNotEmpty()) {
            val atEnd = list.layoutInfo.visibleItemsInfo.lastOrNull()?.index?.let { it >= list.layoutInfo.totalItemsCount - 2 } ?: true
            if (atEnd) list.animateScrollToItem(list.layoutInfo.totalItemsCount.coerceAtLeast(1) - 1)
        }
    }

    val send = {
        val q = input.trim()
        if (q.isNotEmpty() && ready && !busy) {
            haptic.performHapticFeedback(HapticFeedbackType.ContextClick)
            vm.ask(q)
            input = ""
        }
    }

    AskScopeHost(vm, showScope) { showScope = false }
    CappedPane(max = 760.dp) {
        Column(Modifier.fillMaxSize().imePadding()) {
            Box(Modifier.weight(1f)) {
                LazyColumn(state = list, contentPadding = PaddingValues(bottom = 12.dp), modifier = Modifier.fillMaxSize()) {
                    item(key = "title") { LargeTitle("Ask", null, showSubtitle = false) }
                    item(key = "sub") {
                        SubtitleRow(if (turns.isEmpty()) "Answers come with their sources" else null)
                    }
                    item(key = "scope") {
                        AskScopeBar(
                            label = scopeLabel(scope, folders.firstOrNull { it.id == askFolder }),
                            onScope = { showScope = true },
                            showClear = turns.isNotEmpty(), onClear = { vm.clearChat() },
                        )
                    }
                    if (turns.isEmpty()) {
                        item(key = "empty") {
                            Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 12.dp)) {
                                if (suggestions.isEmpty()) {
                                    EmptyState(
                                        if (askFolder != null) "Nothing in this folder yet" else "Ask your own knowledge",
                                        if (askFolder != null) "Add meetings or files to this folder and the AI will answer from them."
                                        else "Record a meeting or add a document, then ask about it. Everything is answered on this phone.",
                                    )
                                } else {
                                    Text("TRY ASKING", style = type.micro, modifier = Modifier.padding(bottom = 8.dp))
                                    suggestions.forEach { s -> SuggestionRow(s) { vm.ask(s) } }
                                }
                            }
                        }
                    }
                    items(turns, key = { it.id }) { t -> TurnView(t, onCite, Modifier.animateItem()) }
                }
                TopBarOverlay("Ask", list, Modifier.align(Alignment.TopCenter)) {
                    AnimatedVisibility(turns.isNotEmpty(), enter = fadeIn() + scaleIn(), exit = fadeOut() + scaleOut()) {
                        IconAction(Icons.Rounded.DeleteSweep, "Clear conversation") { vm.clearChat() }
                    }
                }
            }
            Composer(
                value = input, onChange = { input = it }, busy = busy, enabled = ready,
                onSend = send, onStop = vm::stopAnswer,
                modifier = Modifier.padding(bottom = barSpace.coerceAtLeast(0.dp)),
            )
        }
    }
}

@Composable
private fun AskScopeHost(vm: AppVm, show: Boolean, onDismiss: () -> Unit) {
    if (!show) return
    val scope by vm.scope.collectAsState()
    val folders by vm.folders.collectAsState()
    val askFolder by vm.askFolder.collectAsState()
    AskScopeSheet(scope, folders, askFolder, onScope = { vm.scope.value = it }, onFolder = { vm.setAskFolder(it) }, onDismiss = onDismiss)
}

@Composable
private fun SuggestionRow(text: String, onClick: () -> Unit) {
    val p = pal
    val shape = RoundedCornerShape(p.rMid)
    Row(
        Modifier.padding(vertical = 5.dp).fillMaxWidth().clip(shape).background(p.surface3).border(1.dp, p.line, shape)
            .pressable(pressedScale = 0.98f, onClick = onClick).padding(horizontal = 16.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, style = type.body, modifier = Modifier.weight(1f))
        Icon(Icons.Rounded.NorthEast, null, tint = p.accentInk, modifier = Modifier.size(18.dp))
    }
}

@Composable
private fun TurnView(t: ChatTurn, onCite: (Source) -> Unit, modifier: Modifier = Modifier) {
    val p = pal
    Column(modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
        // question
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            Box(
                Modifier.widthIn(max = if (win.twoPane) 460.dp else 300.dp).clip(RoundedCornerShape(20.dp, 20.dp, 6.dp, 20.dp)).background(p.accentBrush).padding(horizontal = 16.dp, vertical = 11.dp),
            ) { Text(t.question, style = type.body.copy(color = p.onAccent)) }
        }
        Spacer(Modifier.height(14.dp))
        // answer
        Column(Modifier.fillMaxWidth().animateContentSize(Motion.soft()).padding(horizontal = 4.dp)) {
            when {
                t.state == "retrieving" || (t.state == "generating" && t.answer.isBlank()) -> Row(verticalAlignment = Alignment.CenterVertically) {
                    ThinkingDots(color = p.accentInk)
                    Spacer(Modifier.width(10.dp))
                    Text(if (t.state == "retrieving") "Searching your knowledge" else "Writing", style = type.caption)
                }
                t.state == "error" -> Text(t.error.ifBlank { "Something went wrong." }, style = type.callout.copy(color = p.danger))
                else -> Markdown(t.answer, onCite = { n -> t.sources.getOrNull(n - 1)?.let(onCite) })
            }
            if (t.sources.isNotEmpty() && t.answer.isNotBlank()) {
                Spacer(Modifier.height(14.dp))
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    t.sources.forEach { s -> SourceChip(s) { onCite(s) } }
                }
            }
            if (t.stats.isNotBlank() && t.state == "done") Text(t.stats, style = type.time, modifier = Modifier.padding(top = 10.dp))
        }
    }
}

@Composable
fun SourceChip(s: Source, onClick: () -> Unit) {
    val p = pal
    Row(
        Modifier.height(34.dp).widthIn(max = 220.dp).clip(RoundedCornerShape(17.dp)).background(p.surface3).border(1.dp, p.line, RoundedCornerShape(17.dp))
            .pressable(pressedScale = 0.95f, onClick = onClick).padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("${s.n}", style = type.label.copy(color = p.accentInk))
        Spacer(Modifier.width(8.dp))
        Text(s.title, style = type.caption, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun Composer(
    value: String,
    onChange: (String) -> Unit,
    busy: Boolean,
    enabled: Boolean,
    onSend: () -> Unit,
    onStop: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val p = pal
    val shape = RoundedCornerShape(26.dp)
    Row(
        modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp)
            .clip(shape).background(p.surface3).border(1.dp, p.line, shape).padding(start = 18.dp, end = 6.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.Bottom,
    ) {
        Box(Modifier.weight(1f).padding(vertical = 10.dp)) {
            if (value.isEmpty()) Text(if (enabled) "Ask anything" else "The AI is waking up…", style = type.body.copy(color = p.textFaint))
            BasicTextField(
                value = value, onValueChange = onChange, enabled = enabled,
                textStyle = type.body, cursorBrush = SolidColor(p.accentInk), maxLines = 5,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Send),
                keyboardActions = androidx.compose.foundation.text.KeyboardActions(onSend = { onSend() }),
                modifier = Modifier.fillMaxWidth().heightIn(max = 120.dp),
            )
        }
        Spacer(Modifier.width(6.dp))
        AnimatedContent(
            targetState = busy,
            transitionSpec = { (fadeIn() + scaleIn(Motion.bouncy())) togetherWith (fadeOut() + scaleOut()) },
            label = "send",
        ) { b ->
            val active = b || value.isNotBlank()
            Box(
                Modifier.size(44.dp).clip(CircleShape).background(if (active && enabled) p.accentBrush else androidx.compose.ui.graphics.Brush.linearGradient(listOf(p.surface4, p.surface4)))
                    .pressable(enabled = enabled && active, pressedScale = 0.88f, onClick = if (b) onStop else onSend),
                contentAlignment = Alignment.Center,
            ) {
                Icon(if (b) Icons.Rounded.Stop else Icons.Rounded.ArrowUpward, if (b) "Stop" else "Send", tint = if (active && enabled) p.onAccent else p.textFaint, modifier = Modifier.size(22.dp))
            }
        }
    }
}
