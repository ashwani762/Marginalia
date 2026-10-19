// The one view model of the app. It holds the state every screen shows, sends commands to the native core
// through Backend and turns the core's JSON events into that state.

package app.marginalia

import android.app.Application
import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

data class Toast(val kind: String, val message: String, val action: String? = null, val onAction: (() -> Unit)? = null)

/** App state. The native core does the thinking; this holds what the screens show. */
class AppVm(private val app: Application) {
    private val ctx: Context get() = app
    /** Lives as long as the process, not as long as a screen. */
    private val viewModelScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    val backend = Backend()

    // ------------------------------------------------------------------ state shown by the screens

    private val _boot = MutableStateFlow<Boot>(Boot.Loading("Starting", 0, 3))
    val boot: StateFlow<Boot> = _boot.asStateFlow()

    private val _meetings = MutableStateFlow<List<MeetingItem>>(emptyList())
    private val _pendingDelete = MutableStateFlow<MeetingItem?>(null)
    val meetings: StateFlow<List<MeetingItem>> = combine(_meetings, _pendingDelete) { all, gone -> all.filter { it.id != gone?.id } }
        .stateIn(viewModelScope, kotlinx.coroutines.flow.SharingStarted.Eagerly, emptyList())

    private val _live = MutableStateFlow<LiveState?>(null)
    val live: StateFlow<LiveState?> = _live.asStateFlow()
    /** The recording screen is open (true) or tucked into the mini bar (false). */
    val liveExpanded = MutableStateFlow(true)

    private val _review = MutableStateFlow<MeetingDetail?>(null)
    val review: StateFlow<MeetingDetail?> = _review.asStateFlow()

    private val _docs = MutableStateFlow<List<DocItem>>(emptyList())
    val docs: StateFlow<List<DocItem>> = _docs.asStateFlow()
    private val _ingest = MutableStateFlow<List<IngestJob>>(emptyList())
    val ingest: StateFlow<List<IngestJob>> = _ingest.asStateFlow()
    private val _passages = MutableStateFlow<List<Source>?>(null)
    val passages: StateFlow<List<Source>?> = _passages.asStateFlow()

    private val _turns = MutableStateFlow<List<ChatTurn>>(emptyList())
    val turns: StateFlow<List<ChatTurn>> = _turns.asStateFlow()
    private val _suggestions = MutableStateFlow<List<String>>(emptyList())
    val suggestions: StateFlow<List<String>> = _suggestions.asStateFlow()
    val scope = MutableStateFlow("all")
    private var pendingQuestion = ""

    private val _stats = MutableStateFlow(Stats())
    val stats: StateFlow<Stats> = _stats.asStateFlow()
    private val _prefs = MutableStateFlow(Prefs())
    val prefs: StateFlow<Prefs> = _prefs.asStateFlow()

    private val _toasts = MutableSharedFlow<Toast>(extraBufferCapacity = 8)
    val toasts: SharedFlow<Toast> = _toasts.asSharedFlow()

    val modelInfo = MutableStateFlow("")

    /**
     * The loading screen shown while Gemma loads at app start, so the pause happens behind a still screen and not on the main
     * screen. When the last time the phone had no compute node (or the app has never run), it is up from the very first frame, so
     * the main screen never shows first and then jumps away.
     */
    private val _aiWarm = MutableStateFlow<AiWarm?>(
        if (app.getSharedPreferences("engine", Context.MODE_PRIVATE).getBoolean("last_alone", true)) AiWarm("Getting the AI ready", 0f) else null,
    )
    val aiWarm: StateFlow<AiWarm?> = _aiWarm.asStateFlow()
    @Volatile private var warmSkipped = false

    // ---- background write-ups: meeting id -> where it is
    private val _finishing = MutableStateFlow<Map<Long, WrapState>>(emptyMap())
    val finishing: StateFlow<Map<Long, WrapState>> = _finishing.asStateFlow()
    /** True while background notes wait for a recording to end (so transcription is never slowed). */
    val holdFinishing = MutableStateFlow(false)
    /** Screens listen for "go to Meetings" after a recording ends. */
    val navigate = MutableSharedFlow<Int>(extraBufferCapacity = 4)

    // ---- folders
    private val _folders = MutableStateFlow<List<Folder>>(emptyList())
    val folders: StateFlow<List<Folder>> = _folders.asStateFlow()
    /** null = everything, -1 = not in a folder, otherwise a folder id. */
    val meetingsFilter = MutableStateFlow<Long?>(null)
    val knowledgeFilter = MutableStateFlow<Long?>(null)
    /** Folder Ask draws from (null = everything). */
    val askFolder = MutableStateFlow<Long?>(null)
    /** The folder new recordings and additions go to (null = none). */
    val liveFolder = MutableStateFlow<Long?>(null)

    init {
        viewModelScope.launch(Dispatchers.Default) { start() }
    }

    fun toast(kind: String, message: String, action: String? = null, onAction: (() -> Unit)? = null) {
        _toasts.tryEmit(Toast(kind, message, action, onAction))
    }

    // ------------------------------------------------------------------ boot

    // What each half of startup has reached; the app is Ready only when both are.
    private var nativeBoot: Boot = Boot.Loading("Starting", 0, 3)
    private var kotlinLabel: String? = "Starting"
    @Volatile private var failure: String? = null

    private fun publishBoot() {
        val f = failure
        _boot.value = when {
            f != null -> Boot.Failed(f)
            _boot.value is Boot.Unpacking && nativeBoot !is Boot.Ready -> _boot.value
            nativeBoot is Boot.Failed -> nativeBoot
            kotlinLabel != null -> Boot.Loading(kotlinLabel!!, (nativeBoot as? Boot.Loading)?.step ?: 0, 3)
            else -> nativeBoot
        }
    }

    private suspend fun start() {
        try {
            if (BuildConfig.DEBUG && File(ctx.filesDir, "skip-engine").exists()) { _boot.value = Boot.Ready; return }
            val prefs = ctx.getSharedPreferences("engine", Context.MODE_PRIVATE)
            showCachedLists()

            // 1. Files. The first launch unpacks the models from the APK; later launches skip this.
            val missing = ModelInstaller.missing(ctx)
            if (missing.isNotEmpty()) {
                if (!ModelInstaller.canInstall(ctx)) {
                    _boot.value = Boot.Failed(
                        "The on-device models are not in this build.\nPush them with scripts/app/push-models.sh, then reopen the app.\n\nMissing: ${missing.joinToString()}",
                    )
                    return
                }
                _boot.value = Boot.Unpacking(0, 1)
                ModelInstaller.install(ctx) { done, total, _ -> _boot.value = Boot.Unpacking(done, total) }
            }

            // 2. A restore waiting from last time takes effect before anything opens the database.
            if (Backup.hasStaged(ctx)) Backup.applyStaged(ctx)

            // 2b. Open the database and show the meetings at once. Nothing here waits for a model.
            _boot.value = Boot.Loading("Starting", 0, 3)
            viewModelScope.launch(Dispatchers.Default) { backend.events.collect { handle(it) } }
            delay(50)
            Native.load()
            withContext(loaderThread) { backend.start(ctx, ModelInstaller.dir(ctx).absolutePath, "litert", true) }
            refreshAll()

            // 3. Gemma and EmbeddingGemma on LiteRT-LM, in the background.
            nativeReady = true
            backend.modelGate = { ensureModels() }
            viewModelScope.launch(Dispatchers.Default) { ensureModels(); refreshAll() }
            watchIdle()
            refreshPrefs()
        } catch (e: Exception) {
            failure = e.message ?: e.toString()
            publishBoot()
        }
    }

    private var engineNote = ""

    // ------------------------------------------------------------------ model life

    @Volatile private var nativeReady = false
    private val modelLock = kotlinx.coroutines.sync.Mutex()
    private var chatModel: LiteRtGenerator? = null
    private var embedModel: LiteRtEmbedder? = null
    @Volatile private var modelsLoaded = false

    /**
     * Brings the search model into memory and puts the stand-in for Gemma in place. Cheap when they are there.
     * Gemma itself loads when the phone has to write something (see [ensureChat]). With a compute node in reach
     * that is never, and the loading of Gemma on the GPU is what made the screen stall for seconds.
     */
    suspend fun ensureModels() {
        if (modelsLoaded) return
        modelLock.withLock {
            if (!modelsLoaded) loadLight()
        }
    }

    /** Loads Gemma now if it is not loaded. Returns false when it could not start. */
    suspend fun ensureChat(): Boolean {
        if (chatModel != null) return true
        modelLock.withLock {
            if (!modelsLoaded) loadLight()
            if (chatModel == null) loadChat()
        }
        return chatModel != null
    }

    /** Stands in for Gemma until the first job that has to run on the phone, then hands that job to the real one. */
    private val lazyChat = object : LlmGenerator {
        private val stopped = java.util.concurrent.ConcurrentHashMap.newKeySet<Long>()
        override fun start(request: Long, json: String) {
            viewModelScope.launch(Dispatchers.Default) {
                val ready = ensureChat()
                val real = chatModel
                when {
                    request in stopped -> { stopped.remove(request); Native.emit(request, 1, "{\"prompt_tokens\":0,\"generated_tokens\":0}") }
                    !ready || real == null -> Native.emit(request, 2, "The AI could not start")
                    else -> real.start(request, json)
                }
            }
        }
        override fun cancel(request: Long) {
            val real = chatModel
            if (real != null) real.cancel(request) else stopped.add(request)
        }
    }

    /**
     * Everything that loads a model runs on one thread at the lowest priority. The threads the model libraries
     * start inherit it, so the screen keeps its frames and the microphone keeps its turn while they warm up.
     */
    private val loaderThread by lazy { java.util.concurrent.Executors.newSingleThreadExecutor { r ->
        Thread({
            android.os.Process.setThreadPriority(android.os.Process.THREAD_PRIORITY_BACKGROUND)
            r.run()
        }, "model-loader")
    }.asCoroutineDispatcher() }

