package app.marginalia

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** What the screens receive from the Rust core is JSON. These keep both sides honest. */
class ModelParsingTest {
    @Test fun jotReadsStateAndUpdatedFlag() {
        val j = Jot.from(JSONObject("""{"id":4,"t":12.5,"kind":"decision","raw":"ship fri","polished":"We ship on Friday.","state":"ready","updated":true}"""))
        assertEquals(4L, j.id)
        assertEquals(12.5, j.t!!, 0.0)
        assertTrue(j.ready)
        assertTrue(j.updated)
        assertEquals("We ship on Friday.", j.text)
    }

    @Test fun pendingJotShowsTheRawWordsUntilPolished() {
        val j = Jot.from(JSONObject("""{"id":5,"t":null,"kind":"note","raw":"call priya","polished":"","state":"pending"}"""))
        assertNull(j.t)
        assertFalse(j.ready)
        assertFalse(j.updated)
        assertEquals("call priya", j.text)
    }

    @Test fun photoStatesMapToFlags() {
        val queued = Photo.from(JSONObject("""{"id":1,"path":"/p.jpg","t":3.0,"caption":"","state":"queued"}"""))
        val reading = Photo.from(JSONObject("""{"id":2,"path":"/q.jpg","t":null,"caption":"","state":"describing"}"""))
        assertTrue(queued.queued && !queued.describing)
        assertTrue(reading.describing && !reading.queued)
        assertNull(reading.t)
    }

    @Test fun meetingTitleCountsAsRealOnlyWhenNotTheDefault() {
        fun m(title: String) = MeetingItem.from(JSONObject("""{"id":1,"title":"$title","started_at":1,"duration_secs":2,"summary":"","notes_raw":"","notes_enhanced":"","segment_count":0,"card_count":0,"folder_id":null}"""))
        assertFalse(m("Meeting · Oct 16, 14:32").hasTitle)
        assertFalse(m("Untitled meeting").hasTitle)
        assertFalse(m("").hasTitle)
        assertTrue(m("Orchid launch plan").hasTitle)
        assertNull(m("x").folderId)
    }

    @Test fun listsTolerateNull() {
        assertTrue(Jot.list(null).isEmpty())
        assertEquals(2, Photo.list(JSONArray("""[{"id":1,"path":"a"},{"id":2,"path":"b"}]""")).size)
    }

    @Test fun homeComputersParse() {
        val h = HomeComputers.from(JSONObject("""{"enabled":true,"paired":[{"fp":"ab12","name":"Desk","online":true,"rtt_ms":4,"chat":"gemma","embed":null,"chat_tok_s":160.0,"embed_per_s":9.5,"busy":1,"problem":""}],"found":[{"id":"0123456789abcdef","name":"Den","addrs":["192.168.1.5"]}]}"""))
        assertTrue(h.enabled)
        assertEquals("Desk", h.paired[0].name)
        assertNull(h.paired[0].embed)
        assertEquals(listOf("192.168.1.5"), h.found[0].addrs)
        val (status, detail) = app.marginalia.ui.statusOf(h.paired[0])
        assertEquals("Online, fast", status)
        assertEquals("Answers at 120 words a second. Busy", detail)
        assertEquals("ABCD1234", app.marginalia.ui.cleanCode("abcd-1234 99"))
    }

    @Test fun homeComputerStatesParse() {
        val h = HomeComputers.from(JSONObject("""{"enabled":true,"paired":[
            {"fp":"aa","name":"Desk","online":true,"state":"approved","pending":[{"id":"p1","name":"Pixel","waited_s":12},{"id":"p2","name":"","waited_s":3}]},
            {"fp":"bb","name":"Den","online":false,"state":"waiting","pending":[]},
            {"fp":"cc","name":"Attic","online":false,"state":"refused","problem":"No"},
            {"fp":"dd","name":"Old","online":true}]}"""))
        val (desk, den, attic, old) = h.paired
        assertEquals("approved", desk.state)
        assertTrue(desk.approved)
        assertEquals(2, desk.pending.size)
        assertEquals("p1", desk.pending[0].id)
        assertEquals("Pixel", desk.pending[0].name)
        assertEquals(12, desk.pending[0].waitedS)
        assertEquals("", desk.pending[1].name)
        assertEquals("waiting", den.state)
        assertFalse(den.approved)
        assertTrue(den.pending.isEmpty())
        assertEquals("refused", attic.state)
        assertFalse(attic.approved)
        assertEquals("approved", old.state)
        assertTrue(old.pending.isEmpty())
        assertEquals("Not allowed by this computer", app.marginalia.ui.statusOf(attic).first)
        assertEquals("Waiting", app.marginalia.ui.statusOf(den).first)
        assertEquals("76a8 ae41", app.marginalia.ui.shortId("76a8ae41ffff0000"))
        assertEquals("", app.marginalia.ui.shortId(""))
    }

    @Test fun approvalsGroupByPhone() {
        val h = HomeComputers.from(JSONObject("""{"paired":[
            {"fp":"f","name":"falcon","state":"approved","pending":[{"id":"p1","name":"Pixel 8"},{"id":"p2","name":"Moto"}]},
            {"fp":"h","name":"hawk","state":"approved","pending":[{"id":"p1","name":"Pixel 8"}]},
            {"fp":"w","name":"owl","state":"waiting","pending":[{"id":"p1","name":"Pixel 8"}]}]}"""))
        val a = groupApprovals(h.paired, emptySet())!!
        assertEquals("p1", a.who)
        assertEquals("Pixel 8", a.phone)
        assertEquals(listOf("f", "h"), a.computers.map { it.fp })
        val b = groupApprovals(h.paired, setOf("p1"))!!
        assertEquals("p2", b.who)
        assertEquals(listOf("f"), b.computers.map { it.fp })
        assertNull(groupApprovals(h.paired, setOf("p1", "p2")))
        assertEquals(setOf("p1", "p2"), askingPhones(h.paired))
        assertEquals("falcon and hawk", app.marginalia.ui.joinNames(listOf("falcon", "hawk")))
        assertEquals("a, b and c", app.marginalia.ui.joinNames(listOf("a", "b", "c")))
    }

    @Test fun trustSummaryWords() {
        assertEquals("Connected to 3 computers", trustSummary(3, 0))
        assertEquals("Connected to 2 computers, 1 is waiting for approval", trustSummary(2, 1))
        assertEquals("Connected to 1 computer, 2 are waiting for approval", trustSummary(1, 2))
        assertEquals("Waiting for approval", trustSummary(0, 1))
    }

    @Test fun unpairedFoundSkipsPaired() {
        val n = HomeComputers.from(JSONObject("""{"paired":[{"fp":"76a8ae41ffff"}],"found":[{"id":"76a8ae41","name":"a","addrs":[]},{"id":"0123","name":"b","addrs":[]}]}"""))
        assertEquals(listOf("b"), app.marginalia.ui.unpairedFound(n).map { it.name })
    }
}
