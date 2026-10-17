// Kotlin side of the native core: suspend calls go in as JSON and a stream of events comes back.

package app.marginalia

import android.content.Context
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.util.TimeZone

class BackendException(message: String) : Exception(message)

/** Kotlin face of the native core: suspend calls in, a hot stream of events out. */
class Backend(private val io: CoroutineDispatcher = Dispatchers.Default) {
    private val _events = MutableSharedFlow<JSONObject>(extraBufferCapacity = 1 shl 14, onBufferOverflow = BufferOverflow.DROP_OLDEST)
    val events: SharedFlow<JSONObject> = _events.asSharedFlow()

    private val listener = object : NativeListener {
        override fun onEvent(json: String) {
            try {
                _events.tryEmit(JSONObject(json))
            } catch (_: Exception) {
            }
        }
    }

    fun start(context: Context, modelsDir: String, llmBackend: String = "litert", externalEmbedder: Boolean = false): List<String> {
        Native.load()
        Native.setListener(listener)
        val tz = TimeZone.getDefault().getOffset(System.currentTimeMillis()) / 1000
        val res = JSONObject(Native.init(context.filesDir.absolutePath, modelsDir, tz, llmBackend, externalEmbedder))
        if (res.has("error")) throw BackendException(res.getString("error"))
        val missing = res.optJSONObject("ok")?.optJSONArray("missing") ?: JSONArray()
        return List(missing.length()) { missing.getString(it) }
    }

    /** Runs before any call that may need a model, so an unloaded model is brought back first. */
    var modelGate: (suspend () -> Unit)? = null

    private val readOnly = setOf(
        "engine.state", "engine.gpu", "db.checkpoint", "meeting.audio_add", "settings.get", "settings.set", "meeting.list", "meeting.get", "meeting.live", "meeting.rename",
        "meeting.delete", "meeting.set_folder", "meeting.live_folder", "meeting.jot_delete", "meeting.photo_delete", "meeting.card_delete",
        "kb.docs", "kb.doc", "folders.list", "folders.create", "folders.rename", "folders.delete",
        "nodes.list", "nodes.seen", "nodes.pair", "nodes.trust", "nodes.decide", "nodes.forget", "nodes.enable", "nodes.refresh", "nodes.network",
    )

    suspend fun call(method: String, args: JSONObject = JSONObject()): Any? {
        if (method !in readOnly) modelGate?.invoke()
        return callNow(method, args)
    }

    private suspend fun callNow(method: String, args: JSONObject): Any? = withContext(io) {
        val res = JSONObject(Native.call(method, args.toString()))
        if (res.has("error")) throw BackendException(res.getString("error"))
        res.opt("ok")
    }

    suspend fun obj(method: String, args: JSONObject = JSONObject()): JSONObject = (call(method, args) as? JSONObject) ?: JSONObject()
    suspend fun array(method: String, args: JSONObject = JSONObject()): JSONArray = (call(method, args) as? JSONArray) ?: JSONArray()
}

fun JSONArray.objects(): List<JSONObject> = List(length()) { getJSONObject(it) }
fun JSONObject.strings(key: String): List<String> = optJSONArray(key)?.let { a -> List(a.length()) { a.getString(it) } }.orEmpty()