    /** Loads the search model, which stays on the processor and is quick, and sets up the stand-in for Gemma. */
    private suspend fun loadLight() {
        val prefs = ctx.getSharedPreferences("engine", Context.MODE_PRIVATE)
        kotlinLabel = "Loading the search model"
        publishBoot()
        val embedGpu = runCatching { java.util.Properties().apply { File(ctx.filesDir, "tune.properties").takeIf { it.isFile }?.inputStream()?.use { load(it) } }.getProperty("embedGpu") == "true" }.getOrDefault(false)
        var emb = LiteRtEmbedder(ctx, File(ModelInstaller.dir(ctx), ModelInstaller.EMBED_LITERT).absolutePath, gpu = embedGpu, threads = 4)
        var eerr = withContext(loaderThread) { emb.prepare() }
        if (eerr != null && embedGpu) {
            emb = LiteRtEmbedder(ctx, File(ModelInstaller.dir(ctx), ModelInstaller.EMBED_LITERT).absolutePath, gpu = false)
            eerr = withContext(loaderThread) { emb.prepare() }
        }
        if (eerr != null) { failure = "The search model could not start: $eerr"; publishBoot(); return }
        // The first embedding is the slow one (it builds its caches). Do it now, with the other warm up, not at the first question.
        withContext(loaderThread) { runCatching { emb.embed(arrayOf("warm up")) } }
        Native.setEmbedder(emb)
        Native.setGenerator(lazyChat)
        embedModel = emb
        modelsLoaded = true
        modelInfo.value = "Gemma 4 ${if (gemmaFile() == ModelDownload.E4B_FILE) "E4B" else "E2B"} loads when the phone needs it · EmbeddingGemma 2 on ${emb.label} · Whisper small · ${Native.variant}"
        kotlinLabel = null
        publishBoot()
    }

    /** Loads Gemma, on the GPU first (it reads a long prompt many times faster and leaves the processor to speech), then the CPU. */
    private suspend fun loadChat() {
        val prefs = ctx.getSharedPreferences("engine", Context.MODE_PRIVATE)
        val wantedGpu = prefs.getString("chat", "auto") != "litert-cpu" && !prefs.getBoolean("gpu_failed", false)
        val notes = mutableListOf<String>()
        var chat: LiteRtGenerator? = null
        for (gpu in if (wantedGpu) listOf(true, false) else listOf(false)) {
            kotlinLabel = if (gpu) "Loading the AI on the GPU" else "Loading the AI"
            publishBoot()
            val g = LiteRtGenerator(ctx, File(ModelInstaller.dir(ctx), gemmaFile()).absolutePath, gpu, speedUp.value)
            val err = withContext(loaderThread) { g.prepare() }
            if (err == null) { chat = g; break }
            notes += "The AI on the ${if (gpu) "GPU" else "CPU"}: $err"
            if (gpu) prefs.edit().putBoolean("gpu_failed", true).apply()
        }
        if (chat == null) { failure = "The AI could not start.\n" + notes.joinToString("\n"); publishBoot(); return }
        // The first conversation sets up the image reader, which stalls the screen. Do it now, behind the loading screen.
        kotlinLabel = "Getting the AI ready"
        publishBoot()
        withContext(loaderThread) { chat.warmUp() }
        chatModel = chat
        engineNote = if (notes.isNotEmpty()) "The GPU was not usable, so the AI runs on the CPU." else ""
        modelInfo.value = "Gemma 4 ${if (gemmaFile() == ModelDownload.E4B_FILE) "E4B" else "E2B"} on ${chat.label} · EmbeddingGemma 2 on ${embedModel?.label ?: "CPU"} · Whisper small · ${Native.variant}" +
            (if (engineNote.isNotEmpty()) "\n$engineNote" else "")
        kotlinLabel = null
        publishBoot()
    }

    /** Loads everything now, Gemma included. Used when a setting changes how Gemma loads, to find out at once whether it fits. */
    private suspend fun loadModels() {
        loadLight()
        loadChat()
    }

    /** Keep the recording itself next to the transcript (about 11 MB an hour). */
    val keepAudio = MutableStateFlow(enginePrefs.getBoolean("keep_audio", true))
    fun setKeepAudio(on: Boolean) { keepAudio.value = on; enginePrefs.edit().putBoolean("keep_audio", on).apply() }

    /** Starts the microphone service, and writes the audio too when that is switched on. */
    private suspend fun startMic(meetingId: Long, offsetSecs: Double) {
        var keep: String? = null
        if (keepAudio.value) {
            val f = AudioKeeper.newFile(ctx, meetingId)
            runCatching { backend.call("meeting.audio_add", JSONObject().put("id", meetingId).put("path", f.absolutePath).put("offset", offsetSecs)) }
                .onSuccess { keep = f.absolutePath }
        }
        RecordingService.start(ctx, keep)
    }

    // ---- speakers
    /** Meetings whose speakers are being told apart right now, with how far along (0 to 1). */
    private val _separating = MutableStateFlow<Map<Long, Float>>(emptyMap())
    val separating: StateFlow<Map<Long, Float>> = _separating.asStateFlow()

    /** Tells the speakers of a meeting apart from its kept audio. [people] is how many spoke, or 0 to let it decide. */
    fun separateSpeakers(meetingId: Long, people: Int = 0, threshold: Float = Diarizer.THRESHOLD) {
        val d = _review.value?.takeIf { it.meeting.id == meetingId } ?: return
        if (_separating.value.containsKey(meetingId)) return
        if (!d.audio.any { it.exists }) { toast("info", "This meeting has no kept recording to listen to."); return }
        viewModelScope.launch(Dispatchers.Default) {
            val key = "speakers$meetingId"
            fun set(label: String, f: Float) {
                _separating.update { it + (meetingId to f) }
                Work.set(ctx, WorkItem(key, "Telling speakers apart", label, f))
            }
            try {
                set("Starting", 0f)
                if (!Diarizer.ready(ctx)) {
                    Diarizer.fetchModels(ctx) { f -> set("Getting the speaker model", f * 0.2f) }
                }
                val spans = Diarizer.run(ctx, d.audio, people, { f -> set("Listening", 0.2f + f * 0.8f) }, threshold)
                val arr = JSONArray()
                spans.forEach { arr.put(JSONObject().put("start", it.start).put("end", it.end).put("speaker", it.speaker)) }
                val r = backend.obj("meeting.speakers_apply", JSONObject().put("id", meetingId).put("segments", arr))
                openReview(meetingId)
                val n = r.optInt("speakers")
                toast("success", if (n <= 1) "It heard one voice." else "It heard $n voices. Tap a name to rename it.")
            } catch (e: Throwable) {
                toast("error", e.message ?: "Could not tell the speakers apart.")
            } finally {
                _separating.update { it - meetingId }
                Work.done(key)
            }
        }
    }

    /**
     * Asks a compute node with a larger speech model to listen to the kept recording again, then has the notes and
     * the summary written again from what it heard. Progress arrives as the same wrap events as the first write-up,
     * and the end as a wrap_done event (see the handler), which reloads the meeting and says it is done.
     */
    fun improveTranscript(meetingId: Long) {
        if (_finishing.value.containsKey(meetingId)) return
        viewModelScope.launch {
            _finishing.update { it + (meetingId to WrapState("improving", "Improving the transcript", 2f)) }
            try {
                backend.call("meeting.improve_transcript", JSONObject().put("id", meetingId))
            } catch (e: Exception) {
                _finishing.update { it - meetingId }
                toast("info", e.message ?: "Could not improve the transcript.")
            }
        }
    }

    fun renameSpeaker(meetingId: Long, speaker: Int, name: String) {
        viewModelScope.launch {
            runCatching { backend.call("meeting.speaker_rename", JSONObject().put("id", meetingId).put("speaker", speaker).put("name", name)) }
            openReview(meetingId)
        }
    }

    fun mergeSpeakers(meetingId: Long, from: Int, into: Int) {
        viewModelScope.launch {
            runCatching { backend.call("meeting.speaker_merge", JSONObject().put("id", meetingId).put("from", from).put("to", into)) }
            openReview(meetingId)
        }
    }

    fun clearSpeakers(meetingId: Long) {
        viewModelScope.launch {
            runCatching { backend.call("meeting.speakers_clear", JSONObject().put("id", meetingId)) }
            openReview(meetingId)
        }
    }

    // ---- first run
    /** False until the person has been through the short introduction once. */
    val introduced = MutableStateFlow(ctx.getSharedPreferences("engine", Context.MODE_PRIVATE).getBoolean("introduced", false))
    fun finishIntro() {
        introduced.value = true
        ctx.getSharedPreferences("engine", Context.MODE_PRIVATE).edit().putBoolean("introduced", true).apply()
    }

    // ---- backup
    fun exportBackup(uri: Uri) {
        viewModelScope.launch {
            Work.set(ctx, WorkItem("backup", "Saving your backup", "Everything stays in the file you chose", -1f))
            try {
                val bytes = Backup.export(ctx, backend, uri) { pct -> Work.set(ctx, WorkItem("backup", "Saving your backup", "$pct percent", pct / 100f)) }
                toast("success", "Backup saved (${bytes shr 20} MB).")
            } catch (e: Exception) {
                toast("error", e.message ?: "The backup could not be saved.")
            } finally {
                Work.done("backup")
            }
        }
    }

    /** Checks a backup file, then restarts the app so it can take over. */
    fun restoreBackup(uri: Uri) {
        viewModelScope.launch {
            if (_live.value != null || Work.busy) { toast("info", "Try again when nothing is recording or being written."); return@launch }
            try {
                Backup.stage(ctx, uri)
                toast("info", "Restoring. The app will restart.")
                delay(1200)
                Backup.restart(ctx)
            } catch (e: Exception) {
                toast("error", e.message ?: "That backup could not be read.")
            }
        }
    }

    // ---- faster answers
    /** Gemma drafts a few words ahead and checks them, which reads faster on the GPU (about 12 against 9 words a second here). */
    val speedUp = MutableStateFlow(enginePrefs.getBoolean("mtp", true))

