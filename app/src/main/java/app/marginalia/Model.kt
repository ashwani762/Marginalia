// Plain data classes for what the native core sends to the screens, with their JSON parsing.

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

/**
 * A line of the transcript. [i] is its position and changes when lines before it are replaced, so lists on screen are
 * keyed by [id], which stays with the line (a line that is corrected in place keeps it).
 */
data class Line(val i: Int, val t0: Double, val t1: Double, val text: String, val speaker: Int? = null, val id: Long = i.toLong()) {
    companion object {
        fun from(o: JSONObject): Line {
            val i = o.optInt("i")
            return Line(i, o.optDouble("t0"), o.optDouble("t1"), o.optString("text"), if (o.isNull("speaker")) null else o.optInt("speaker"), o.optLong("id", i.toLong()))
        }
        fun list(a: JSONArray?): List<Line> = if (a == null) emptyList() else List(a.length()) { from(a.getJSONObject(it)) }
    }
}

/** A line that arrived (or grew) is put where the line with its id is, else at the end. Positions are renumbered. */
fun List<Line>.upsertLine(line: Line): List<Line> {
    val at = indexOfFirst { it.id == line.id }
    val out = toMutableList()
    if (at >= 0) out[at] = line else out.add(line)
    return out.renumbered()
}

/** [removed] lines from [from] are taken out and [new] ones put in their place (the live transcript was corrected). */
fun List<Line>.replacingLines(from: Int, removed: Int, new: List<Line>): List<Line> {
    val a = from.coerceIn(0, size)
    val b = (a + removed.coerceAtLeast(0)).coerceAtMost(size)
    val out = ArrayList<Line>(size - (b - a) + new.size)
    out.addAll(subList(0, a))
    out.addAll(new)
    out.addAll(subList(b, size))
    // A new line must never share an id with one that stays, or two rows would get the same key.
    val seen = HashSet<Long>()
    return out.filter { seen.add(it.id) }.renumbered()
}

private fun List<Line>.renumbered(): List<Line> = mapIndexed { n, l -> if (l.i == n) l else l.copy(i = n) }

/** "Corrected by Desk" for the live screen, with the node's name kept to the house style. */
fun correctionLabel(node: String?): String? {
    val name = node?.replace("—", ", ")?.replace(";", ".")?.trim().orEmpty()
    return if (node == null) null else if (name.isEmpty()) "Corrected by a compute node" else "Corrected by $name"
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
    /** After a meeting, a compute node listens to the recording again with a larger model. */
    val improveTranscript: Boolean = true,
    /** While recording, a compute node listens again in short windows and fixes the captions. */
    val liveCorrection: Boolean = false,
    /** Seconds of talk per correction window: 20, 30, 45 or 60. */
    val liveCorrectionWindowS: Int = DEFAULT_CORRECTION_WINDOW_S,
) {
    companion object {
        fun from(o: JSONObject) = Prefs(
            language = o.optString("language", "auto"),
            noteStyle = o.optString("note_style", "Standard"),
            liveAssistance = o.optBoolean("live_assistance", true),
            userName = o.optString("user_name", "You"),
            appearance = o.optString("appearance", "System"),
            improveTranscript = o.optBoolean("improve_transcript", true),
            liveCorrection = o.optBoolean("live_correction", false),
            liveCorrectionWindowS = o.optInt("live_correction_window_s", DEFAULT_CORRECTION_WINDOW_S).let { if (it in CORRECTION_WINDOWS_S) it else DEFAULT_CORRECTION_WINDOW_S },
        )
    }
}

const val DEFAULT_CORRECTION_WINDOW_S = 30
val CORRECTION_WINDOWS_S = listOf(20, 30, 45, 60)

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
    /** The compute node that is fixing the captions while recording, else null. */
    val correctedBy: String? = null,
)

/** A compute node on the home network that has been paired with this phone. */
data class HomeComputer(
    val fp: String,
    val name: String,
    val online: Boolean,
    val rttMs: Int,
    val chat: String?,
    val embed: String?,
    val chatTokPerSec: Double,
    val embedPerSec: Double,
    val busy: Int,
    val problem: String,
    /** "approved" (in use), "waiting" (the compute node has not allowed this phone yet) or "refused". */
    val state: String = "approved",
    /** Other phones asking to use this compute node. Only filled when this phone is approved there. */
    val pending: List<PendingPhone> = emptyList(),
    /** Name of the speech model when this compute node can listen to a recording again, else null. */
    val transcribe: String? = null,
    /** The longest correction window, in seconds, that this compute node takes as a quick job. 0 when it does not say. */
    val windowMaxS: Int = 0,
) {
    val approved: Boolean get() = state != "waiting" && state != "refused"

    companion object {
        fun from(o: JSONObject) = HomeComputer(
            fp = o.optString("fp"),
            name = o.optString("name"),
            online = o.optBoolean("online", false),
            rttMs = o.optInt("rtt_ms", 0),
            chat = if (o.isNull("chat")) null else o.optString("chat").ifBlank { null },
            embed = if (o.isNull("embed")) null else o.optString("embed").ifBlank { null },
            chatTokPerSec = o.optDouble("chat_tok_s", 0.0).let { if (it.isNaN()) 0.0 else it },
            embedPerSec = o.optDouble("embed_per_s", 0.0).let { if (it.isNaN()) 0.0 else it },
            busy = o.optInt("busy", 0),
            problem = o.optString("problem"),
            state = o.optString("state").lowercase().ifBlank { "approved" },
            pending = o.optJSONArray("pending")?.objects().orEmpty().map(PendingPhone::from),
            transcribe = if (o.isNull("transcribe")) null else o.optString("transcribe").ifBlank { null },
            windowMaxS = o.optInt("window_max_s", 0),
        )
    }
}

