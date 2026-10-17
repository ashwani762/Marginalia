package app.marginalia.ui

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.AutoAwesome
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.EditNote
import androidx.compose.material.icons.rounded.Lock
import androidx.compose.material.icons.rounded.Mic
import androidx.compose.material.icons.rounded.Notifications
import androidx.compose.material.icons.rounded.PhotoCamera
import androidx.compose.material.icons.rounded.QuestionAnswer
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.launch

/**
 * Shown once, the first time the app opens: what it does, how a meeting goes, and the two permissions it
 * asks for with the reason for each. Nothing here is required to continue except the microphone, and even
 * that can wait until the first recording.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun Onboarding(onDone: () -> Unit) {
    val p = pal
    val ctx = LocalContext.current
    val pager = rememberPagerState { 3 }
    val scope = rememberCoroutineScope()
    fun has(perm: String) = ContextCompat.checkSelfPermission(ctx, perm) == PackageManager.PERMISSION_GRANTED
    var mic by remember { mutableStateOf(has(Manifest.permission.RECORD_AUDIO)) }
    var notes by remember { mutableStateOf(Build.VERSION.SDK_INT < 33 || has(Manifest.permission.POST_NOTIFICATIONS)) }
    val askMic = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { mic = it }
    val askNotes = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { notes = it }

    Box(Modifier.fillMaxSize().background(p.surface)) {
        Aurora(level = { 0f }, modifier = Modifier.fillMaxSize())
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding(), horizontalAlignment = Alignment.CenterHorizontally) {
            Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Spacer(Modifier.weight(1f))
                if (pager.currentPage < 2) Text(
                    "Skip", style = type.label.copy(color = p.textDim),
                    modifier = Modifier.clip(RoundedCornerShape(50)).pressable(onClick = { scope.launch { pager.animateScrollToPage(2) } }).padding(horizontal = 14.dp, vertical = 10.dp),
                )
            }
            HorizontalPager(pager, Modifier.weight(1f).widthIn(max = 560.dp).fillMaxWidth()) { page ->
                Column(Modifier.fillMaxSize().padding(horizontal = 28.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
                    when (page) {
                        0 -> {
                            BrandMark(88.dp)
                            Spacer(Modifier.height(28.dp))
                            Text("Your meetings, written up", style = type.largeTitle, textAlign = TextAlign.Center)
                            Spacer(Modifier.height(12.dp))
                            Text(
                                "Record a meeting and Marginalia listens, writes the notes, and answers questions about what was said. All of it happens on this phone.",
                                style = type.body.copy(color = p.textDim), textAlign = TextAlign.Center,
                            )
                            Spacer(Modifier.height(22.dp))
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Icon(Icons.Rounded.Lock, null, tint = p.success, modifier = Modifier.size(18.dp))
                                Spacer(Modifier.size(8.dp))
                                Text("Nothing leaves this phone", style = type.label.copy(color = p.textDim))
                            }
                        }
                        1 -> {
                            Text("While you listen", style = type.largeTitle, textAlign = TextAlign.Center)
                            Spacer(Modifier.height(22.dp))
                            Tip(Icons.Rounded.EditNote, "Jot", "A few words is enough. The AI finds what was said around it and writes it up properly.")
                            Tip(Icons.Rounded.QuestionAnswer, "Ask the AI", "Ask about what was just said and get the answer while the meeting goes on.")
                            Tip(Icons.Rounded.AutoAwesome, "Questions to ask", "Not sure what to ask the speaker? It suggests three, from what they said.")
                            Tip(Icons.Rounded.PhotoCamera, "Photos", "Snap a slide or a page. The AI reads it and it joins the notes.")
                        }
                        else -> {
                            Text("Two permissions", style = type.largeTitle, textAlign = TextAlign.Center)
                            Spacer(Modifier.height(10.dp))
                            Text("You can change either one later in the phone's settings.", style = type.body.copy(color = p.textDim), textAlign = TextAlign.Center)
                            Spacer(Modifier.height(24.dp))
                            Permission(Icons.Rounded.Mic, "Microphone", "To hear the meeting. It never goes anywhere else.", mic) { askMic.launch(Manifest.permission.RECORD_AUDIO) }
                            if (Build.VERSION.SDK_INT >= 33) {
                                Spacer(Modifier.height(12.dp))
                                Permission(Icons.Rounded.Notifications, "Notifications", "To tell you when the notes are ready, even if the app is closed.", notes) { askNotes.launch(Manifest.permission.POST_NOTIFICATIONS) }
                            }
                        }
                    }
                }
            }
            Row(Modifier.padding(bottom = 18.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                repeat(3) { i ->
                    Box(Modifier.size(if (i == pager.currentPage) 22.dp else 8.dp, 8.dp).clip(CircleShape).background(if (i == pager.currentPage) p.accentInk else p.lineStrong))
                }
            }
            PrimaryButton(
                if (pager.currentPage < 2) "Next" else "Start",
                Modifier.widthIn(max = 420.dp).fillMaxWidth().padding(horizontal = 28.dp).padding(bottom = 22.dp),
            ) {
                if (pager.currentPage < 2) scope.launch { pager.animateScrollToPage(pager.currentPage + 1) } else onDone()
            }
        }
    }
}

@Composable
private fun Tip(icon: ImageVector, title: String, text: String) {
    val p = pal
    Row(Modifier.fillMaxWidth().padding(vertical = 9.dp), verticalAlignment = Alignment.Top) {
        Box(Modifier.size(42.dp).clip(RoundedCornerShape(p.rSmall)).background(p.tonal), contentAlignment = Alignment.Center) {
            Icon(icon, null, tint = p.accentInk, modifier = Modifier.size(22.dp))
        }
        Spacer(Modifier.size(14.dp))
        Column(Modifier.weight(1f)) {
            Text(title, style = type.headline)
            Text(text, style = type.callout)
        }
    }
}

@Composable
private fun Permission(icon: ImageVector, title: String, text: String, granted: Boolean, onAllow: () -> Unit) {
    val p = pal
    val shape = RoundedCornerShape(p.rMid)
    Row(
        Modifier.fillMaxWidth().clip(shape).background(p.surface3).padding(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, null, tint = p.accentInk, modifier = Modifier.size(26.dp))
        Spacer(Modifier.size(14.dp))
        Column(Modifier.weight(1f)) {
            Text(title, style = type.headline)
            Text(text, style = type.caption)
        }
        Spacer(Modifier.size(10.dp))
        if (granted) Icon(Icons.Rounded.CheckCircle, "Allowed", tint = p.success, modifier = Modifier.size(26.dp))
        else TonalButton("Allow", onClick = onAllow)
    }
}