    /** Applies at once by reloading Gemma, so it waits until nothing is being written. */
    fun setSpeedUp(on: Boolean) {
        if (on == speedUp.value) return
        if (Work.busy || _live.value != null || _finishing.value.isNotEmpty()) {
            toast("info", "Try again when the notes are finished.")
            return
        }
        speedUp.value = on
        enginePrefs.edit().putBoolean("mtp", on).apply()
        viewModelScope.launch(Dispatchers.Default) {
            modelLock.withLock { unloadLocked(); loadModels() }
        }
    }

    // ---- which Gemma
    private val enginePrefs get() = ctx.getSharedPreferences("engine", Context.MODE_PRIVATE)
    /** "e2b" (smaller, faster, the default) or "e4b" (larger, a little sharper, needs about 4 GB more space). */
    val gemmaChoice = MutableStateFlow(enginePrefs.getString("gemma", "e2b") ?: "e2b")
    sealed interface E4b {
        data object Absent : E4b
        data class Downloading(val done: Long, val total: Long) : E4b
        data object Ready : E4b
        data class Failed(val message: String) : E4b
    }
    val e4b = MutableStateFlow<E4b>(if (ModelDownload.have(ctx, ModelDownload.E4B_FILE)) E4b.Ready else E4b.Absent)
    private var e4bJob: Job? = null

    private fun gemmaFile(): String =
        if (gemmaChoice.value == "e4b" && ModelDownload.have(ctx, ModelDownload.E4B_FILE)) ModelDownload.E4B_FILE else ModelInstaller.LITERT_MODEL

    fun downloadE4b() {
        if (e4bJob?.isActive == true) return
        e4b.value = E4b.Downloading(ModelDownload.partial(ctx), ModelDownload.E4B_BYTES)
        Work.set(ctx, WorkItem("e4b", "Downloading the larger Gemma AI", "Stays on this phone", 0f))
        e4bJob = viewModelScope.launch {
            try {
                ModelDownload.fetchE4b(ctx) { d, t ->
                    e4b.value = E4b.Downloading(d, t)
                    Work.set(ctx, WorkItem("e4b", "Downloading the larger Gemma AI", "${d shr 20} of ${t shr 20} MB", d.toFloat() / t))
                }
                e4b.value = E4b.Ready
                toast("success", "The larger Gemma AI is ready to use.")
            } catch (e: kotlinx.coroutines.CancellationException) {
                e4b.value = if (ModelDownload.have(ctx, ModelDownload.E4B_FILE)) E4b.Ready else E4b.Absent
            } catch (e: Exception) {
                e4b.value = E4b.Failed(e.message ?: "The download failed.")
            } finally {
                Work.done("e4b")
            }
        }
    }

    fun cancelE4b() { e4bJob?.cancel() }

    fun removeE4b() {
        if (gemmaChoice.value == "e4b") useGemma("e2b")
        ModelDownload.delete(ctx)
        e4b.value = E4b.Absent
    }

    /** Switches Gemma. It waits until nothing is being written, because the model reloads. */
    fun useGemma(choice: String) {
        if (choice == gemmaChoice.value) return
        if (choice == "e4b" && !ModelDownload.have(ctx, ModelDownload.E4B_FILE)) return
        if (Work.busy || _live.value != null || _finishing.value.isNotEmpty()) {
            toast("info", "Try again when the notes are finished.")
            return
        }
        viewModelScope.launch(Dispatchers.Default) {
            val before = gemmaChoice.value
            gemmaChoice.value = choice
            enginePrefs.edit().putString("gemma", choice).apply()
            modelLock.withLock {
                swapping = true
                try {
                    unloadLocked()
                    loadModels()
                } finally { swapping = false }
            }
            if (failure != null) {
                // The larger model did not fit: go back to the smaller one rather than leave the app without Gemma.
                failure = null
                publishBoot()
                gemmaChoice.value = before
                enginePrefs.edit().putString("gemma", before).apply()
                modelLock.withLock { unloadLocked(); loadModels() }
                toast("warning", "There was not enough memory for that one, so the smaller Gemma AI is back.")
            }
        }
    }
    @Volatile private var swapping = false

    private fun unloadLocked() {
        Native.setGenerator(null)
        Native.setEmbedder(null)
        chatModel?.close()
        embedModel?.close()
        chatModel = null
        embedModel = null
        modelsLoaded = false
    }

    /** Frees the models' memory. Only ever called when nothing is using them. */
    private suspend fun offloadModels() {
        modelLock.withLock {
            if (!modelsLoaded || isBusy()) return
            unloadLocked()
            android.util.Log.i("Marginalia", "models unloaded: nothing left to do")
        }
    }

    private fun isBusy(): Boolean =
        Work.busy || AppState.foreground || _live.value != null || _finishing.value.isNotEmpty() ||
            _ingest.value.any { it.active } || _turns.value.any { it.busy }

    /**
     * The models stay loaded for as long as anything is being written or read, and for ten minutes
     * after the last of it ends. Then they go, and come back when the app is opened again.
     */
    private fun watchIdle() {
        viewModelScope.launch(Dispatchers.Default) {
            kotlinx.coroutines.flow.combine(AppState.flow, Work.items, _live, _finishing) { fg, w, l, f -> !fg && w.isEmpty() && l == null && f.isEmpty() }
                .collectLatest { idle ->
                    if (idle) {
                        // Ten minutes: loading Gemma on the GPU stalls the screen for seconds, so it is not dropped after a short pause.
                        delay(600_000)
                        offloadModels()
                    }
                }
        }
    }

    /** The app came to the front: make sure the models are there and pick up anything left unfinished. */
    fun onForeground() {
        discovery.start()
        startNodeWatch()
        loadChatIfAlone()
        viewModelScope.launch(Dispatchers.Default) {
            if (nativeReady) ensureModels()
        }
        resumeUnfinished()
    }

    /** The window left the screen: stop listening for home compute nodes. */
    fun onBackground() {
        // While notes are being written in the background the compute nodes are still looked for, because the
        // write-up may be running on one of them and one may have moved to a new address.
        if (!Work.busy) discovery.stop()
        nodeWatch?.cancel(); nodeWatch = null
    }

    /** Finishes meetings whose notes were never completed (the app was ended mid-way, or a step failed). */
    fun resumeUnfinished() {
        viewModelScope.launch(Dispatchers.Default) {
            var waited = 0
            while (!nativeReady && waited < 120_000) { delay(500); waited += 500 }
            if (!nativeReady) return@launch
            runCatching { backend.call("meeting.recover", JSONObject().put("grace", if (AppState.foreground) 90 else 0)) }
            // The placeholder from a restarted service must not linger if there turns out to be nothing to do.
            delay(90_000)
            Work.done("resume")
        }
    }

    /** Chooses how Gemma runs: "auto", "litert-cpu" or "litert-gpu". Takes effect on the next start. */
    fun setEngine(name: String) {
        ctx.getSharedPreferences("engine", Context.MODE_PRIVATE).edit().putString("chat", name).remove("gpu_failed").apply()
    }

    fun retryBoot() {
        failure = null
        kotlinLabel = "Starting"
        _boot.value = Boot.Loading("Starting", 0, 3)
        viewModelScope.launch(Dispatchers.Default) { start() }
    }

    // ------------------------------------------------------------------ events

