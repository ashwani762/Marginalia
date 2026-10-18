# Compute nodes: design

The phone app always works alone. A compute node is an optional helper on the local network that does the
slow AI work faster. Nodes keep no user data: a job's text, audio and answer live in memory for the length
of the request and are gone after it.

## What exists now (slice 1)

- One QUIC connection per phone and node pair, one stream per request.
- Pairing with a one-time code, then both sides pin each other's certificate.
- The node announces itself with mDNS (`_marginalia._udp`), the phone discovers it with Android's NSD.
- Jobs: chat (streamed tokens) and search embeddings. The phone core tries a node first and falls back to
  its own models, so no feature depends on a node.
- Each node measures its own speed at start and the phone only uses it when it is clearly faster.
- `marginalia-node stats`: counts, speed, failures and per phone totals. Numbers only, never content.

## Built: peers, shared phone records and placing work (first part of slice 2)

The goal is that the phone never has to manage nodes. It pairs with one node of a cluster and the cluster takes
care of the rest. What is built is the part below. Standby mode, roles and the split model are not (see "Still
designed, not built").

### Peers

- Two nodes become peers with `marginalia-node join ADDRESS CODE` on one of them, after `marginalia-node pair` on
  the other. The joining node acts like a phone: it pairs with the code, then sends a `Join` request on the same
  connection that carries its name, its port and a second proof of the code (`pair::Side::Peer`). Without that
  proof a paired phone could make itself a node. The other node takes the joiner's address from the connection.
  The phone record the pairing made is withdrawn at once (a revoke, which spreads like any other).
- Both sides save the other's certificate fingerprint, name and address in `peers.json` in the state dir (0600).
  Each trusts the other's certificate pinned, as a client (the peer may talk to it) and as a server (it only
  connects to the saved fingerprint). A peer is known to the request handler like a paired phone, plus the right
  to trade records and to leave.
- `marginalia-node peers` lists them, `status` shows them, `unjoin` forgets one (and tells it, if it answers).
- Nodes see each other on the network through mDNS and list the ones they have not joined (`peers`, `status`, the tray window). Seeing is all it does, joining still needs a code from the other node. `join` takes the name or id of a node that was seen as well as an address. Peers do not tell each other about other nodes yet.
  Peers are not transitive for jobs, only for records (a record reaches a peer's peers because a change is sent on).
  A peer that moved gets its new address when it next contacts this node.

### Shared phone records

- Allowed phones, blocked phones and the tombstones of revokes and unblocks are small records `(phone
  fingerprint, name, kind, time in milliseconds)`. A phone has two registers. One holds Allow or Revoke, the
  other Block or Unblock. The later record in a register wins. On a tie Revoke beats Allow and Block beats
  Unblock. A phone is let in when it is allowed and not blocked, so a block beats an allow for the same phone
  until a later unblock. The merge (`records.rs`) is a pure function, the same in any order and any number of
  times, with tests for that.
- A change made here (pair, approve, block, unblock, revoke) is stamped with the later of the clock and one
  millisecond after what this node already knows about that phone, so it always beats what it follows even when
  clocks differ by a little. Real clock differences of seconds can still make two near simultaneous changes on
  different nodes resolve the wrong way round.
- Peers trade their records at once when one changes and every 3 seconds anyway (the `Records` request: each
  side sends its records and gets the other's back). A record taken from a peer is sent on to the other peers,
  so a chain of nodes works, and it stops when every node holds the same set.
- Block records and revokes are kept as tombstones for good, so a node that was off learns of them when it returns.
- Pair a phone with any one node and every node accepts it without pairing again. This is configuration, not
  job data. Nothing about a job is ever sent to a peer except the job it is asked to run.

### Placing work

- Every 2 seconds a node asks each peer for its `Status` and keeps the answer. A peer whose answer is older than
  8 seconds is not counted on.
- A chat or search job that arrives from a phone is placed by `place.rs`. The expected wait of a node is its jobs
  in flight plus the new one, times the time this job takes at its measured speed. A node that is paused, or has no
  model for the job, has no wait. The job is forwarded when this node has no wait, or when its wait is more than
  1.5 times a peer's plus 0.5 seconds for the hop. Several peers are tried best first. Jobs this node has just
  sent to a peer are added to that peer's load, so a burst does not all go to the same peer.
- The answer is relayed through the same stream, so the phone holds one connection to one node and sees one answer
  stream. While the peer is silent the phone is sent a heartbeat every 2 seconds.
- A request from a peer is never forwarded, which is the loop protection: it is recognised by the sender's
  fingerprint being a peer's.
- Before the first token, a peer that cannot be reached, refuses (paused, no model) or is silent for 10 seconds is
  skipped and the next peer is tried, then this node. After the first token an error reaches the phone, whose
  write-up retries from saved parts.
- Transcription is not forwarded: the phone already chooses nodes for it and a recording needs a long upload.
- Load is the number of jobs in flight of any kind, because that is what `Status` has. A node busy with a long
  recording looks busy for chat too.

### Still designed, not built

- Peers handing each other their member lists (another subnet, a VPN), and joining nodes that were seen without typing a code on both sides.
- Per node settings (priority, role) as records, and revoking a node as a record. Leaving is `unjoin` on each side.
- Idle nodes taking waiting work from a busy peer (work stealing), and fair sharing between phones.
- A batch request (`ChatBatch`, `EmbedBatch`) that runs the sections of a long write-up in parallel on several nodes.
- Standby mode, roles and the split model below.

### Modes

- **Hot-hot (default, built).** Every node serves and any node can be the entry. If one disappears, the phone's
  next request goes to another and an unfinished job is sent again from the start.
- **Primary and standby (not built).** A node can be marked standby. It keeps its models loaded and takes work only when
  the primary stops answering its heartbeat, or when the primary is full. Useful for a small always-on box
  backing up a big machine that sleeps.
- **Roles (not built).** Tags per node: `gpu`, `light` (a Raspberry Pi serving only search), `batch-only`. Placement
  respects them.
- **Distributed model (not built, optional, later).** One large model split across machines with llama.cpp's RPC
  backend. Only worth it on wired networks and for models that do not fit one machine.

### Failure handling (built for chat and search jobs)

- Before the first token: the job moves to another node, invisibly.
- After tokens have been sent: the error reaches the phone, whose write-up already retries from saved
  parts. Resuming mid-answer by sending the text so far as a prefix is possible and left for later.
- The phone also keeps its own models, so with no node reachable everything still works.

### What the nodes may show

Counts and timings only: jobs served, failures, tokens and passages processed, speed, busy time, uptime,
per phone totals, and the size and duration of recent jobs. Never any text, audio, vectors or titles.
