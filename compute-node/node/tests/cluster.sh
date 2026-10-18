#!/usr/bin/env bash
# Starts three compute nodes on this machine (own ports, own state folders) and checks that they work together:
# joining, phones paired on one node being accepted on the others, a block spreading, and chat jobs coming back
# from a peer when the node that got them cannot or should not run them.
#
#   compute-node/node/tests/cluster.sh
#
# Needs: a built node (BIN, default target/debug/marginalia-node), jq, and in MODELS the chat model and the
# search model (default ~/.local/share/marginalia/models). Chat runs on the processor (--cpu) so the graphics
# card is not touched, and it is slow: the whole run takes a few minutes.
#
# Layout: a (search only, no chat model) - b (chat) - c (chat). b joined a and c joined b, so a block or an
# allowed phone reaches c through b, and a chat job sent to a has to be answered by b.
#
# Environment: BIN, MODELS, ROOT (state and logs go here, default /tmp/marginalia-cluster-test), BASE_PORT
# (default 47700, uses BASE_PORT+1 to +3 for the nodes and +11 to +13 for the control channels).
set -u

REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
BIN="${BIN:-$REPO/target/debug/marginalia-node}"
MODELS="${MODELS:-$HOME/.local/share/marginalia/models}"
ROOT="${ROOT:-/tmp/marginalia-cluster-test}"
BASE="${BASE_PORT:-47700}"
CHAT_FILE=gemma-4-E2B-it-Q4_0.gguf
EMBED_FILE=embeddinggemma-2-Q8_0.gguf

declare -A PORT=([a]=$((BASE + 1)) [b]=$((BASE + 2)) [c]=$((BASE + 3)))
declare -A ADMIN=([a]=$((BASE + 11)) [b]=$((BASE + 12)) [c]=$((BASE + 13)))
PIDS=()
FAILED=0

say() { printf '\n== %s\n' "$*"; }
ok() { printf '  ok    %s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; FAILED=$((FAILED + 1)); }
check() { # check DESCRIPTION COMMAND...
  local what="$1"
  shift
  if "$@" >/dev/null 2>&1; then ok "$what"; else bad "$what"; fi
}

# node NAME ARGS...: the node binary with that node's state folder and control channel.
node() {
  local n="$1"
  shift
  "$BIN" --state-dir "$ROOT/$n" --admin-port "${ADMIN[$n]}" "$@"
}
status() { node "$1" status; }
served() { status "$1" | jq .served; }
forwarded() { status "$1" | jq .forwarded; }
# field NODE JQ_EXPRESSION: a value from that node's status.
field() { status "$1" | jq -r "$2"; }
# eq_soon SECONDS VALUE COMMAND...: waits until the command prints VALUE.
eq_soon() {
  local end=$((SECONDS + $1)) want="$2"
  shift 2
  while ((SECONDS < end)); do
    if [ "$("$@" 2>/dev/null)" = "$want" ]; then return 0; fi
    sleep 0.5
  done
  return 1
}
# wait_for SECONDS FILE TEXT: waits until the file holds the text.
wait_for() {
  local end=$((SECONDS + $1))
  while ((SECONDS < end)); do
    if grep -q "$3" "$2" 2>/dev/null; then return 0; fi
    sleep 0.5
  done
  return 1
}
# burst NODE_DIR NODE COUNT: sends COUNT chat jobs from that phone to the node 0.3 seconds apart and waits for all.
burst() {
  local dir="$1" n="$2" count="$3" i pids=()
  for ((i = 1; i <= count; i++)); do
    "$BIN" --state-dir "$ROOT/$dir" probe "127.0.0.1:${PORT[$n]}" --only chat >"$ROOT/burst-$i.out" 2>&1 &
    pids+=($!)
    sleep 0.3
  done
  wait "${pids[@]}"
}
code_of() { node "$1" pair | grep -oE '[A-Z0-9]{4}-[A-Z0-9]{4}' | head -1; }
# phone DIR NODE ARGS...: the test phone (probe) with its own identity folder, talking to a node.
phone() {
  local dir="$1" n="$2"
  shift 2
  "$BIN" --state-dir "$ROOT/$dir" probe "127.0.0.1:${PORT[$n]}" "$@"
}

cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  wait 2>/dev/null
}
trap cleanup EXIT

[ -x "$BIN" ] || { echo "no node binary at $BIN (cargo build -p marginalia-node)"; exit 2; }
command -v jq >/dev/null || { echo "jq is needed"; exit 2; }
rm -rf "$ROOT"
mkdir -p "$ROOT/models-search" "$ROOT/models-chat"
ln -s "$MODELS/$EMBED_FILE" "$ROOT/models-search/"
ln -s "$MODELS/$EMBED_FILE" "$MODELS/$CHAT_FILE" "$ROOT/models-chat/"