    private fun handle(e: JSONObject) {
        when (e.optString("type")) {
            "engine" -> {
                when (e.optString("state")) {
                    "loading" -> nativeBoot = Boot.Loading(e.optString("label"), e.optInt("step"), e.optInt("steps", 3))
                    "ready" -> nativeBoot = Boot.Ready
                    "failed" -> nativeBoot = Boot.Failed(e.optString("error"))
                }
                publishBoot()
                if (nativeBoot is Boot.Ready && kotlinLabel == null && failure == null) refreshAll()
            }
            "toast" -> toast(e.optString("kind"), e.optString("message"))
            "tick" -> _live.update { it?.copy(secs = e.optLong("secs")) }
            "partial" -> _live.update { it?.copy(partial = e.optString("text")) }
            "line" -> {
                val line = Line.from(e.getJSONObject("line"))
                // A line is found by its id, never by its position: positions shift when the transcript is corrected,
                // and a line that grows (words merged into it) is replaced, so a missing earlier line cannot make a duplicate.
                _live.update { s -> s?.copy(lines = s.lines.upsertLine(line)) }
            }
            "lines_replace" -> {
                val new = Line.list(e.optJSONArray("lines"))
                val from = e.optInt("from")
                val removed = e.optInt("removed")
                var fresh = emptySet<Long>()
                _live.update { s ->
                    s?.let {
                        // Only lines whose words really changed are highlighted.
                        val before = it.lines.drop(from).take(removed).joinToString(" ") { l -> l.text }.split(Regex("\\s+")).filter { w -> w.isNotEmpty() }
                        val after = new.joinToString(" ") { l -> l.text }.split(Regex("\\s+")).filter { w -> w.isNotEmpty() }
                        fresh = if (before == after) emptySet() else new.map { l -> l.id }.toSet()
                        it.copy(lines = it.lines.replacingLines(from, removed, new), freshFixes = it.freshFixes + fresh)
                    }
                }
                if (fresh.isNotEmpty()) viewModelScope.launch {
                    delay(4000)
                    _live.update { s -> s?.copy(freshFixes = s.freshFixes - fresh) }
                }
            }
            "correction_state" -> _live.update {
                it?.copy(
                    correctionPhase = e.optString("phase", "idle"),
                    workFrom = e.optDouble("from", it.workFrom), workTo = e.optDouble("to", it.workTo),
                    checkedUpto = if (e.has("upto")) e.optDouble("upto") else it.checkedUpto,
                )
            }
            "correction" -> _live.update { it?.copy(correctedBy = if (e.isNull("node")) null else e.optString("node")) }
            "enhance" -> _live.update { it?.copy(enhanced = e.optString("text"), enhanceStreaming = e.optBoolean("streaming"), enhanceBusy = e.optBoolean("streaming")) }
            "enhance_state" -> _live.update { it?.copy(enhanceBusy = e.optBoolean("busy")) }
            "card" -> {
                val card = Card.from(e.getJSONObject("card"))
                _live.update { s ->
                    s?.let {
                        val idx = it.cards.indexOfFirst { c -> c.id == card.id }
                        val cards = if (idx >= 0) it.cards.toMutableList().also { l -> l[idx] = card } else listOf(card) + it.cards
                        it.copy(cards = cards, newAssist = it.newAssist || (idx < 0 && card.state != CardState.NoMatch))
                    }
                }
            }
            "photo" -> {
                val photo = Photo.from(e.getJSONObject("photo"))
                val mid = e.optLong("meeting_id")
                _live.update { st -> if (st != null && st.meetingId == mid) st.copy(photos = st.photos.upsert(photo)) else st }
                _review.update { d -> if (d != null && d.meeting.id == mid) d.copy(photos = d.photos.upsert(photo)) else d }
            }
            "jot" -> {
                val jot = Jot.from(e.getJSONObject("jot"))
                val mid = e.optLong("meeting_id")
                _live.update { st -> if (st != null && st.meetingId == mid) st.copy(jots = st.jots.upsertJot(jot)) else st }
                _review.update { d -> if (d != null && d.meeting.id == mid) d.copy(jots = d.jots.upsertJot(jot)) else d }
            }
            "activity" -> {
                val a = e.optJSONArray("items")
                _live.update { it?.copy(activity = if (a == null) emptyList() else List(a.length()) { i -> a.getString(i) }) }
            }
            "card_remove" -> _live.update { s -> s?.copy(cards = s.cards.filter { it.id != e.optLong("id") }) }
            "wrap_hold" -> holdFinishing.value = e.optBoolean("waiting")
            "reindex" -> {
                val done = e.optInt("done"); val total = e.optInt("total")
                Work.set(ctx, WorkItem("reindex", "Updating search", "Reading your library again", if (total > 0) done.toFloat() / total else -1f))
            }
            "reindex_done" -> Work.done("reindex")
            "recovery" -> {
                if (e.optInt("pending") == 0) Work.done("resume")
                val cut = e.optJSONArray("interrupted")
                if (cut != null && cut.length() > 0 && AppState.foreground && _live.value == null) {
                    val id = cut.getLong(cut.length() - 1)
                    toast("warning", "A recording stopped early. Its notes will be written in a moment.", "Continue") { continueRecording(id) }
                }
            }
            "wrap" -> {
                Work.done("resume")
                val id = e.optLong("meeting_id")
                // The recording behind the live screen has ended (stopped from somewhere other than this screen).
                if (_live.value?.meetingId == id && _live.value?.recording == true) _live.value = null
                val w = WrapState(e.optString("step"), e.optString("label"), e.optDouble("progress").toFloat())
                _finishing.update { it + (id to w) }
                Work.set(ctx, WorkItem("meeting$id", "Writing your meeting notes", w.label, w.progress / 100f))
            }
            "wrap_done" -> {
                val id = e.optLong("meeting_id")
                val title = e.optString("title")
                _finishing.update { it - id }
                Work.done("meeting$id")
                viewModelScope.launch {
                    refreshMeetings(); refreshStats(); refreshSuggestions(); refreshFolders()
                    if (_review.value?.meeting?.id == id) openReview(id)
                }
                // The person asked for a better transcript from the Review screen.
                if (e.optBoolean("improve")) {
                    if (e.optBoolean("improved")) toast("success", "Transcript improved")
                    else toast("info", e.optString("note").ifBlank { "The transcript was left as it was." })
                }
                if (e.optBoolean("notify", true)) {
                    if (AppState.foreground) toast("success", "Notes ready: ${title.ifBlank { "your meeting" }}", "Open") { openReview(id) }
                    else Notifier.notesReady(ctx, id, title, e.optString("preview"))
                }
            }
            "wrap_failed" -> {
                val id = e.optLong("meeting_id")
                _finishing.update { it - id }
                Work.done("meeting$id")
                if (AppState.foreground) toast("warning", "The notes could not be finished. The meeting is saved.")
                else Notifier.failed(ctx, "")
            }
            "discarded" -> {
                val id = e.optLong("meeting_id")
                _finishing.update { it - id }
                Work.done("meeting$id")
                toast("info", "Nothing was captured, so the meeting was discarded.")
                refreshMeetings()
            }
            "meetings_changed" -> refreshMeetings()
            "ingest" -> onIngest(e)
            "chat" -> onChat(e)
        }
    }

    // ------------------------------------------------------------------ refresh

    fun refreshAll() {
        refreshMeetings(); refreshDocs(); refreshStats(); refreshSuggestions(); refreshPrefs(); refreshFolders()
    }

    // ------------------------------------------------------------------ folders

    fun refreshFolders() {
        viewModelScope.launch { runCatching { Folder.list(backend.array("folders.list")) }.onSuccess { _folders.value = it } }
    }

    fun createFolder(name: String, color: Int, then: ((Long) -> Unit)? = null) {
        val n = name.trim()
        if (n.isEmpty()) return
        viewModelScope.launch {
            try {
                val id = backend.obj("folders.create", JSONObject().put("name", n).put("color", color)).optLong("id")
                refreshFolders()
                then?.invoke(id)
            } catch (e: Exception) { toast("error", e.message ?: "Could not create the folder") }
        }
    }

    fun renameFolder(f: Folder, name: String, color: Int) {
        val n = name.trim()
        if (n.isEmpty()) return
        _folders.update { l -> l.map { if (it.id == f.id) it.copy(name = n, color = color) else it } }
        viewModelScope.launch { runCatching { backend.call("folders.rename", JSONObject().put("id", f.id).put("name", n).put("color", color)) } }
    }

    fun deleteFolder(f: Folder) {
        _folders.update { l -> l.filter { it.id != f.id } }
        if (meetingsFilter.value == f.id) meetingsFilter.value = null
        if (knowledgeFilter.value == f.id) knowledgeFilter.value = null
        if (askFolder.value == f.id) askFolder.value = null
        if (liveFolder.value == f.id) liveFolder.value = null
        viewModelScope.launch {
            runCatching { backend.call("folders.delete", JSONObject().put("id", f.id)) }
            refreshMeetings(); refreshDocs(); refreshFolders(); refreshSuggestions()
            toast("info", "Folder removed. Its items are now unfiled.")
        }
    }

    fun moveMeeting(id: Long, folder: Long?) {
        _meetings.update { l -> l.map { if (it.id == id) it.copy(folderId = folder) else it } }
        _review.update { d -> if (d != null && d.meeting.id == id) d.copy(meeting = d.meeting.copy(folderId = folder)) else d }
        viewModelScope.launch {
            runCatching { backend.call("meeting.set_folder", JSONObject().put("id", id).put("folder", folder ?: 0)) }
            refreshFolders()
        }
    }

    fun moveDoc(id: Long, folder: Long?) {
        _docs.update { l -> l.map { if (it.id == id) it.copy(folderId = folder) else it } }
        _doc.update { d -> if (d != null && d.doc.id == id) d.copy(doc = d.doc.copy(folderId = folder)) else d }
        viewModelScope.launch {
            runCatching { backend.call("doc.set_folder", JSONObject().put("id", id).put("folder", folder ?: 0)) }
            refreshFolders(); refreshSuggestions()
        }
    }

    /** Changes the folder of the meeting being recorded: Live Assistance follows it at once. */
    fun setLiveFolder(folder: Long?) {
        liveFolder.value = folder
        if (_live.value != null) viewModelScope.launch { runCatching { backend.call("meeting.live_folder", JSONObject().put("folder", folder ?: 0)) } }
    }

    fun setAskFolder(folder: Long?) {
        askFolder.value = folder
        refreshSuggestions()
    }

    // ------------------------------------------------------------------ meetings: list, recording, review, delete

    fun refreshMeetings() {
        viewModelScope.launch {
            runCatching {
                val arr = backend.array("meeting.list")
                saveCache("meetings", arr.toString())
                arr.objects().map(MeetingItem::from)
            }.onSuccess { _meetings.value = it }
        }
    }

    // ---- the last lists, kept on disk so the screen is full the instant the app opens, before the engine is up
    private fun cacheFile(name: String) = File(ctx.filesDir, "cache-$name.json")

    private fun saveCache(name: String, json: String) {
        runCatching {
            val tmp = File(ctx.filesDir, "cache-$name.tmp")
            tmp.writeText(json)
            tmp.renameTo(cacheFile(name))
        }
    }

    private fun loadCache(name: String): JSONArray? =
        runCatching { cacheFile(name).takeIf { it.isFile }?.readText()?.let { JSONArray(it) } }.getOrNull()

    /** Shows what was on screen last time. Replaced a moment later by the real lists. */
    private fun showCachedLists() {
        loadCache("meetings")?.let { a -> runCatching { _meetings.value = a.objects().map(MeetingItem::from) } }
        loadCache("docs")?.let { a -> runCatching { _docs.value = a.objects().map(DocItem::from) } }
    }

    private var aloneJob: Job? = null
    @Volatile private var speechWarmed = false

    /** "Keep going": the person does not want to wait. The load carries on behind the app. */
    fun skipAiWarm() { warmSkipped = true; _aiWarm.value = null }

    /** Gemma is loaded when the app opens even with a compute node nearby, so the phone can take over at once. Off by default. */
    val keepAiReady = MutableStateFlow(enginePrefs.getBoolean("keep_ai_ready", false))
    fun setKeepAiReady(on: Boolean) {
        keepAiReady.value = on
        enginePrefs.edit().putBoolean("keep_ai_ready", on).apply()
        if (on) loadChatIfAlone()
    }