/**
 * The compute node that could listen to a recording again right now: switched on, approved, reachable and with a
 * speech model. Null when there is none, and then "Improve transcript" is not offered.
 */
fun transcribeNode(n: HomeComputers): HomeComputer? =
    if (!n.enabled) null else n.paired.firstOrNull { it.approved && it.online && it.transcribe != null }

/** Whether the Review screen offers "Improve transcript": a kept recording, a compute node that can listen, and nothing else being written for the meeting. */
fun canImproveTranscript(n: HomeComputers, hasAudio: Boolean, busy: Boolean): Boolean = hasAudio && !busy && transcribeNode(n) != null

/** Whether "Correct the transcript while recording" can be used: a compute node with the speech model is online. */
fun canCorrectLive(n: HomeComputers): Boolean = transcribeNode(n) != null

/**
 * A short note under the window choice when the compute node takes only shorter windows than the person chose
 * (a window is the chosen talk plus 5 seconds that the phone repeats). Null when the choice fits or nothing is known.
 */
fun windowLimitNote(chosenS: Int, nodeMaxS: Int): String? {
    if (nodeMaxS <= 0 || chosenS + 5 <= nodeMaxS) return null
    return "This compute node handles windows up to $nodeMaxS s, so they are cut shorter."
}

/** Another phone that is asking to use a home compute node. */
data class PendingPhone(val id: String, val name: String, val waitedS: Int) {
    companion object {
        fun from(o: JSONObject) = PendingPhone(o.optString("id"), o.optString("name"), o.optInt("waited_s", 0))
    }
}

/** A compute node the network search has seen that is not paired yet. */
data class FoundComputer(val id: String, val name: String, val addrs: List<String>) {
    companion object {
        fun from(o: JSONObject) = FoundComputer(o.optString("id"), o.optString("name"), o.strings("addrs"))
    }
}

data class HomeComputers(
    val enabled: Boolean = false,
    val paired: List<HomeComputer> = emptyList(),
    val found: List<FoundComputer> = emptyList(),
) {
    companion object {
        fun from(o: JSONObject) = HomeComputers(
            enabled = o.optBoolean("enabled", false),
            paired = o.optJSONArray("paired")?.objects().orEmpty().map(HomeComputer::from),
            found = o.optJSONArray("found")?.objects().orEmpty().map(FoundComputer::from),
        )
    }
}

/** One phone asking to use one or more compute nodes. The person answers once for all of them. */
data class ApprovalAsk(val who: String, val phone: String, val computers: List<AskedComputer>)

data class AskedComputer(val fp: String, val name: String)

/** Groups the pending phones of every approved compute node by phone id and returns the first phone not in [dismissed] (a set of phone ids). */
fun groupApprovals(paired: List<HomeComputer>, dismissed: Set<String>): ApprovalAsk? {
    val byPhone = LinkedHashMap<String, MutableList<Pair<HomeComputer, PendingPhone>>>()
    paired.filter { it.approved }.forEach { c -> c.pending.forEach { ph -> byPhone.getOrPut(ph.id) { mutableListOf() }.add(c to ph) } }
    val (id, rows) = byPhone.entries.firstOrNull { it.key !in dismissed } ?: return null
    return ApprovalAsk(id, rows.map { it.second.name }.firstOrNull { it.isNotBlank() }.orEmpty(), rows.map { AskedComputer(it.first.fp, it.first.name) })
}

/** The ids of all phones that are asking for something right now. */
fun askingPhones(paired: List<HomeComputer>): Set<String> = paired.flatMap { c -> c.pending.map { it.id } }.toSet()

/** Compute nodes the network search has seen that are not paired yet. */
fun unpairedFound(n: HomeComputers): List<FoundComputer> =
    n.found.filter { f -> n.paired.none { f.id.isNotBlank() && it.fp.startsWith(f.id, ignoreCase = true) } }

/** What a found compute node is remembered by when the person says "Not now". Falls back to the name when there is no id. */
fun foundKey(f: FoundComputer): String = f.id.trim().lowercase().ifBlank { "name:" + f.name.trim().lowercase() }

/** The found compute nodes the banner should still offer: not paired yet and not dismissed. Nothing while home compute nodes are switched off. */
fun computersToOffer(n: HomeComputers, dismissed: Set<String>): List<FoundComputer> =
    if (!n.enabled) emptyList() else unpairedFound(n).filter { foundKey(it) !in dismissed }

/** "1 compute node found on your network", "3 compute nodes found on your network". */
fun offerTitle(count: Int): String = if (count == 1) "1 compute node found on your network" else "$count compute nodes found on your network"

/** The small status chip text for the approved compute nodes that are online right now, or null when there are none. */
fun onlineChipText(paired: List<HomeComputer>): String? {
    val on = paired.filter { it.approved && it.online }
    return when (on.size) {
        0 -> null
        1 -> on[0].name.trim().takeIf { it.isNotBlank() }?.let { "Compute node: $it" } ?: "Compute node"
        else -> "Compute nodes: ${on.size}"
    }
}
