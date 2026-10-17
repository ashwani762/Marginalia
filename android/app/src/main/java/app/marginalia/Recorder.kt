package app.marginalia

import java.io.File
import android.Manifest
import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlin.math.sqrt

/** Live microphone level (0..1) for the waveform, and recorder state for the UI. */
object RecorderBus {
    private val _level = MutableStateFlow(0f)
    val level: StateFlow<Float> = _level
    private val _running = MutableStateFlow(false)
    val running: StateFlow<Boolean> = _running
    private val _error = MutableStateFlow<String?>(null)
    val error: StateFlow<String?> = _error

    internal fun level(v: Float) { _level.value = v }
    fun setLevel(v: Float) { _level.value = v }
    internal fun running(v: Boolean) { _running.value = v }
    internal fun error(v: String?) { _error.value = v }
}

/**
 * Keeps the process alive while recording with the screen off, owns the microphone, and
 * streams 16 kHz PCM16 straight into the native core.
 */
class RecordingService : Service() {
    private var thread: Thread? = null
    @Volatile private var stopping = false
    private var wake: PowerManager.WakeLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_MIC_OFF, ACTION_STOP -> {
                // The notes are written by WorkService from here on.
                stopRecording()
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
            else -> {
                startInForeground()
                startRecording(intent?.getStringExtra(EXTRA_KEEP))
            }
        }
        return START_NOT_STICKY
    }

    private fun startInForeground() {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Recording", NotificationManager.IMPORTANCE_LOW).apply {
            description = "Shown while Marginalia is recording a meeting"
            setShowBadge(false)
        })
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val n: Notification = NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_mic)
            .setContentTitle("Recording a meeting")
            .setContentText("Transcribing on this phone")
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setContentIntent(open)
            .build()
        startForeground(ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
    }

    @SuppressLint("MissingPermission", "WakelockTimeout")
    private fun startRecording(keepTo: String?) {
        if (thread != null) return
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            RecorderBus.error("Microphone permission is needed to record.")
            stopSelf()
            return
        }
        stopping = false
        wake = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "marginalia:recording").apply { acquire() }
        val rate = 16_000
        val minBuf = AudioRecord.getMinBufferSize(rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val frame = rate / 10 // 100 ms
        val rec = AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION, rate,
            AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
            maxOf(minBuf, frame * 2 * 4),
        )
        if (rec.state != AudioRecord.STATE_INITIALIZED) {
            RecorderBus.error("The microphone could not be opened.")
            rec.release()
            stopSelf()
            return
        }
        RecorderBus.error(null)
        val keeper = keepTo?.let { AudioKeeper(File(it)).also { k -> k.start() } }
        thread = Thread({
            android.os.Process.setThreadPriority(android.os.Process.THREAD_PRIORITY_URGENT_AUDIO)
            val buf = ShortArray(frame)
            try {
                rec.startRecording()
                RecorderBus.running(true)
                while (!stopping) {
                    val n = rec.read(buf, 0, buf.size)
                    if (n <= 0) {
                        if (n < 0) break
                        continue
                    }
                    var sum = 0.0
                    for (i in 0 until n) { val v = buf[i] / 32768.0; sum += v * v }
                    RecorderBus.level(minOf(1f, (sqrt(sum / n) * 7.0).toFloat()))
                    Native.pushAudio(buf, n)
                    keeper?.offer(buf, n)
                }
            } finally {
                try { rec.stop() } catch (_: IllegalStateException) {}
                rec.release()
                keeper?.close()
                RecorderBus.level(0f)
                RecorderBus.running(false)
            }
        }, "mic").also { it.start() }
    }

    private fun stopRecording() {
        stopping = true
        thread?.join(1500)
        thread = null
        wake?.let { if (it.isHeld) it.release() }
        wake = null
    }

    override fun onDestroy() {
        stopRecording()
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "recording"
        private const val ID = 7
        const val ACTION_STOP = "app.marginalia.STOP"
        const val ACTION_MIC_OFF = "app.marginalia.MIC_OFF"

        const val EXTRA_KEEP = "keep_to"

        /** [keepTo] is where the audio is also written, or null to keep only the transcript. */
        fun start(context: Context, keepTo: String? = null) {
            ContextCompat.startForegroundService(context, Intent(context, RecordingService::class.java).putExtra(EXTRA_KEEP, keepTo))
        }

        fun micOff(context: Context) {
            context.startService(Intent(context, RecordingService::class.java).setAction(ACTION_MIC_OFF))
        }

        fun stop(context: Context) {
            context.startService(Intent(context, RecordingService::class.java).setAction(ACTION_STOP))
        }
    }
}