    /**
     * With no compute node in reach the phone writes everything itself, so Gemma is loaded now, while the app is opening and
     * nothing is being recorded. Loading it later, in the middle of a meeting, stalls the screen for about 5 seconds then.
     * A compute node that is paired gets six seconds to answer first (after it restarts the phone can take that long to see it), and if one does Gemma stays unloaded.
     */
    fun loadChatIfAlone() {
        aloneJob?.cancel()
        warmSkipped = false
        aloneJob = viewModelScope.launch(Dispatchers.Default) {
            while (!nativeReady) delay(250)
            // All the warm up happens here, behind the loading screen when it is up: the search model, the speech model run once the
            // way live captions run it, and (below) Gemma with its image reader. After this only real work is left.
            ensureModels()
            if (!speechWarmed) {
                speechWarmed = true
                runCatching { backend.call("engine.warm_speech") }
            }
            var waited = 0
            while (true) {
                loadNodes()
                val n = _nodes.value
                val hasNode = n.enabled && n.paired.any { it.approved }
                if (!hasNode || keepAiReady.value || computeNodeInReach() || waited >= 6000) break
                // The loading screen says what it is waiting for, so a few seconds of looking for the compute node is not a mystery.
                if (_aiWarm.value != null && !warmSkipped) _aiWarm.value = AiWarm("Looking for ${n.paired.first { it.approved }.name}", 0f)
                delay(1000); waited += 1000
            }
            val alone = keepAiReady.value || !computeNodeInReach()
            // Remembered for the next start: after a start with a compute node in reach the loading screen is not put up first.
            enginePrefs.edit().putBoolean("last_alone", alone).apply()
            if (alone && _live.value == null && chatModel == null) warmChat() else _aiWarm.value = null
        }
    }

    /**
     * Loads Gemma behind a loading screen with a bar. LiteRT-LM does not say how far it is, so the bar follows how long the last
     * load took (20 seconds the first time) and stops short of the end until the load is really done.
     */
    private suspend fun warmChat() {
        val expectedMs = enginePrefs.getLong("chat_load_ms", 20_000L).coerceIn(4_000L, 120_000L)
        val began = android.os.SystemClock.elapsedRealtime()
        if (!warmSkipped) _aiWarm.value = AiWarm("Getting the AI ready", 0f)
        val ticker = viewModelScope.launch(Dispatchers.Default) {
            while (true) {
                if (!warmSkipped) {
                    val f = ((android.os.SystemClock.elapsedRealtime() - began).toFloat() / expectedMs).coerceIn(0f, 0.94f)
                    _aiWarm.value = AiWarm(kotlinLabel?.takeIf { it.isNotBlank() && it != "Starting" } ?: "Getting the AI ready", f)
                }
                delay(250)
            }
        }
        val ok = ensureChat()
        ticker.cancel()
        if (ok) enginePrefs.edit().putLong("chat_load_ms", android.os.SystemClock.elapsedRealtime() - began).apply()
        if (!warmSkipped) {
            _aiWarm.value = AiWarm(if (ok) "Ready" else "Getting the AI ready", if (ok) 1f else 0f)
            delay(350)
        }
        _aiWarm.value = null
    }

    /** True when a compute node is switched on, allowed for this phone and answering. */
    private fun computeNodeInReach(): Boolean = _nodes.value.let { n -> n.enabled && n.paired.any { it.approved && it.online } }

    fun startRecording() {
        if (_live.value != null) {
            liveExpanded.value = true
            return
        }
        viewModelScope.launch {
            try {
                val folder = liveFolder.value ?: meetingsFilter.value?.takeIf { it > 0 }
                val r = backend.obj("meeting.start", JSONObject().put("folder", folder ?: 0))
                // With no compute node in reach the phone writes the jots and cards itself, so Gemma is loaded now
                // instead of in the middle of the meeting.
                if (!computeNodeInReach()) viewModelScope.launch(Dispatchers.Default) { ensureChat() }
                _live.value = LiveState(meetingId = r.optLong("id"), recording = true)
                liveFolder.value = folder
                liveExpanded.value = true
                startMic(r.optLong("id"), 0.0)
            } catch (e: Exception) {
                toast("error", e.message ?: "Could not start recording")
            }
        }
    }

    /**
     * Ends the recording and returns at once. The rest (last words, polishing, summary, title,
     * search index) runs in the background; a notification says when the notes are ready.
     */
    fun stopRecording() {
        val s = _live.value ?: return
        if (!s.recording) return
        RecordingService.micOff(ctx)
        val id = s.meetingId
        _finishing.update { it + (id to WrapState("finalizing", "Finalizing the transcript", 8f)) }
        Work.set(ctx, WorkItem("meeting$id", "Writing your meeting notes", "Finalizing the transcript", 0.08f))
        _live.value = null
        liveExpanded.value = true
        navigate.tryEmit(0)
        toast("info", "Saved. Your notes are being written, and you can keep using the phone.")
        viewModelScope.launch {
            try {
                backend.call("meeting.stop")
            } catch (e: Exception) {
                _finishing.update { it - id }
                Work.done("meeting$id")
                toast("error", e.message ?: "Could not stop")
            }
            refreshMeetings()
        }
    }

    /**
     * Carries on recording a meeting that ended early (the app was closed, the phone restarted) or one the person wants
     * to add more to. It is the same entry: the transcript, the notes and the photos continue where they stopped.
     */
    fun continueRecording(id: Long) {
        if (_live.value != null) {
            liveExpanded.value = true
            return
        }
        viewModelScope.launch {
            try {
                val r = backend.obj("meeting.resume", JSONObject().put("id", id))
                val snap = r.optJSONObject("snapshot") ?: JSONObject()
                _live.value = LiveState(
                    meetingId = id, recording = true, secs = snap.optLong("secs"),
                    lines = Line.list(snap.optJSONArray("lines")), notes = snap.optString("notes"), enhanced = snap.optString("enhanced"),
                    photos = Photo.list(snap.optJSONArray("photos")), jots = Jot.list(snap.optJSONArray("jots")),
                    correctedBy = if (snap.isNull("correcting")) null else snap.optString("correcting"),
                )
                liveExpanded.value = true
                _review.value = null
                startMic(id, snap.optDouble("secs", 0.0))
                refreshMeetings()
            } catch (e: Exception) {
                toast("error", e.message ?: "Could not continue recording")
            }
        }
    }

    fun setStyle(style: String) {
        _prefs.update { it.copy(noteStyle = style) }
        viewModelScope.launch { runCatching { backend.call("meeting.style", JSONObject().put("style", style)) } }
    }

    fun openReview(id: Long) {
        viewModelScope.launch {
            try {
                val o = backend.obj("meeting.get", JSONObject().put("id", id))
                _review.value = MeetingDetail(
                    MeetingItem.from(o.getJSONObject("meeting")), Line.list(o.optJSONArray("lines")),
                    Card.list(o.optJSONArray("cards")), Photo.list(o.optJSONArray("photos")), Jot.list(o.optJSONArray("jots")),
                    o.optJSONArray("audio")?.let { a -> List(a.length()) { i -> a.getJSONObject(i).let { x -> AudioPart(x.optString("path"), x.optDouble("offset")) } } }.orEmpty(),
                    o.optJSONArray("speakers")?.let { a -> (0 until a.length()).associate { i -> a.getJSONObject(i).let { x -> x.optInt("speaker") to x.optString("name") } } }.orEmpty(),
                )
            } catch (e: Exception) {
                toast("error", e.message ?: "Could not open the meeting")
            }
        }
    }

    fun closeReview() { player.stop(); _review.value = null }

    val player = ReviewPlayer(viewModelScope)

    fun rename(id: Long, title: String) {
        val t = title.trim()
        if (t.isEmpty()) return
        _review.update { d -> if (d != null && d.meeting.id == id) d.copy(meeting = d.meeting.copy(title = t)) else d }
        _meetings.update { l -> l.map { if (it.id == id) it.copy(title = t) else it } }
        viewModelScope.launch { runCatching { backend.call("meeting.rename", JSONObject().put("id", id).put("title", t)) } }
    }

    private var deleteJob: Job? = null

    /** Hides the meeting now and erases it after a few seconds unless Undo is tapped. */
    fun deleteMeeting(m: MeetingItem) {
        commitDelete()
        _pendingDelete.value = m
        if (_review.value?.meeting?.id == m.id) _review.value = null
        deleteJob = viewModelScope.launch {
            delay(6000)
            commitDelete()
        }
        toast("info", "Deleted “${m.title.ifBlank { "Untitled meeting" }}”", "Undo") { undoDelete() }
    }

    fun undoDelete() {
        deleteJob?.cancel()
        _pendingDelete.value = null
    }

    private fun commitDelete() {
        val m = _pendingDelete.value ?: return
        deleteJob?.cancel()
        _pendingDelete.value = null
        _meetings.update { l -> l.filter { it.id != m.id } }
        viewModelScope.launch {
            runCatching { backend.call("meeting.delete", JSONObject().put("id", m.id)) }
            AudioKeeper.deleteFor(ctx, m.id)
            refreshStats(); refreshDocs(); refreshSuggestions()
        }
    }

    fun meetingMarkdown(d: MeetingDetail): String = buildString {
        append("# ").append(d.meeting.title).append("\n\n")
        if (d.meeting.summary.isNotBlank()) append(d.meeting.summary.trim()).append("\n\n")
        val notes = d.meeting.notesEnhanced.ifBlank { d.meeting.notesRaw }
        if (notes.isNotBlank()) append("## Notes\n").append(notes.trim()).append("\n\n")
        if (d.lines.isNotEmpty()) {
            append("## Transcript\n")
            d.lines.forEach { append("[").append(formatClock(it.t0)).append("] ").append(it.text).append("\n") }
        }
    }.trim()

    // ------------------------------------------------------------------ development

    fun debugLiteRt(model: String, backend: String, max: Int, mtp: Boolean) {
        viewModelScope.launch(Dispatchers.IO) {
            try { Bench.log("litert $backend mtp=$mtp", Bench.litert(ctx, model, backend, max, mtp)) }
            catch (e: Throwable) { Bench.log("litert $backend FAILED", e.toString()) }
        }
    }

