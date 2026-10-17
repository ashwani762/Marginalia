// Playback of the kept meeting audio in the review screen.

package app.marginalia

import android.media.MediaPlayer
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.File

/** A stretch of kept audio: the file and how many seconds into the meeting it starts. */
data class AudioPart(val path: String, val offset: Double) {
    val exists get() = File(path).isFile
}

/** Plays the kept audio of a meeting from any moment in its transcript. */
class ReviewPlayer(private val scope: CoroutineScope) {
    private var mp: MediaPlayer? = null
    private var parts: List<AudioPart> = emptyList()
    private var index = 0
    private var ticker: Job? = null

    private val _playing = MutableStateFlow(false)
    val playing: StateFlow<Boolean> = _playing.asStateFlow()

    /** Seconds into the meeting where playback is, or null when nothing is loaded. */
    private val _at = MutableStateFlow<Double?>(null)
    val at: StateFlow<Double?> = _at.asStateFlow()

    fun play(all: List<AudioPart>, t: Double) {
        val list = all.filter { it.exists }.sortedBy { it.offset }
        if (list.isEmpty()) return
        parts = list
        index = list.indexOfLast { it.offset <= t + 0.01 }.coerceAtLeast(0)
        open(list[index], ((t - list[index].offset).coerceAtLeast(0.0) * 1000).toInt())
    }

    private fun open(part: AudioPart, fromMs: Int) {
        release()
        try {
            val p = MediaPlayer()
            p.setDataSource(part.path)
            p.prepare()
            p.seekTo(fromMs.coerceAtMost(p.duration.coerceAtLeast(0)))
            p.setOnCompletionListener { next() }
            p.start()
            mp = p
            _playing.value = true
            _at.value = part.offset + fromMs / 1000.0
            ticker = scope.launch(Dispatchers.Main) {
                while (isActive) {
                    mp?.let { m -> if (m.isPlaying) _at.value = parts[index].offset + m.currentPosition / 1000.0 }
                    delay(250)
                }
            }
        } catch (_: Exception) {
            release()
        }
    }

    private fun next() {
        if (index + 1 < parts.size) {
            index += 1
            open(parts[index], 0)
        } else {
            stop()
        }
    }

    fun toggle() {
        val m = mp ?: return
        if (m.isPlaying) { m.pause(); _playing.value = false } else { m.start(); _playing.value = true }
    }

    fun stop() {
        release()
        _playing.value = false
        _at.value = null
    }

    private fun release() {
        ticker?.cancel()
        ticker = null
        runCatching { mp?.release() }
        mp = null
    }
}
