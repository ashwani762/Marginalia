package app.marginalia

import org.json.JSONArray
import org.json.JSONObject

data class Source(
    val n: Int,
    val title: String,
    val locator: String,
    val kind: String,
    val snippet: String,
    val quote: String,
    val docId: Long,
) {
    companion object {
        fun from(o: JSONObject) = Source(
            n = o.optInt("n"),
            title = o.optString("title"),
            locator = o.optString("locator"),
            kind = o.optString("kind"),
            snippet = o.optString("snippet"),
            quote = o.optString("quote"),
            docId = o.optLong("doc_id"),
        )

        fun list(a: JSONArray?): List<Source> = if (a == null) emptyList() else List(a.length()) { from(a.getJSONObject(it)) }
    }
}

data class Line(val i: Int, val t0: Double, val t1: Double, val text: String, val speaker: Int? = null) {
    companion object {
        fun from(o: JSONObject) = Line(o.optInt("i"), o.optDouble("t0"), o.optDouble("t1"), o.optString("text"), if (o.isNull("speaker")) null else o.optInt("speaker"))
        fun list(a: JSONArray?): List<Line> = if (a == null) emptyList() else List(a.length()) { from(a.getJSONObject(it)) }
    }
}

enum class CardState { Retrieving, Answering, Done, NoMatch;
    companion object {
        fun of(s: String) = when (s) {
            "retrieving" -> Retrieving
            "answering" -> Answering
            "nomatch" -> NoMatch
            else -> Done
        }
    }
}

/** [kind] is empty for a lookup in the library, "ask" for a question typed in the meeting, "ideas" for questions to put to the speaker. */
data class Card(val id: Long, val t: Double, val question: String, val answer: String, val state: CardState, val sources: List<Source>, val kind: String = "") {
    companion object {
        fun from(o: JSONObject) = Card(
            id = o.optLong("id"),
            t = o.optDouble("t", Double.MAX_VALUE),
            question = o.optString("question"),
            answer = o.optString("answer"),
            state = CardState.of(o.optString("state")),
            sources = Source.list(o.optJSONArray("sources")),
            kind = o.optString("kind"),
        )

        fun list(a: JSONArray?): List<Card> = if (a == null) emptyList() else List(a.length()) { from(a.getJSONObject(it)) }
    }
}

data class Folder(val id: Long, val name: String, val color: Int, val meetings: Int, val docs: Int) {
    companion object {
        fun from(o: JSONObject) = Folder(o.optLong("id"), o.optString("name"), o.optInt("color"), o.optInt("meetings"), o.optInt("docs"))
        fun list(a: JSONArray?): List<Folder> = if (a == null) emptyList() else List(a.length()) { from(a.getJSONObject(it)) }
    }
}

data class MeetingItem(
    val id: Long,
    val title: String,
    val startedAt: Long,
    val durationSecs: Long,
    val summary: String,
    val notesRaw: String,
    val notesEnhanced: String,
    val segmentCount: Int,
    val cardCount: Int,
    val folderId: Long? = null,
) {
    val hasTitle get() = !(title.isBlank() || title.startsWith("Untitled") || title.startsWith("Meeting ·"))

    companion object {
        fun from(o: JSONObject) = MeetingItem(
            id = o.optLong("id"),
            title = o.optString("title"),
            startedAt = o.optLong("started_at"),
            durationSecs = o.optLong("duration_secs"),
            summary = o.optString("summary"),
            notesRaw = o.optString("notes_raw"),
            notesEnhanced = o.optString("notes_enhanced"),
            segmentCount = o.optInt("segment_count"),
            cardCount = o.optInt("card_count"),
            folderId = if (o.isNull("folder_id")) null else o.optLong("folder_id"),
        )
    }
}

/** A quick note taken in a meeting. [polished] is Gemma's clean version, written once the words around it are known. */
data class Jot(val id: Long, val t: Double?, val kind: String, val raw: String, val polished: String, val ready: Boolean, val updated: Boolean = false) {
    val text get() = polished.ifBlank { raw }
    companion object {
        fun from(o: org.json.JSONObject) = Jot(
            o.optLong("id"), if (o.isNull("t")) null else o.optDouble("t"),
            o.optString("kind", "note"), o.optString("raw"), o.optString("polished"), o.optString("state") == "ready", o.optBoolean("updated"),
        )
        fun list(a: JSONArray?): List<Jot> = if (a == null) emptyList() else List(a.length()) { from(a.getJSONObject(it)) }
    }
}