start() { # start NAME MODELS_FOLDER
  # exec, so that the process id is the node's and not a shell's.
  (exec "$BIN" --state-dir "$ROOT/$1" --admin-port "${ADMIN[$1]}" run --port "${PORT[$1]}" --name "cluster-test-$1" --models "$2" --cpu --no-speech --ctx 2048 --lock >"$ROOT/$1.log" 2>&1) &
  PIDS+=($!)
  eval "PID_$1=$!"
}

say "starting three nodes (a without a chat model, b and c with one), all locked"
start a "$ROOT/models-search"
start b "$ROOT/models-chat"
start c "$ROOT/models-chat"
for n in a b c; do
  wait_for 240 "$ROOT/$n.log" "ready on port" && ok "node $n is ready" || bad "node $n did not start, see $ROOT/$n.log"
done

say "joining: b joins a, c joins b"
node b join "127.0.0.1:${PORT[a]}" "$(code_of a)"
node c join "127.0.0.1:${PORT[b]}" "$(code_of b)"
check "a lists b as a peer" test "$(status a | jq '.peers | length')" = 1
check "b lists a and c as peers" test "$(status b | jq '.peers | length')" = 2
check "c lists b as a peer" test "$(status c | jq '.peers | length')" = 1
eq_soon 15 2 field b '[.peers[] | select(.reachable)] | length' && ok "b reaches both peers" || bad "b does not reach both peers"
check "a peer is not counted as a phone" test "$(status a | jq .phones)" = 0
node b peers
check "peers.json is private to this user" test "$(stat -c %a "$ROOT/a/peers.json")" = 600
check "a wrong code is refused" bash -c "! '$BIN' --state-dir '$ROOT/c' --admin-port ${ADMIN[c]} join 127.0.0.1:${PORT[a]} WRONG-CODE"
node a pair >/dev/null
check "joining a node again is refused" bash -c "! '$BIN' --state-dir '$ROOT/b' --admin-port ${ADMIN[b]} join 127.0.0.1:${PORT[a]} $(code_of a)"

say "a phone paired on a is accepted on b and c without pairing again"
phone phone a --code "$(code_of a)" --only status 2>&1 | tail -1
# Same phone (same identity) knocking at b and c. Both are locked, so only a record from a lets it in.
for n in b c; do
  mkdir -p "$ROOT/phone-$n/probe"
  cp "$ROOT/phone/probe/probe-identity.json" "$ROOT/phone-$n/probe/"
done
check "a phone nobody knows is made to wait by b" bash -c "timeout 6 '$BIN' --state-dir '$ROOT/stranger' probe 127.0.0.1:${PORT[b]} --knock --only status 2>/dev/null | grep -q waiting"
for n in b c; do
  eq_soon 15 1 field $n .phones && ok "$n holds the phone record" || bad "$n never got the phone record"
  if timeout 10 "$BIN" --state-dir "$ROOT/phone-$n" probe "127.0.0.1:${PORT[$n]}" --knock --only status 2>/dev/null | grep -q "let in by"; then ok "the phone is let in by $n"; else bad "the phone was not let in by $n"; fi
done

say "a block on a reaches b and c"
node a block probe
for n in b c; do
  eq_soon 15 1 field $n '.blocked | length' && ok "$n shows the phone as blocked" || bad "$n never got the block"
  if timeout 10 "$BIN" --state-dir "$ROOT/phone-$n" probe "127.0.0.1:${PORT[$n]}" --only status >/dev/null 2>&1; then bad "the blocked phone still got through on $n"; else ok "the blocked phone is turned away by $n"; fi
done
say "an unblock on c reaches a and b"
node c unblock probe
for n in a b; do
  eq_soon 15 0 field $n '.blocked | length' && ok "$n shows no blocked phone" || bad "$n never got the unblock"
done
check "the phone is not let in again by the unblock alone" test "$(status b | jq .phones)" = 0

say "chat: a has no chat model, so a sends a chat job to b"
phone phone a --code "$(code_of a)" --only status >/dev/null
for n in b c; do eq_soon 15 1 field $n .phones || bad "$n never got the phone record again"; done
B0=$(served b)
A_FWD0=$(forwarded a)
phone phone a --only chat | tail -3
check "b answered a chat job from a's phone" test "$(served b)" -gt "$B0"
check "a counted one forwarded job" test "$(forwarded a)" = $((A_FWD0 + 1))
echo "  search on a is answered by a itself (it has the search model):"
phone phone a --only embed | tail -1
check "search stayed on a" test "$(forwarded a)" = $((A_FWD0 + 1))

