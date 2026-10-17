package app.marginalia

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.platform.LocalView
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.core.view.WindowCompat
import app.marginalia.ui.App
import app.marginalia.ui.MarginaliaTheme
import app.marginalia.ui.isDark

class MainActivity : ComponentActivity() {
    private val vm: AppVm get() = (application as MarginaliaApp).vm

    override fun onCreate(savedInstanceState: Bundle?) {
        installSplashScreen()
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.auto(android.graphics.Color.TRANSPARENT, android.graphics.Color.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.auto(android.graphics.Color.TRANSPARENT, android.graphics.Color.TRANSPARENT),
        )
        setContent {
            val prefs by vm.prefs.collectAsState()
            val dark = isDark(prefs.appearance)
            val view = LocalView.current
            SideEffect {
                val c = WindowCompat.getInsetsController(window, view)
                c.isAppearanceLightStatusBars = !dark
                c.isAppearanceLightNavigationBars = !dark
            }
            App(vm)
        }
        handle(intent)
    }

    override fun onStart() {
        super.onStart()
        AppState.foreground = true
        vm.onForeground()
    }

    override fun onStop() {
        AppState.foreground = false
        super.onStop()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    /** Files, links and text shared from other apps; development hooks in debug builds. */
    @Suppress("DEPRECATION")
    private fun handle(i: Intent?) {
        i ?: return
        when (i.action) {
            Notifier.ACTION_OPEN_MEETING -> {
                val id = i.getLongExtra("meeting_id", 0L)
                if (id > 0) vm.openReview(id)
            }
            Intent.ACTION_SEND -> {
                val stream: Uri? = if (Build.VERSION.SDK_INT >= 33) i.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java) else i.getParcelableExtra(Intent.EXTRA_STREAM)
                val text = i.getStringExtra(Intent.EXTRA_TEXT)
                when {
                    stream != null -> vm.ingestUris(listOf(stream))
                    !text.isNullOrBlank() && text.trim().startsWith("http") -> vm.ingestUrl(text.trim())
                    !text.isNullOrBlank() -> vm.ingestText(text, i.getStringExtra(Intent.EXTRA_SUBJECT) ?: "Shared note")
                }
                if (stream != null || !text.isNullOrBlank()) vm.toast("info", "Added to Knowledge")
            }
            Intent.ACTION_SEND_MULTIPLE -> {
                val list: List<Uri> = (if (Build.VERSION.SDK_INT >= 33) i.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java) else i.getParcelableArrayListExtra(Intent.EXTRA_STREAM)).orEmpty()
                vm.ingestUris(list)
            }
            DEBUG_LITERT -> if (BuildConfig.DEBUG) vm.debugLiteRt(
                i.getStringExtra("model").orEmpty(), i.getStringExtra("backend") ?: "cpu",
                i.getIntExtra("max", 160), i.getBooleanExtra("mtp", false),
            )
            DEBUG_ENGINE -> if (BuildConfig.DEBUG) { vm.setEngine(i.getStringExtra("engine") ?: "llama"); android.util.Log.i("MarginaliaDbg", "engine set; restart the app") }
            DEBUG_LLAMA -> if (BuildConfig.DEBUG) vm.debugLlama(i.getIntExtra("max", 160))
            DEBUG_CALL -> if (BuildConfig.DEBUG) vm.debugCall(i.getStringExtra("method").orEmpty(), i.getStringExtra("args").orEmpty())
            DEBUG_FEED -> if (BuildConfig.DEBUG) vm.debugFeed(i.getStringExtra("path").orEmpty(), i.getFloatExtra("speed", 1f), i.getLongExtra("resume", 0L))
        }
        i.action = null
    }

    companion object {
        const val DEBUG_FEED = "app.marginalia.DEBUG_FEED"
        const val DEBUG_CALL = "app.marginalia.DEBUG_CALL"
        const val DEBUG_LITERT = "app.marginalia.DEBUG_LITERT"
        const val DEBUG_LLAMA = "app.marginalia.DEBUG_LLAMA"
        const val DEBUG_ENGINE = "app.marginalia.DEBUG_ENGINE"
    }
}