/** A photo attached to a meeting. [t] is seconds into the meeting, or null if added afterwards. */
data class Photo(val id: Long, val path: String, val t: Double?, val caption: String, val describing: Boolean, val queued: Boolean = false) {
    companion object {
        fun from(o: org.json.JSONObject) = Photo(
            o.optLong("id"), o.optString("path"),
            if (o.isNull("t")) null else o.optDouble("t"),
            o.optString("caption"), o.optString("state") == "describing", o.optString("state") == "queued",
        )
        fun list(a: JSONArray?): List<Photo> = if (a == null) emptyList() else List(a.length()) { from(a.getJSONObject(it)) }
    }
}

data class MeetingDetail(val meeting: MeetingItem, val lines: List<Line>, val cards: List<Card>, val photos: List<Photo> = emptyList(), val jots: List<Jot> = emptyList(), val audio: List<AudioPart> = emptyList(), val speakers: Map<Int, String> = emptyMap())

data class DocItem(
    val id: Long,
    val title: String,
    val source: String,
    val kind: String,
    val bytes: Long,
    val addedAt: Long,
    val chunks: Int,
    val words: Int,
    val folderId: Long? = null,
) {
    companion object {
        fun from(o: JSONObject) = DocItem(
            o.optLong("id"), o.optString("title"), o.optString("source"), o.optString("kind"),
            o.optLong("bytes"), o.optLong("added_at"), o.optInt("chunk_count"), o.optInt("word_count"),
            if (o.isNull("folder_id")) null else o.optLong("folder_id"),
        )
    }
}

/** One passage of a document, in reading order. [page] is 0 when the document has no pages. */
data class DocChunk(val section: String, val page: Int, val text: String)

/** The original file of a document, kept in the app's library. */
data class DocFile(val path: String, val mime: String)

data class DocDetail(val doc: DocItem, val chunks: List<DocChunk>, val file: DocFile? = null)

data class IngestJob(val job: Long, val label: String, val stage: String, val done: Int = 0, val total: Int = 0, val message: String = "") {
    val active get() = stage !in setOf("done", "skipped", "failed")
}

data class ChatTurn(
    val id: Long,
    val question: String,
    val answer: String = "",
    val sources: List<Source> = emptyList(),
    val state: String = "retrieving",
    val stats: String = "",
    val error: String = "",
) {
    val busy get() = state == "retrieving" || state == "generating"
}

data class WrapState(val step: String, val label: String, val progress: Float)

data class Stats(
    val meetings: Long = 0, val seconds: Long = 0, val words: Long = 0,
    val documents: Long = 0, val chunks: Long = 0, val questions: Long = 0,
)

data class Prefs(
    val language: String = "auto",
    val noteStyle: String = "Standard",
    val liveAssistance: Boolean = true,
    val userName: String = "You",
    val appearance: String = "System",
) {
    companion object {
        fun from(o: JSONObject) = Prefs(
            language = o.optString("language", "auto"),
            noteStyle = o.optString("note_style", "Standard"),
            liveAssistance = o.optBoolean("live_assistance", true),
            userName = o.optString("user_name", "You"),
            appearance = o.optString("appearance", "System"),
        )
    }
}

/** Where the engine is, from first launch to ready. */
sealed interface Boot {
    data class Unpacking(val done: Long, val total: Long) : Boot
    data class Loading(val label: String, val step: Int, val steps: Int) : Boot
    data object Ready : Boot
    data class Failed(val message: String) : Boot
}

data class LiveState(
    val meetingId: Long = 0,
    val recording: Boolean = false,
    val secs: Long = 0,
    val lines: List<Line> = emptyList(),
    val partial: String = "",
    val notes: String = "",
    val enhanced: String = "",
    val enhanceStreaming: Boolean = false,
    val enhanceBusy: Boolean = false,
    val cards: List<Card> = emptyList(),
    val wrap: WrapState? = null,
    val summary: String = "",
    val summaryStreaming: Boolean = false,
    val newAssist: Boolean = false,
    val photos: List<Photo> = emptyList(),
    val jots: List<Jot> = emptyList(),
    /** What the background workers are doing right now, in plain words. */
    val activity: List<String> = emptyList(),
)