    /** Debug builds only: run any native command and log the answer (`adb logcat -s MarginaliaDbg`). */
    fun debugCall(method: String, args: String) {
        if (method == "debug.separate") {
            val id = JSONObject(args.ifBlank { "{}" }).optLong("id")
            viewModelScope.launch(Dispatchers.Default) {
                var waited = 0
                while (!nativeReady && waited < 60_000) { delay(250); waited += 250 }
                openReview(id)
                delay(2500)
                separateSpeakers(id, JSONObject(args.ifBlank { "{}" }).optInt("people"), JSONObject(args.ifBlank { "{}" }).optDouble("threshold", Diarizer.THRESHOLD.toDouble()).toFloat())
            }
            return
        }
        if (method == "debug.embed_bench") {
            viewModelScope.launch(Dispatchers.Default) {
                ensureModels()
                val text = "The quarterly planning meeting covered the migration of the export service and the owner of each task. ".repeat(14)
                val texts = Array(48) { "$it $text" }
                val out = StringBuilder()
                for (th in listOf<Int?>(null, 3, 4, 6)) {
                    val e = LiteRtEmbedder(ctx, File(ModelInstaller.dir(ctx), ModelInstaller.EMBED_LITERT).absolutePath, gpu = false, threads = th)
                    val err = e.prepare()
                    if (err != null) { out.append("${e.label} threads=$th: $err\n"); continue }
                    e.embed(texts.sliceArray(0 until 4)) // warm up
                    val t = System.nanoTime()
                    e.embed(texts)
                    val ms = (System.nanoTime() - t) / 1e6
                    out.append("${e.label} threads=${th ?: "default"}: ${texts.size} passages in ${ms.toInt()} ms = ${"%.2f".format(texts.size * 1000 / ms)} per second\n")
                    e.close()
                }
                android.util.Log.i("MarginaliaDbg", "debug.embed_bench -> $out")
            }
            return
        }
        // Long jobs run under the same foreground service the real flow uses, or Android would reap the process.
        val long = method == "debug.long_meeting"
        if (long) Work.set(ctx, WorkItem("debug", "Running a test meeting", "Writing the notes", -1f))
        viewModelScope.launch(Dispatchers.IO) {
            var waited = 0
            while (!nativeReady && waited < 60_000) { delay(250); waited += 250 }
            val r = try { backend.call(method, if (args.isBlank()) JSONObject() else JSONObject(args)).toString() } catch (e: Exception) { "ERROR ${e.message}" }
            if (long) Work.done("debug")
            android.util.Log.i("MarginaliaDbg", "$method -> $r")
        }
    }

