// Background work bookkeeping: the foreground service, restart job and notifications for long jobs.

package app.marginalia

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** True while the app has a visible screen; decides between an in-app message and a notification. */
object AppState {
    private val _flow = MutableStateFlow(false)
    val flow: StateFlow<Boolean> = _flow.asStateFlow()
    var foreground: Boolean
        get() = _flow.value
        set(v) { _flow.value = v }
}

/** Something happening in the background that the person should be able to leave and come back to. */
data class WorkItem(val key: String, val title: String, val detail: String, val progress: Float = -1f)

/** What is being worked on. [WorkService] mirrors it into a notification and keeps the process alive. */
object Work {
    private val _items = MutableStateFlow<Map<String, WorkItem>>(emptyMap())
    val items: StateFlow<Map<String, WorkItem>> = _items.asStateFlow()

    fun set(context: Context, item: WorkItem) {
        val wasEmpty = _items.value.isEmpty()
        _items.update { it + (item.key to item) }
        if (wasEmpty) {
            // Written down first: if Android ends the process, the next start knows there was work.
            Pending.set(context, true)
            RestartJob.schedule(context)
            WorkService.start(context)
        }
    }

    fun done(key: String) {
        _items.update { it - key }
    }

    /** True while anything is being written or read. */
    val busy: Boolean get() = _items.value.isNotEmpty()
}

/** A one-line record that unfinished work may exist, kept across process deaths. */
object Pending {
    private fun prefs(c: Context) = c.getSharedPreferences("work", Context.MODE_PRIVATE)
    fun set(c: Context, on: Boolean) = prefs(c).edit().putBoolean("pending", on).apply()
    fun get(c: Context) = prefs(c).getBoolean("pending", false)
}

/**
 * If Android ends the process while notes are being written, this job brings the app back so the
 * unfinished meeting is picked up again. It survives reboots and is cancelled when the work ends.
 */
class RestartJob : android.app.job.JobService() {
    override fun onStartJob(params: android.app.job.JobParameters?): Boolean {
        // Touching the Application starts the app, and the app resumes unfinished meetings on its own.
        (application as MarginaliaApp).vm.resumeUnfinished()
        if (Pending.get(this)) WorkService.start(this)
        jobFinished(params, false)
        return false
    }

    override fun onStopJob(params: android.app.job.JobParameters?) = true

    companion object {
        private const val ID = 4711

        fun schedule(context: Context) {
            try {
                val js = context.getSystemService(android.app.job.JobScheduler::class.java)
                val job = android.app.job.JobInfo.Builder(ID, android.content.ComponentName(context, RestartJob::class.java))
                    .setPersisted(true)
                    .setPeriodic(15 * 60_000L)
                    .build()
                js.schedule(job)
            } catch (_: Exception) {
            }
        }

        fun cancel(context: Context) {
            try {
                context.getSystemService(android.app.job.JobScheduler::class.java).cancel(ID)
            } catch (_: Exception) {
            }
        }
    }
}

/**
 * Keeps Marginalia alive while notes are being written or files are being read, with a single
 * quiet notification that shows what is going on. It stops itself when the work runs out.
 */
