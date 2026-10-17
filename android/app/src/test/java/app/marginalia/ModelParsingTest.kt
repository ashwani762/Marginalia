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
}