    /**
     * Debug builds only: plays a 16 kHz mono PCM16 WAV into the engine as if it came from the
     * microphone, so the live pipeline can be exercised on an emulator without audio hardware.
     */
    fun debugFeed(path: String, speed: Float, resume: Long = 0L) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                var waited = 0
                while (!nativeReady && waited < 60_000) { delay(250); waited += 250 }
                ensureModels()
                val bytes = File(path).readBytes()
                val r = if (resume > 0) backend.obj("meeting.resume", JSONObject().put("id", resume)) else backend.obj("meeting.start")
                val snap = r.optJSONObject("snapshot")
                _live.value = LiveState(meetingId = r.optLong("id"), recording = true, lines = Line.list(snap?.optJSONArray("lines")), jots = Jot.list(snap?.optJSONArray("jots")))
                liveExpanded.value = true
                // Like a real recording, keep the audio so the rest of the flow (playback, speakers) can be tried.
                var keeper: AudioKeeper? = null
                if (keepAudio.value) {
                    val f = AudioKeeper.newFile(ctx, r.optLong("id"))
                    runCatching { backend.call("meeting.audio_add", JSONObject().put("id", r.optLong("id")).put("path", f.absolutePath).put("offset", snap?.optDouble("secs", 0.0) ?: 0.0)) }
                    keeper = AudioKeeper(f).also { it.start() }
                }
                val frame = 1600 * 2
                var off = 44
                val buf = ShortArray(1600)
                val t0 = System.nanoTime()
                var sent = 0L
                while (off + frame <= bytes.size && _live.value?.recording == true) {
                    java.nio.ByteBuffer.wrap(bytes, off, frame).order(java.nio.ByteOrder.LITTLE_ENDIAN).asShortBuffer().get(buf)
                    var sum = 0.0
                    for (v in buf) { val f = v / 32768.0; sum += f * f }
                    RecorderBus.setLevel(minOf(1f, (kotlin.math.sqrt(sum / buf.size) * 7.0).toFloat()))
                    Native.pushAudio(buf, buf.size)
                    keeper?.offer(buf, buf.size)
                    off += frame; sent += 1600
                    val due = t0 + (sent * 1_000_000_000L / 16_000 / speed).toLong()
                    val wait = (due - System.nanoTime()) / 1_000_000
                    if (wait > 0) delay(wait)
                }
                RecorderBus.setLevel(0f)
                keeper?.close()
            } catch (e: Exception) {
                toast("error", "Debug feed failed: ${e.message}")
            }
        }
    }

    // ------------------------------------------------------------------ knowledge

    fun refreshDocs() {
        viewModelScope.launch {
            runCatching {
                val arr = backend.array("kb.docs")
                saveCache("docs", arr.toString())
                arr.objects().map(DocItem::from)
            }.onSuccess { _docs.value = it }
        }
    }

    fun refreshStats() {
        viewModelScope.launch {
            runCatching { backend.obj("stats.get") }.onSuccess {
                _stats.value = Stats(
                    it.optLong("meetings"), it.optLong("seconds_transcribed"), it.optLong("words_transcribed"),
                    it.optLong("documents"), it.optLong("chunks"), it.optLong("questions_answered"),
                )
            }
        }
    }

    private val _doc = MutableStateFlow<DocDetail?>(null)
    /** The document being read, or null. */
    val doc: StateFlow<DocDetail?> = _doc.asStateFlow()

    /** Opens a document. A meeting that was filed as a document opens as the meeting. */
    fun openDoc(d: DocItem) {
        if (d.source.startsWith("meeting://")) {
            d.source.removePrefix("meeting://").toLongOrNull()?.let { openReview(it) }
            return
        }
        viewModelScope.launch {
            try {
                val o = backend.obj("kb.doc", JSONObject().put("id", d.id))
                val chunks = o.optJSONArray("chunks")?.let { a -> List(a.length()) { i -> a.getJSONObject(i).let { x -> DocChunk(x.optString("section"), x.optInt("page"), x.optString("text")) } } }.orEmpty()
                _doc.value = DocDetail(DocItem.from(o.getJSONObject("doc")), chunks, o.optJSONObject("file")?.let { DocFile(it.optString("path"), it.optString("mime")) })
            } catch (e: Exception) {
                toast("error", e.message ?: "Could not open the document")
            }
        }
    }

    fun closeDoc() { _doc.value = null }

    fun deleteDoc(d: DocItem) {
        if (_doc.value?.doc?.id == d.id) _doc.value = null
        _docs.update { l -> l.filter { it.id != d.id } }
        viewModelScope.launch {
            runCatching { backend.call("kb.delete", JSONObject().put("id", d.id)) }
            refreshStats(); refreshSuggestions()
        }
    }

    /**
     * Files picked from the system picker. Documents are copied in as they are; photos are
     * shrunk for the vision model; audio and video are decoded to 16 kHz WAV so any format the
     * phone can play can be transcribed.
     */
    fun ingestUris(uris: List<Uri>) {
        viewModelScope.launch(Dispatchers.IO) {
            val dir = File(ctx.filesDir, "inbox").also { it.mkdirs() }
            for (uri in uris) {
                val name = displayName(uri) ?: "document"
                val stem = name.substringBeforeLast('.', name)
                // Some apps share files without a type: fall back to the extension.
                val mime = ctx.contentResolver.getType(uri)
                    ?: android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase())
                    ?: ""
                try {
                    when {
                        mime.startsWith("image/") -> {
                            val dst = File(dir, "$stem.jpg")
                            MediaTools.downscaleJpeg(ctx, uri, dst)
                            backend.call("kb.ingest", ingestArgs(dst.absolutePath))
                        }
                        mime.startsWith("audio/") || mime.startsWith("video/") -> {
                            val jobId = -System.nanoTime()
                            val kind = if (mime.startsWith("video/")) "video" else "audio"
                            setLocalJob(IngestJob(jobId, name, "decoding"))
                            val dst = File(dir, "$stem.wav")
                            val secs = try {
                                MediaTools.decodeToWav(ctx, uri, dst) { f -> setLocalJob(IngestJob(jobId, name, "decoding", (f * 100).toInt(), 100)) }
                            } finally {
                                dismissJob(jobId)
                            }
                            if (secs < 0.5) throw IllegalStateException("That $kind has no speech to transcribe")
                            backend.call("kb.ingest", ingestArgs(dst.absolutePath))
                        }
                        else -> {
                            val dst = File(dir, name)
                            ctx.contentResolver.openInputStream(uri)?.use { i -> dst.outputStream().use { o -> i.copyTo(o) } }
                                ?: throw IllegalStateException("cannot read $name")
                            backend.call("kb.ingest", ingestArgs(dst.absolutePath))
                        }
                    }
                } catch (e: Exception) {
                    toast("error", "Could not add ${name}: ${e.message}")
                }
            }
        }
    }

    /** Where a new addition is filed: the folder being browsed, if any. */
    private fun ingestArgs(path: String): JSONObject {
        val f = knowledgeFilter.value?.takeIf { it > 0 }
        return JSONObject().put("path", path).put("folder", f ?: 0)
    }

    private fun setLocalJob(job: IngestJob) {
        _ingest.update { l -> if (l.any { it.job == job.job }) l.map { if (it.job == job.job) job else it } else l + job }
    }

    // ------------------------------------------------------------------ photos, jots and questions during a meeting

    /**
     * A photo from the camera or gallery. It joins the meeting being recorded, or the one open
     * for review; Gemma describes it and the description becomes searchable.
     */
    fun addPhoto(uri: Uri) {
        viewModelScope.launch(Dispatchers.IO) {
            try {
                val dir = File(ctx.filesDir, "photos").also { it.mkdirs() }
                val dst = File(dir, "p${System.currentTimeMillis()}.jpg")
                // The camera app may still be writing: wait until the file stops growing, then read it, trying
                // a few times before giving up.
                var lastSize = -1L
                var steady = 0
                for (i in 0 until 30) {
                    val size = runCatching { ctx.contentResolver.openAssetFileDescriptor(uri, "r")?.use { it.length } ?: -1L }.getOrDefault(-1L)
                    if (size > 10_000 && size == lastSize) { if (++steady >= 2) break } else steady = 0
                    lastSize = size
                    delay(150)
                }
                var decoded = false
                var failure: Exception? = null
                for (attempt in 0 until 3) {
                    try { MediaTools.downscaleJpeg(ctx, uri, dst); decoded = dst.length() > 0; if (decoded) break } catch (e: Exception) { failure = e }
                    delay(400)
                }
                if (!decoded) throw (failure ?: IllegalStateException("the camera did not save the picture. Try again"))
                val live = _live.value
                val args = JSONObject().put("path", dst.absolutePath)
                if (live == null || !live.recording) _review.value?.let { args.put("id", it.meeting.id) }
                backend.call("meeting.photo", args)
            } catch (e: Exception) {
                toast("error", "Could not add that photo: ${e.message}")
            }
        }
    }

    /** A question typed while the meeting runs, answered from what has been said so far. */
    fun askLive(question: String) {
        val q = question.trim()
        if (q.isEmpty()) return
        viewModelScope.launch {
            try { backend.call("meeting.ask", JSONObject().put("question", q)) }
            catch (e: Exception) { toast("info", e.message ?: "Could not answer that now") }
        }
    }

    /** Three questions the listener could put to the speaker. */
    fun suggestQuestions() {
        viewModelScope.launch {
            try { backend.call("meeting.suggest") }
            catch (e: Exception) { toast("info", e.message ?: "Could not think of questions yet") }
        }
    }

    /** A quick note. It lands in the timeline at once and is polished in the background. */
    fun addJot(text: String, kind: String = "note", t: Double? = null) {
        val clean = text.trim()
        if (clean.isEmpty()) return
        viewModelScope.launch {
            try {
                val args = JSONObject().put("text", clean).put("kind", kind)
                if (t != null) args.put("t", t)
                val live = _live.value
                if (live == null || !live.recording) _review.value?.let { args.put("id", it.meeting.id) }
                backend.call("meeting.jot", args)
            } catch (e: Exception) {
                toast("error", e.message ?: "Could not save the note")
            }
        }
    }

    fun editJot(j: Jot, text: String) {
        val clean = text.trim()
        if (clean.isEmpty() || clean == j.raw) return
        viewModelScope.launch { runCatching { backend.call("meeting.jot_edit", JSONObject().put("id", j.id).put("text", clean)) } }
    }

    fun deleteJot(j: Jot) {
        _live.update { s -> s?.copy(jots = s.jots.filter { it.id != j.id }) }
        _review.update { d -> d?.copy(jots = d.jots.filter { it.id != j.id }) }
        viewModelScope.launch { runCatching { backend.call("meeting.jot_delete", JSONObject().put("id", j.id)) } }
    }

    /** Have Gemma read a photo now instead of after the meeting. */
    fun readPhotoNow(p: Photo) {
        viewModelScope.launch { runCatching { backend.call("meeting.photo_read", JSONObject().put("id", p.id)) } }
    }

    fun deletePhoto(p: Photo) {
        _live.update { s -> s?.copy(photos = s.photos.filter { it.id != p.id }) }
        _review.update { d -> d?.copy(photos = d.photos.filter { it.id != p.id }) }
        viewModelScope.launch { runCatching { backend.call("meeting.photo_delete", JSONObject().put("id", p.id)) } }
    }

    /** Remove an assistance card. [live] is true for a card on the live timeline, false for a saved card in the review. */
    fun deleteCard(card: Card, live: Boolean) {
        if (live) _live.update { s -> s?.copy(cards = s.cards.filter { it.id != card.id }) }
        viewModelScope.launch {
            runCatching { backend.call("meeting.card_delete", JSONObject().put("id", card.id).put("db", !live)) }
                .onSuccess {
                    if (!live) _review.update { d -> d?.copy(cards = d.cards.filter { it.id != card.id }) }
                    toast("success", "Card removed")
                }
                .onFailure { toast("error", it.message ?: "Could not remove the card.") }
        }
    }

    // ------------------------------------------------------------------ adding to the library

    fun ingestUrl(url: String) {
        val u = url.trim()
        if (u.isEmpty()) return
        viewModelScope.launch {
            runCatching { backend.call("kb.ingest_url", JSONObject().put("url", u).put("folder", knowledgeFilter.value?.takeIf { it > 0 } ?: 0)) }
                .onFailure { toast("error", it.message ?: "Could not add that page") }
        }
    }

    /** Text shared from another app becomes a small note in the knowledge base. */
    fun ingestText(text: String, title: String) {
        viewModelScope.launch(Dispatchers.IO) {
            val dir = File(ctx.filesDir, "inbox").also { it.mkdirs() }
            val f = File(dir, title.replace(Regex("[^A-Za-z0-9 ._-]"), "_").take(60).ifBlank { "Note" } + ".txt")
            f.writeText(text)
            runCatching { backend.call("kb.ingest", ingestArgs(f.absolutePath)) }
        }
    }

    fun searchPassages(query: String) {
        if (query.isBlank()) { _passages.value = null; return }
        viewModelScope.launch {
            runCatching { Source.list(backend.array("kb.search", JSONObject().put("query", query).put("scope", "all").put("folder", knowledgeFilter.value?.takeIf { it > 0 } ?: 0))) }.onSuccess { _passages.value = it }
        }
    }

    fun dismissJob(job: Long) = _ingest.update { l -> l.filter { it.job != job } }

    private fun onIngest(e: JSONObject) {
        val job = IngestJob(
            e.optLong("job"), e.optString("label"), e.optString("stage"),
            e.optInt("done"), e.optInt("total"), e.optString("reason").ifBlank { e.optString("error") },
        )
        _ingest.update { l -> if (l.any { it.job == job.job }) l.map { if (it.job == job.job) job else it } else l + job }
        if (job.active) {
            Work.set(ctx, WorkItem("ingest${job.job}", "Reading ${job.label.ifBlank { "your file" }}", job.message.ifBlank { "Making it searchable" }, if (job.total > 0) job.done.toFloat() / job.total else -1f))
        } else {
            Work.done("ingest${job.job}")
        }
        if (job.stage == "done" || job.stage == "skipped" || job.stage == "failed") {
            refreshDocs(); refreshStats(); refreshSuggestions()
            viewModelScope.launch {
                delay(if (job.stage == "done") 2500 else 6000)
                dismissJob(job.job)
            }
        }
    }

    private fun displayName(uri: Uri): String? =
        ctx.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        } ?: uri.lastPathSegment

    // ------------------------------------------------------------------ ask

    fun refreshSuggestions() {
        viewModelScope.launch {
            runCatching { backend.array("chat.suggestions", JSONObject().put("folder", askFolder.value ?: 0)) }.onSuccess { a -> _suggestions.value = List(a.length()) { a.getString(it) } }
        }
    }

    fun ask(question: String, scopeOverride: String? = null) {
        val q = question.trim()
        if (q.isEmpty() || _turns.value.any { it.busy }) return
        val history = JSONArray()
        _turns.value.filter { it.state == "done" }.takeLast(3).forEach { history.put(JSONArray().put(it.question).put(it.answer)) }
        pendingQuestion = q
        viewModelScope.launch {
            try {
                val r = backend.obj("chat.ask", JSONObject().put("question", q).put("scope", scopeOverride ?: scope.value).put("history", history).put("folder", askFolder.value ?: 0))
                val id = r.optLong("turn")
                _turns.update { l -> if (l.any { it.id == id }) l else l + ChatTurn(id, q) }
            } catch (e: Exception) {
                toast("warning", e.message ?: "Could not ask")
            }
        }
    }

    fun stopAnswer() {
        val t = _turns.value.lastOrNull { it.busy } ?: return
        viewModelScope.launch { runCatching { backend.call("chat.stop", JSONObject().put("turn", t.id)) } }
    }

    fun clearChat() { _turns.value = emptyList() }

    private fun onChat(e: JSONObject) {
        val id = e.optLong("turn")
        _turns.update { l ->
            val turn = l.firstOrNull { it.id == id } ?: ChatTurn(id, pendingQuestion)
            val updated = turn.copy(
                state = e.optString("state"),
                answer = if (e.has("text")) e.optString("text") else turn.answer,
                sources = if (e.has("sources")) Source.list(e.optJSONArray("sources")) else turn.sources,
                stats = if (e.has("stats")) e.optString("stats") else turn.stats,
                error = e.optString("error", turn.error),
            )
            if (l.any { it.id == id }) l.map { if (it.id == id) updated else it } else l + updated
        }
        if (e.optString("state") == "done") refreshStats()
    }

    // ------------------------------------------------------------------ settings

    fun refreshPrefs() {
        viewModelScope.launch { runCatching { backend.obj("settings.get") }.onSuccess { _prefs.value = Prefs.from(it) } }
    }

    fun setPref(key: String, value: Any) {
        viewModelScope.launch {
            runCatching { backend.obj("settings.set", JSONObject().put(key, value)) }.onSuccess { _prefs.value = Prefs.from(it) }
        }
    }

    fun setAppearance(v: String) {
        _prefs.update { it.copy(appearance = v) }
        setPref("appearance", v)
    }

    // ------------------------------------------------------------------ home compute nodes

    private val _nodes = MutableStateFlow(HomeComputers())
    val nodes: StateFlow<HomeComputers> = _nodes.asStateFlow()
    private val _pairing = MutableStateFlow(false)
    val pairing: StateFlow<Boolean> = _pairing.asStateFlow()

    /** Reports each compute node that network discovery finds to the core. */
    private val discovery = NodeDiscovery(app, report = { n ->
        viewModelScope.launch(Dispatchers.Default) {
            // A compute node found while the app is still starting is reported as soon as the core is up.
            var waited = 0
            while (!nativeReady && waited < 120_000) { delay(250); waited += 250 }
            if (!nativeReady) return@launch
            runCatching {
                backend.call("nodes.seen", JSONObject().put("name", n.name).put("id", n.id).put("port", n.port).put("addrs", JSONArray(n.addrs)).put("paused", n.paused))
            }
            if (nodeWatch?.isActive == true) loadNodes()
        }
    }, gone = { id ->
        // A compute node that says goodbye is stopping. Show it as not reachable at once.
        viewModelScope.launch(Dispatchers.Default) {
            if (!nativeReady) return@launch
            runCatching { backend.call("nodes.gone", JSONObject().put("id", id)) }
            if (nodeWatch?.isActive == true) loadNodes()
        }
    })

    /**
     * Tells the core when the network under the phone changes, so work running on a home compute node
     * moves to another compute node or back to the phone at once instead of after a timeout.
     */
    private val netWatch = object : android.net.ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: android.net.Network) { reportNetwork(true) }
        override fun onLost(network: android.net.Network) { reportNetwork(false) }
    }

    private fun reportNetwork(up: Boolean) {
        viewModelScope.launch(Dispatchers.Default) {
            var waited = 0
            while (!nativeReady && waited < 120_000) { delay(250); waited += 250 }
            if (!nativeReady) return@launch
            runCatching { backend.call("nodes.network", JSONObject().put("up", up)) }
        }
    }

    init {
        // Background work (notes being written with the app closed) keeps looking for home compute nodes so
        // it can use them, and the search stops again when the work ends and the app is not on screen.
        viewModelScope.launch {
            Work.items.collect { items ->
                if (items.isNotEmpty()) discovery.start() else if (!AppState.foreground) discovery.stop()
            }
        }
        runCatching {
            (app.getSystemService(android.content.Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager).registerDefaultNetworkCallback(netWatch)
        }
    }

    /** Phones (by id) the person said "Not now" to or already answered. They are forgotten once the phone stops asking. */
    private val _dismissed = MutableStateFlow<Set<String>>(emptySet())

    /** The next phone waiting for the person's yes or no, from any approved compute node. */
    val approvalAsk: StateFlow<ApprovalAsk?> = combine(_nodes, _dismissed) { n, gone -> groupApprovals(n.paired, gone) }
        .stateIn(viewModelScope, SharingStarted.Eagerly, null)

    private suspend fun loadNodes() {
        runCatching { backend.obj("nodes.list") }.onSuccess { j ->
            val list = HomeComputers.from(j)
            _nodes.value = list
            val asking = askingPhones(list.paired)
            _dismissed.update { d -> d.intersect(asking) }
        }
    }

    private var nodeWatch: Job? = null

    /** Found compute nodes the person said "Not now" to, by [foundKey]. Kept on the phone so they are not asked again on every launch. */
    private val _foundGone = MutableStateFlow(enginePrefs.getStringSet("found_dismissed", emptySet()).orEmpty().toSet())

    /** Found compute nodes the Meetings banner should offer to connect to. */
    val connectOffer: StateFlow<List<FoundComputer>> = combine(_nodes, _foundGone) { n, gone -> computersToOffer(n, gone) }
        .stateIn(viewModelScope, SharingStarted.Eagerly, emptyList())

    /** "Not now" on the banner: these compute nodes are not offered again. A new compute node is. */
    fun dismissOffer(list: List<FoundComputer>) {
        val keys = list.map { foundKey(it) }.toSet()
        _foundGone.update { it + keys }
        enginePrefs.edit().putStringSet("found_dismissed", _foundGone.value).apply()
    }

    /**
     * While the app is on screen, look at the list of home compute nodes every few seconds, also when none is paired,
     * so a request from another phone and a new compute node on the network both show up wherever the person is.
     * It stops when the app goes to the background.
     */
    private fun startNodeWatch() {
        if (nodeWatch?.isActive == true) return
        nodeWatch = viewModelScope.launch {
            while (!nativeReady) delay(250)
            loadNodes()
            // The search was only just started, so look once more soon for what it found.
            delay(1500)
            loadNodes()
            while (true) {
                delay(5000)
                loadNodes()
            }
        }
    }

    /** Keeps the list of home compute nodes fresh. Call it from a screen that shows them, and cancel it when that screen closes. */
    suspend fun watchNodes() {
        while (!nativeReady) delay(250)
        while (true) {
            loadNodes()
            delay(3000)
        }
    }

    /** Pairs with a compute node. [onDone] says whether it worked. */
    fun pairNode(addr: String, id: String, code: String, onDone: (Boolean) -> Unit = {}) {
        if (_pairing.value) return
        _pairing.value = true
        viewModelScope.launch {
            val args = JSONObject().put("code", code.filter { it.isLetterOrDigit() }.uppercase()).put("phone_name", android.os.Build.MODEL)
            if (addr.isNotBlank()) args.put("addr", addr.trim())
            if (id.isNotBlank()) args.put("id", id)
            var ok = false
            try {
                val res = backend.call("nodes.pair", args)
                if (res is String && res.isNotBlank()) {
                    toast("error", res)
                } else {
                    ok = true
                    val name = (res as? JSONObject)?.optString("name").orEmpty()
                    toast("success", if (name.isBlank()) "Compute node added." else "$name is added.")
                }
            } catch (e: Exception) {
                toast("error", e.message?.takeIf { it.isNotBlank() } ?: "That did not work. Check the code and try again.")
            }
            _pairing.value = false
            loadNodes()
            onDone(ok)
        }
    }

    /** Asks a compute node to trust this phone. The person has already said yes. [onDone] says whether it worked. */
    fun trustNode(addr: String, id: String, onDone: (Boolean) -> Unit = {}) {
        if (_pairing.value) return
        _pairing.value = true
        viewModelScope.launch {
            val args = JSONObject().put("phone_name", android.os.Build.MODEL)
            if (addr.isNotBlank()) args.put("addr", addr.trim())
            if (id.isNotBlank()) args.put("id", id)
            var ok = false
            try {
                val res = backend.call("nodes.trust", args)
                if (res is String && res.isNotBlank()) {
                    toast("error", res)
                } else {
                    ok = true
                    val o = res as? JSONObject
                    val name = o?.optString("name").orEmpty()
                    if (o?.optString("state") == "waiting") toast("info", "Waiting for approval")
                    else toast("success", if (name.isBlank()) "Connected" else "Connected to $name")
                }
            } catch (e: Exception) {
                toast("error", e.message?.takeIf { it.isNotBlank() } ?: "That did not work. Check the address and try again.")
            }
            _pairing.value = false
            loadNodes()
            onDone(ok)
        }
    }

    /** Answers another phone's request on every compute node where it is pending. */
    fun decideNode(ask: ApprovalAsk, allow: Boolean) {
        _dismissed.update { it + ask.who }
        viewModelScope.launch {
            var failed: String? = null
            for (c in ask.computers) {
                try {
                    backend.call("nodes.decide", JSONObject().put("fp", c.fp).put("who", ask.who).put("allow", allow))
                } catch (e: Exception) {
                    failed = failed ?: e.message ?: "Could not save that answer."
                }
            }
            if (failed != null) {
                _dismissed.update { it - ask.who }
                toast("error", failed)
            } else if (allow) {
                toast("success", "${ask.phone.ifBlank { "That phone" }} can use ${ask.computers.size.let { if (it == 1) "the compute node" else "$it compute nodes" }}")
            }
            loadNodes()
        }
    }

    /** Asks each compute node in turn to trust this phone, then says how it went in one toast. */
    fun trustAll(targets: List<FoundComputer>, onDone: (Boolean) -> Unit = {}) {
        if (_pairing.value || targets.isEmpty()) return
        _pairing.value = true
        viewModelScope.launch {
            var approved = 0; var waiting = 0; var firstError: String? = null
            for (t in targets) {
                val args = JSONObject().put("phone_name", android.os.Build.MODEL).put("id", t.id)
                if (t.id.isBlank()) t.addrs.firstOrNull()?.let { args.put("addr", it) }
                try {
                    val res = backend.call("nodes.trust", args)
                    if (res is String && res.isNotBlank()) { firstError = firstError ?: res }
                    else if ((res as? JSONObject)?.optString("state") == "waiting") waiting++
                    else approved++
                } catch (e: Exception) {
                    firstError = firstError ?: e.message?.takeIf { it.isNotBlank() } ?: "That did not work. Check the address and try again."
                }
            }
            _pairing.value = false
            loadNodes()
            if (approved + waiting == 0) toast("error", firstError ?: "That did not work.")
            else toast(if (waiting == 0) "success" else "info", trustSummary(approved, waiting))
            onDone(approved + waiting > 0)
        }
    }

    fun forgetNode(fp: String) {
        viewModelScope.launch {
            try {
                backend.call("nodes.forget", JSONObject().put("fp", fp))
                toast("info", "Compute node removed.")
            } catch (e: Exception) {
                toast("error", e.message ?: "Could not remove that compute node.")
            }
            loadNodes()
        }
    }

    fun setNodesEnabled(on: Boolean) {
        _nodes.update { it.copy(enabled = on) }
        viewModelScope.launch {
            try { backend.call("nodes.enable", JSONObject().put("on", on)) }
            catch (e: Exception) { toast("error", e.message ?: "Could not change that.") }
            loadNodes()
        }
    }

    fun refreshNodes() {
        viewModelScope.launch {
            runCatching { backend.call("nodes.refresh") }
            loadNodes()
        }
    }
}