say "chat: b is paused, so b sends a chat job to c"
node b pause >/dev/null
# a still thinks b is fine for up to two seconds. A job sent to a now goes to b, and b must not pass it on to c.
C0=$(served c)
phone phone a --only chat 2>&1 | tail -2
check "c did not get a job that came to b from a peer" test "$(served c)" = "$C0"
if sed 's/\x1b\[[0-9;]*m//g' "$ROOT/a.log" | grep -q "turned a chat job away (paused)"; then ok "b turned the job from a away instead of passing it on"; else echo "  note  a had already seen that b is paused, so the job never reached b (not exercised this time)"; fi
eq_soon 10 true field a '.peers[0].paused' || bad "a never saw that b is paused"
"$BIN" --state-dir "$ROOT/phone-b" probe "127.0.0.1:${PORT[b]}" --only chat | tail -3
check "c answered a chat job sent to the paused b" test "$(served c)" -gt "$C0"
node b resume >/dev/null

say "chat: b is busy, so jobs sent to b while it works go to c"
B1=$(forwarded b)
C2=$(served c)
burst phone-b b 4
check "b forwarded at least one job" test "$(forwarded b)" -gt "$B1"
check "c answered at least one" test "$(served c)" -gt "$C2"
check "all 4 jobs came back with an answer" test "$(grep -l '^chat: first token' "$ROOT"/burst-*.out | wc -l)" = 4

say "a peer that stops halfway: the node carries on from the words it had"
# Two long jobs go to b: b runs the first and passes the second to c. c is stopped while it writes it.
LONG=("$BIN" --state-dir "$ROOT/phone-b" probe "127.0.0.1:${PORT[b]}" --only chat --long)
"${LONG[@]}" >"$ROOT/long-1.out" 2>&1 &
J1=$!
sleep 1
"${LONG[@]}" >"$ROOT/long-2.out" 2>&1 &
J2=$!
if eq_soon 30 1 field c '.running_jobs'; then
  sleep 3
  kill -9 "$PID_c" 2>/dev/null
  ok "c was stopped while it wrote a job it got from b"
else
  bad "b never passed a job to c"
fi
wait "$J1" "$J2"
check "both long jobs came back with an answer" test "$(grep -l '^chat: first token' "$ROOT"/long-*.out | wc -l)" = 2
check "no job ended in an error" test "$(grep -il 'error' "$ROOT"/long-*.out | wc -l)" = 0
if sed 's/\x1b\[[0-9;]*m//g' "$ROOT/b.log" | grep -q "carrying on here"; then ok "b carried the answer on itself"; else bad "b did not say it carried the answer on"; fi
# c comes back for the next section.
start c "$ROOT/models-chat"
wait_for 240 "$ROOT/c.log" "ready on port" && ok "c is back" || bad "c did not start again"
eq_soon 30 2 field b '[.peers[] | select(.reachable)] | length' && ok "b reaches both peers again" || bad "b does not reach c again"

say "a peer that stops answering: the job runs on the node that got it"
B2=$(served b)
FWD=$(forwarded b)
JOBS=()
"$BIN" --state-dir "$ROOT/phone-b" probe "127.0.0.1:${PORT[b]}" --only chat >"$ROOT/burst-1.out" 2>&1 &
JOBS+=($!)
sleep 0.3
kill -9 "$PID_c" 2>/dev/null
# b has not noticed yet (it asks every 2 seconds), so it sends these to c, hears nothing and runs them itself.
for i in 2 3 4; do
  "$BIN" --state-dir "$ROOT/phone-b" probe "127.0.0.1:${PORT[b]}" --only chat >"$ROOT/burst-$i.out" 2>&1 &
  JOBS+=($!)
  sleep 0.3
done
wait "${JOBS[@]}"
check "all 4 jobs came back with an answer" test "$(grep -l '^chat: first token' "$ROOT"/burst-*.out | wc -l)" = 4
check "b answered the ones c did not" test "$(served b)" -ge $((B2 + 2))
check "b did not count a forwarded job that never came back" test "$(forwarded b)" = "$FWD"
sed 's/\x1b\[[0-9;]*m//g' "$ROOT/b.log" | grep -E "went quiet|turned a chat|not answering" | tail -3

say "unjoin"
node b unjoin cluster-test-c
check "b no longer lists c" test "$(status b | jq '.peers | length')" = 1
node a unjoin cluster-test-b
check "a no longer lists b" test "$(status a | jq '.peers | length')" = 0
check "b was told and no longer lists a" eq_soon 5 0 field b '.peers | length'
echo
if ((FAILED == 0)); then echo "all checks passed"; else echo "$FAILED check(s) failed, logs are in $ROOT"; fi
exit $((FAILED > 0))
