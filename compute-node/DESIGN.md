# Compute nodes: design

The phone app always works alone. A compute node is an optional helper on the home network that does the
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

## Next: nodes that work together (slice 2)

The goal is that the phone never has to manage nodes. It pairs with one node of a cluster, finds the rest
through that node and through discovery, and sends work to whichever it can reach. The cluster places the
work.

### Finding each other

- Phones browse for every node on the network, as today.
- Nodes also browse for each other (mDNS). Finding a node is not trusting it: two nodes only talk after
  they have paired, the same way a phone does (`marginalia-node join ADDRESS CODE`).
- Paired nodes exchange their member lists, so a node that mDNS cannot see (another subnet, a VPN) is still
  known. A node that moved gets its new address from gossip.

### Membership and what is shared

- A cluster is the set of nodes that have paired with each other. Membership, the list of paired phones and
  per node settings (priority, role) are small records with a version number. They are copied between nodes
  by gossip with last writer wins. Revoking a phone or a node is a record too, so it spreads.
- Blocking a phone (`marginalia-node block`) writes a block record, stronger than a revoke: the phone is
  refused even when connecting is open or it knows a pairing code. Block records spread to all peers like any
  other record and win over an allow record for the same phone, so blocking on one node blocks it on all.
  Unblocking is a newer record. Block records are kept, never forgotten, so a node that was offline learns of
  them when it returns.
- Pair a phone with any one node and every node accepts it. This is configuration, not job data.
- Nothing about a job is replicated. That is why there is no need for consensus (Raft): losing a node loses
  at most the jobs it was running, and those are simply sent again.

### Placing work

- Every node tells its peers its load and speed once or twice a second over a standing peer connection.
- A node that receives a job (the entry node) either runs it or forwards it to the peer with the shortest
  expected wait, counting the forwarding cost. The phone still holds one connection and sees one answer
  stream. The hop between nodes is encrypted like every other connection.
- Placement uses capabilities (which models, which engine, GPU or not), the measured speed and the job
  priority (interactive before background before bulk, fair between phones).
- Idle nodes can take waiting work from a busy peer (work stealing). Jobs are self contained, so this is safe.
- A batch request (`ChatBatch`, `EmbedBatch`) lets the phone submit the sections of a long write-up at once.
  The cluster runs them in parallel on several nodes and streams results back as they finish.

### Modes

- **Hot-hot (default).** Every node serves and any node can be the entry. If one disappears, the phone's
  next request goes to another and an unfinished job is sent again from the start.
- **Primary and standby.** A node can be marked standby. It keeps its models loaded and takes work only when
  the primary stops answering its heartbeat, or when the primary is full. Useful for a small always-on box
  backing up a big machine that sleeps.
- **Roles.** Tags per node: `gpu`, `light` (a Raspberry Pi serving only search), `batch-only`. Placement
  respects them.
- **Distributed model (optional, later).** One large model split across machines with llama.cpp's RPC
  backend. Only worth it on wired networks and for models that do not fit one machine.

### Failure handling

- Before the first token: the job moves to another node, invisibly.
- After tokens have been sent: the error reaches the phone, whose write-up already retries from saved
  parts. Resuming mid-answer by sending the text so far as a prefix is possible and left for later.
- The phone also keeps its own models, so with no node reachable everything still works.

### What the nodes may show

Counts and timings only: jobs served, failures, tokens and passages processed, speed, busy time, uptime,
per phone totals, and the size and duration of recent jobs. Never any text, audio, vectors or titles.