private fun List<Jot>.upsertJot(j: Jot): List<Jot> =
    if (any { it.id == j.id }) map { if (it.id == j.id) j else it } else (this + j).sortedWith(compareBy({ it.t == null }, { it.t ?: 0.0 }, { it.id }))

private fun List<Photo>.upsert(p: Photo): List<Photo> =
    if (any { it.id == p.id }) map { if (it.id == p.id) p else it } else (this + p).sortedWith(compareBy({ it.t == null }, { it.t ?: 0.0 }, { it.id }))

fun formatClock(secs: Double): String {
    val t = secs.coerceAtLeast(0.0).toLong()
    val h = t / 3600; val m = (t / 60) % 60; val s = t % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, s) else "%02d:%02d".format(m, s)
}

/** One sentence for the result of trusting several compute nodes. Needs at least one that answered. */
fun trustSummary(approved: Int, waiting: Int): String {
    fun computers(n: Int) = if (n == 1) "1 compute node" else "$n compute nodes"
    return when {
        waiting == 0 -> "Connected to ${computers(approved)}"
        approved == 0 -> if (waiting == 1) "Waiting for approval" else "Waiting for approval on $waiting compute nodes"
        else -> "Connected to ${computers(approved)}, $waiting ${if (waiting == 1) "is" else "are"} waiting for approval"
    }
}