class WorkService : Service() {
    private var scope: CoroutineScope? = null
    private var watcher: Job? = null
    private var wake: android.os.PowerManager.WakeLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        Notifier.channels(this)
        if (wake == null) {
            @Suppress("WakelockTimeout")
            wake = getSystemService(android.os.PowerManager::class.java).newWakeLock(android.os.PowerManager.PARTIAL_WAKE_LOCK, "marginalia:work").apply { acquire(60 * 60_000L) }
        }
        if (intent == null && Work.items.value.isEmpty()) {
            // Android restarted this service after ending the process: hold on while the app starts and
            // finds the meeting that was left half written.
            Work.set(this, WorkItem("resume", "Finishing your notes", "Picking up where it stopped", -1f))
            (application as MarginaliaApp).vm.resumeUnfinished()
        }
        try {
            if (android.os.Build.VERSION.SDK_INT >= 34) {
                startForeground(ID, build(Work.items.value.values.toList()), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
            } else {
                startForeground(ID, build(Work.items.value.values.toList()))
            }
        } catch (e: Exception) {
            // Android refused (not allowed from here, or a time limit): the work carries on while the
            // process lives, and the restart job brings it back if it does not.
            android.util.Log.w("Marginalia", "foreground service refused: $e")
            stopSelf()
            return START_NOT_STICKY
        }
        if (watcher == null) {
            val sc = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate).also { scope = it }
            watcher = sc.launch {
                Work.items.collect { map ->
                    if (map.isEmpty()) {
                        delay(1500) // a short grace so back-to-back jobs do not flicker the notification
                        if (Work.items.value.isEmpty()) {
                            Pending.set(this@WorkService, false)
                            RestartJob.cancel(this@WorkService)
                            stopForeground(STOP_FOREGROUND_REMOVE)
                            stopSelf()
                        }
                    } else {
                        getSystemService(NotificationManager::class.java).notify(ID, build(map.values.toList()))
                    }
                }
            }
        }
        return START_STICKY
    }

    private fun build(items: List<WorkItem>): Notification {
        val first = items.firstOrNull()
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val b = NotificationCompat.Builder(this, Notifier.WORK)
            .setSmallIcon(R.drawable.ic_stat_mic)
            .setContentTitle(first?.title ?: "Working")
            .setContentText(first?.detail.orEmpty() + if (items.size > 1) "  ·  +${items.size - 1} more" else "")
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .setContentIntent(open)
        if (first != null && first.progress in 0f..1f) b.setProgress(100, (first.progress * 100).toInt(), false) else b.setProgress(0, 0, true)
        return b.build()
    }

    override fun onDestroy() {
        wake?.let { if (it.isHeld) it.release() }
        scope?.cancel()
        super.onDestroy()
    }

    companion object {
        private const val ID = 11

        fun start(context: Context) {
            try {
                ContextCompat.startForegroundService(context, Intent(context, WorkService::class.java))
            } catch (_: Exception) {
                // Not allowed from the background on some devices: the work still runs while the app is alive.
            }
        }
    }
}

object Notifier {
    const val WORK = "work"
    const val READY = "ready"
    const val ACTION_OPEN_MEETING = "app.marginalia.OPEN_MEETING"

    fun channels(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(WORK, "Working in the background", NotificationManager.IMPORTANCE_LOW).apply {
            description = "Shown while notes are written or files are read"
            setShowBadge(false)
        })
        nm.createNotificationChannel(NotificationChannel(READY, "Notes ready", NotificationManager.IMPORTANCE_DEFAULT).apply {
            description = "Tells you when a meeting's notes are finished"
        })
    }

    /** "Your notes are ready": tapping it opens the meeting. */
    fun notesReady(context: Context, meetingId: Long, title: String, preview: String) {
        if (!NotificationManagerCompat.from(context).areNotificationsEnabled()) return
        channels(context)
        val open = PendingIntent.getActivity(
            context, meetingId.toInt(),
            Intent(context, MainActivity::class.java).setAction(ACTION_OPEN_MEETING).putExtra("meeting_id", meetingId)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val n = NotificationCompat.Builder(context, READY)
            .setSmallIcon(R.drawable.ic_stat_mic)
            .setContentTitle("Notes ready")
            .setContentText(title.ifBlank { "Your meeting" })
            .setStyle(NotificationCompat.BigTextStyle().bigText(listOf(title, preview).filter { it.isNotBlank() }.joinToString("\n")))
            .setAutoCancel(true)
            .setContentIntent(open)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .build()
        try {
            NotificationManagerCompat.from(context).notify(2000 + meetingId.toInt(), n)
        } catch (_: SecurityException) {
        }
    }

    fun failed(context: Context, title: String) {
        if (!NotificationManagerCompat.from(context).areNotificationsEnabled()) return
        channels(context)
        val n = NotificationCompat.Builder(context, READY)
            .setSmallIcon(R.drawable.ic_stat_mic)
            .setContentTitle("Could not finish the notes")
            .setContentText("${title.ifBlank { "Your meeting" }} was saved. Open Marginalia to try again.")
            .setAutoCancel(true)
            .build()
        try {
            NotificationManagerCompat.from(context).notify(3000, n)
        } catch (_: SecurityException) {
        }
    }
}
