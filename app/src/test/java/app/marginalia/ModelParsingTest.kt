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
        assertEquals("Not allowed by this compute node", app.marginalia.ui.statusOf(attic).first)
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
        assertEquals("Connected to 3 compute nodes", trustSummary(3, 0))
        assertEquals("Connected to 2 compute nodes, 1 is waiting for approval", trustSummary(2, 1))
        assertEquals("Connected to 1 compute node, 2 are waiting for approval", trustSummary(1, 2))
        assertEquals("Waiting for approval", trustSummary(0, 1))
    }

    @Test fun unpairedFoundSkipsPaired() {
        val n = HomeComputers.from(JSONObject("""{"paired":[{"fp":"76a8ae41ffff"}],"found":[{"id":"76a8ae41","name":"a","addrs":[]},{"id":"0123","name":"b","addrs":[]}]}"""))
        assertEquals(listOf("b"), unpairedFound(n).map { it.name })
    }

    private fun nodes(json: String) = HomeComputers.from(JSONObject(json))

    @Test fun offerSkipsDismissedButShowsNewComputers() {
        val n = nodes("""{"enabled":true,"paired":[],"found":[{"id":"AA11","name":"falcon","addrs":[]},{"id":"bb22","name":"hawk","addrs":[]}]}""")
        assertEquals(listOf("falcon", "hawk"), computersToOffer(n, emptySet()).map { it.name })
        // The key is lower case, so the saved set matches whatever case the id comes in.
        assertEquals(listOf("hawk"), computersToOffer(n, setOf("aa11")).map { it.name })
        assertTrue(computersToOffer(n, setOf("aa11", "bb22")).isEmpty())
        val later = nodes("""{"enabled":true,"paired":[],"found":[{"id":"aa11","name":"falcon","addrs":[]},{"id":"cc33","name":"owl","addrs":[]}]}""")
        assertEquals(listOf("owl"), computersToOffer(later, setOf("aa11", "bb22")).map { it.name })
    }

    @Test fun offerSkipsPairedAndSwitchedOff() {
        val paired = nodes("""{"enabled":true,"paired":[{"fp":"aa11ffff"}],"found":[{"id":"aa11","name":"falcon","addrs":[]}]}""")
        assertTrue(computersToOffer(paired, emptySet()).isEmpty())
        val off = nodes("""{"enabled":false,"paired":[],"found":[{"id":"aa11","name":"falcon","addrs":[]}]}""")
        assertTrue(computersToOffer(off, emptySet()).isEmpty())
    }

    @Test fun computerWithoutIdIsRememberedByName() {
        val f = FoundComputer("", " Falcon ", listOf("192.168.1.5"))
        assertEquals("name:falcon", foundKey(f))
        val n = HomeComputers(enabled = true, found = listOf(f))
        assertTrue(computersToOffer(n, setOf(foundKey(f))).isEmpty())
    }

    @Test fun offerTitleCounts() {
        assertEquals("1 compute node found on your network", offerTitle(1))
        assertEquals("3 compute nodes found on your network", offerTitle(3))
    }

    private fun node(name: String, online: Boolean, state: String = "approved", paused: Boolean = false) =
        HomeComputer.from(JSONObject("""{"fp":"$name","name":"$name","online":$online,"state":"$state","paused":$paused}"""))

    @Test fun chipTextForZeroOneAndSeveralRunning() {
        assertNull(nodeChip(emptyList()))
        // Waiting or refused nodes show nothing, and nothing shows while compute nodes are switched off.
        assertNull(nodeChip(listOf(node("hawk", true, "waiting"), node("owl", true, "refused"))))
        assertNull(nodeChip(listOf(node("falcon", true)), enabled = false))
        assertEquals(NodeChip("Compute node: falcon", NodeLight.Running), nodeChip(listOf(node("falcon", true), node("hawk", false))))
        assertEquals(NodeChip("Compute nodes: 2", NodeLight.Running), nodeChip(listOf(node("falcon", true), node("hawk", true), node("owl", false))))
        assertEquals(NodeChip("Compute node", NodeLight.Running), nodeChip(listOf(node("", true))))
    }

    @Test fun chipIsYellowWhenEveryReachableNodeIsPausedAndRedWhenNoneAnswers() {
        assertEquals(NodeChip("Compute node: falcon is paused", NodeLight.Paused), nodeChip(listOf(node("falcon", true, paused = true))))
        assertEquals(NodeChip("Compute nodes: 2 paused", NodeLight.Paused), nodeChip(listOf(node("a", true, paused = true), node("b", true, paused = true), node("c", false))))
        // One running node is enough for green.
        assertEquals(NodeChip("Compute node: b", NodeLight.Running), nodeChip(listOf(node("a", true, paused = true), node("b", true))))
        assertEquals(NodeChip("Compute node: falcon is not reachable", NodeLight.Stopped), nodeChip(listOf(node("falcon", false))))
        assertEquals(NodeChip("Compute node is not reachable", NodeLight.Stopped), nodeChip(listOf(node("", false))))
        assertEquals(NodeChip("Compute nodes: none reachable", NodeLight.Stopped), nodeChip(listOf(node("a", false), node("b", false))))
    }

    @Test fun lightFollowsOnlineAndPaused() {
        assertEquals(NodeLight.Running, node("a", true).light)
        assertEquals(NodeLight.Paused, node("a", true, paused = true).light)
        assertEquals(NodeLight.Stopped, node("a", false).light)
        // A node that does not answer is stopped, whatever it said last.
        assertEquals(NodeLight.Stopped, node("a", false, paused = true).light)
        assertFalse(HomeComputer.from(JSONObject("""{"fp":"a","name":"a","online":true}""")).paused)
    }

    @Test fun settingsSaysPausedOnTheNode() {
        assertEquals("Paused on the node" to null, app.marginalia.ui.statusOf(node("a", true, paused = true)))
        assertEquals("Not reachable on this network" to null, app.marginalia.ui.statusOf(node("a", false)))
    }

    @Test fun aPausedNodeIsNotOfferedTheRecording() {
        fun n(vararg list: HomeComputer) = HomeComputers(enabled = true, paired = list.toList())
        fun c(paused: Boolean) = HomeComputer.from(JSONObject().put("fp", "x").put("name", "Desk").put("online", true).put("paused", paused).put("transcribe", "large"))
        assertNull(transcribeNode(n(c(true))))
        assertEquals("Desk", transcribeNode(n(c(true), c(false)))!!.name)
        assertFalse(canCorrectLive(n(c(true))))
    }

    @Test fun computerReadsWhatItCanTranscribe() {
        val with = HomeComputer.from(JSONObject("""{"fp":"a","name":"Desk","online":true,"transcribe":"large-v3-turbo"}"""))
        assertEquals("large-v3-turbo", with.transcribe)
        assertNull(HomeComputer.from(JSONObject("""{"fp":"a","name":"Desk","online":true,"transcribe":null}""")).transcribe)
        assertNull(HomeComputer.from(JSONObject("""{"fp":"a","name":"Desk","online":true}""")).transcribe)
        assertNull(HomeComputer.from(JSONObject("""{"fp":"a","name":"Desk","online":true,"transcribe":""}""")).transcribe)
    }

    @Test fun improvingNeedsAnOnlineComputerThatCanListen() {
        fun c(online: Boolean, transcribe: String?, state: String = "approved") =
            HomeComputer.from(JSONObject().put("fp", "x").put("name", "Desk").put("online", online).put("state", state).put("transcribe", transcribe ?: JSONObject.NULL))
        fun n(enabled: Boolean, vararg list: HomeComputer) = HomeComputers(enabled = enabled, paired = list.toList())
        assertEquals("Desk", transcribeNode(n(true, c(true, "large")))!!.name)
        assertNull(transcribeNode(n(true, c(false, "large"))))
        assertNull(transcribeNode(n(true, c(true, null))))
        assertNull(transcribeNode(n(true, c(true, "large", "waiting"))))
        assertNull(transcribeNode(n(false, c(true, "large"))))
        assertNull(transcribeNode(n(true)))
        // The second one can listen even if the first cannot.
        assertEquals("large", transcribeNode(n(true, c(true, null), c(true, "large")))!!.transcribe)

        val ok = n(true, c(true, "large"))
        assertTrue(canImproveTranscript(ok, hasAudio = true, busy = false))
        assertFalse(canImproveTranscript(ok, hasAudio = false, busy = false))
        assertFalse(canImproveTranscript(ok, hasAudio = true, busy = true))
        assertFalse(canImproveTranscript(n(true, c(false, "large")), hasAudio = true, busy = false))
    }

    @Test fun improvingTranscriptsIsOnByDefault() {
        assertTrue(Prefs.from(JSONObject("""{}""")).improveTranscript)
        assertFalse(Prefs.from(JSONObject("""{"improve_transcript":false}""")).improveTranscript)
    }

    private fun line(i: Int, id: Long, text: String = "t$id") = Line(i, i * 10.0, i * 10.0 + 8, text, null, id)

    @Test fun lineEventsFindTheLineById() {
        val parsed = Line.from(JSONObject("""{"i":4,"id":91,"t0":1.5,"t1":3.0,"text":"hi","speaker":null}"""))
        assertEquals(91L, parsed.id)
        // An older core without ids falls back to the position.
        assertEquals(4L, Line.from(JSONObject("""{"i":4,"t0":1.5,"t1":3.0,"text":"hi"}""")).id)
        val lines = listOf(line(0, 10), line(1, 11))
        val grown = lines.upsertLine(line(1, 11, "grown"))
        assertEquals(listOf("t10", "grown"), grown.map { it.text })
        val added = grown.upsertLine(line(9, 12))
        assertEquals(listOf(10L, 11L, 12L), added.map { it.id })
        assertEquals(listOf(0, 1, 2), added.map { it.i })
    }

    @Test fun aCorrectionSplicesTheLinesAndKeepsIdsUnique() {
        val lines = listOf(line(0, 10), line(1, 11), line(2, 12), line(3, 13))
        // Lines 1 and 2 become three lines. The first keeps an old id, the others are new.
        val out = lines.replacingLines(1, 2, listOf(line(1, 11, "fixed a"), line(2, 20, "fixed b"), line(3, 21, "fixed c")))
        assertEquals(listOf(10L, 11L, 20L, 21L, 13L), out.map { it.id })
        assertEquals(listOf("t10", "fixed a", "fixed b", "fixed c", "t13"), out.map { it.text })
        assertEquals((0..4).toList(), out.map { it.i })
        assertEquals(out.size, out.map { it.id }.toSet().size)
        // Fewer lines than before, none at all, and lines added in a gap.
        assertEquals(listOf(10L, 13L), lines.replacingLines(1, 2, emptyList()).map { it.id })
        assertEquals(listOf(10L, 11L, 30L, 12L, 13L), lines.replacingLines(2, 0, listOf(line(2, 30))).map { it.id })
        // At the end, and a range that reaches past the end is cut to it.
        assertEquals(listOf(10L, 11L, 12L, 13L, 31L), lines.replacingLines(4, 0, listOf(line(4, 31))).map { it.id })
        assertEquals(listOf(10L, 40L), lines.replacingLines(1, 99, listOf(line(1, 40))).map { it.id })
        // A repeated id is dropped rather than shown twice (a stale event must never crash the list).
        assertEquals(listOf(10L, 11L, 12L, 13L), lines.replacingLines(1, 1, listOf(line(1, 11), line(2, 12))).map { it.id })
    }

    @Test fun liveCorrectionSettingsAndTheNodeWindow() {
        val d = Prefs.from(JSONObject("""{}"""))
        assertFalse(d.liveCorrection)
        assertEquals(20, d.liveCorrectionWindowS)
        val p = Prefs.from(JSONObject("""{"live_correction":true,"live_correction_window_s":45}"""))
        assertTrue(p.liveCorrection)
        assertEquals(45, p.liveCorrectionWindowS)
        // An odd value falls back to 30.
        assertEquals(20, Prefs.from(JSONObject("""{"live_correction_window_s":33}""")).liveCorrectionWindowS)
        assertEquals(listOf(20, 30, 45, 60), CORRECTION_WINDOWS_S)
        assertEquals(90, HomeComputer.from(JSONObject("""{"fp":"a","name":"Desk","online":true,"window_max_s":90}""")).windowMaxS)
        assertEquals(0, HomeComputer.from(JSONObject("""{"fp":"a","name":"Desk","online":true}""")).windowMaxS)
    }

    @Test fun correctionNeedsANodeThatCanListenAndSaysWhoDoesIt() {
        fun c(online: Boolean, transcribe: String?) =
            HomeComputer.from(JSONObject().put("fp", "x").put("name", "Desk").put("online", online).put("transcribe", transcribe ?: JSONObject.NULL))
        assertTrue(canCorrectLive(HomeComputers(enabled = true, paired = listOf(c(true, "large")))))
        assertFalse(canCorrectLive(HomeComputers(enabled = true, paired = listOf(c(true, null)))))
        assertFalse(canCorrectLive(HomeComputers(enabled = true, paired = listOf(c(false, "large")))))
        assertFalse(canCorrectLive(HomeComputers(enabled = false, paired = listOf(c(true, "large")))))
        assertEquals("Desk will check the captions every 20 s", correctionLabel("Desk"))
        assertEquals("Desk will check the captions every 45 s", correctionLabel("Desk", windowS = 45))
        assertEquals("A compute node will check the captions every 20 s", correctionLabel(" "))
        assertEquals("Den. Up will check the captions every 20 s", correctionLabel("Den; Up"))
        assertEquals("Desk is checking 00:25 to 00:55", correctionLabel("Desk", "working", 25.0, 55.0))
        assertEquals("Checked by Desk up to 00:55", correctionLabel("Desk", "idle", checkedUpto = 55.0))
        assertEquals("Checked by Desk up to 01:05", correctionLabel("Desk", "working".let { "idle" }, checkedUpto = 65.0))
        assertNull(correctionLabel(null))
        assertNull(windowLimitNote(30, 0))
        assertNull(windowLimitNote(30, 120))
        assertNull(windowLimitNote(60, 65))
        assertEquals("This compute node handles windows up to 40 s, so they are cut shorter.", windowLimitNote(60, 40))
    }
}
