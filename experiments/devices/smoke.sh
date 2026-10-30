#!/usr/bin/env bash
# Bring up three devices, put them in one Circle, and check that they work as
# one: task create / claim / conflicting claim / done seen on every device, a
# run log per device from `enox watch --json --log`, and the route each pair
# of devices uses. Exits non-zero on the first check that fails.
#
#   experiments/devices/smoke.sh                 # fresh run (wipes the volumes)
#   FORCE_RELAY_C=1 experiments/devices/smoke.sh # C reaches A and B only via the relay
#   NETEM_C="delay 200ms" experiments/devices/smoke.sh   # slow C's link
#
# Run logs land in experiments/devices/runs/<run id>-<device>.jsonl.
set -euo pipefail
cd "$(dirname "$0")"

CIRCLE=${CIRCLE:-exp}
RUN_ID=${RUN_ID:-smoke-$(date -u +%Y%m%dT%H%M%SZ)}
RELAY_URL=http://relay:3340
RENDEZVOUS=/dns4/relay/tcp/36521

dc() { docker compose "$@"; }
on() { local dev=$1; shift; dc exec -T "dev-$dev" "$@"; }
as() { local dev=$1; shift; dc exec -T -e "ENOXIAN_AGENT_ID=agent-$dev" "dev-$dev" "$@"; }
say() { printf '\n▸ %s\n' "$*"; }
fail() { printf '✗ %s\n' "$*" >&2; exit 1; }
# Poll a command until it succeeds, for at most $1 seconds.
wait_for() {
    local secs=$1; shift
    for _ in $(seq "$((secs * 2))"); do "$@" >/dev/null 2>&1 && return 0; sleep 0.5; done
    return 1
}
api() { # api <dev> <path>: GET a circle API path on that device's daemon
    on "$1" sh -c "curl -sf -H \"Authorization: Bearer \$(cat ~/.enoxian/api.token)\" \
        localhost:36521/circles/$CIRCLE_ID/api/$2"
}
task_id() { on a enox --json --circle "$CIRCLE" tasks | jq -r --arg t "$1" '.[] | select(.title == $t) | .task_id'; }
status_on() { on "$1" enox --json --circle "$CIRCLE" tasks | jq -r --arg id "$2" '.[] | select(.task_id == $id) | "\(.status) \(.claimed_by)"'; }

say "fresh devices"
dc down -v --remove-orphans >/dev/null 2>&1 || true
dc up -d
for d in a b c; do
    wait_for 60 on "$d" enox circles || fail "dev-$d daemon did not come up"
done

say "A creates Circle '$CIRCLE' on the local relay"
CIRCLE_ID=$(on a enox init --name "$CIRCLE" | awk '/circle-id/ {print $3}')
[ -n "$CIRCLE_ID" ] || fail "enox init printed no circle id"
# `enox init` takes no relay flags: point the Circle at the compose relay and
# bootstrap server, which joiners then inherit from the invite.
on a sh -c "sed -i \
    -e 's#^iroh_relays = .*#iroh_relays = [\"$RELAY_URL\"]#' \
    -e 's#^rendezvous_addrs = .*#rendezvous_addrs = [\"$RENDEZVOUS\"]#' \
    ~/.enoxian/circles/$CIRCLE_ID/config.toml"
dc restart dev-a >/dev/null
wait_for 60 api a status || fail "dev-a did not restart"
# The invite embeds A's direct address once its endpoint has published one.
wait_for 30 sh -c "docker compose exec -T dev-a enox invite '$CIRCLE' | grep -q '/ip4/'" \
    || fail "A never published a direct address"

for d in b c; do
    say "$d joins by invite"
    invite=$(on a enox invite "$CIRCLE" | grep -o 'enoxian://s1/[^ ]*')
    on "$d" enox enter "$invite" | tail -1
done

if [ -n "${FORCE_RELAY_C:-}" ]; then
    say "C: force_relay = true"
    on c sh -c "sed -i 's/^force_relay = .*/force_relay = true/' ~/.enoxian/circles/$CIRCLE_ID/config.toml"
    dc restart dev-c >/dev/null
fi

say "every device sees all three online and connected to the other two"
three_connected() {
    for d in a b c; do
        api "$d" who | jq -e '[.[] | select(.status == "online")] | length == 3' >/dev/null || return 1
        api "$d" who | jq -e '[.[] | select(.connections | length > 0)] | length == 2' >/dev/null || return 1
    done
}
wait_for 120 three_connected || { for d in a b c; do on "$d" enox --circle "$CIRCLE" who; done; fail "mesh did not form"; }
for d in a b c; do
    echo "  seen from $d:"
    api "$d" who | jq -r '.[] | "    \(.agent_id)  " + ([.connections[] | "\(.kind) \(.address)"] | join(", "))'
done

if [ -n "${NETEM_C:-}" ]; then
    say "C: tc netem $NETEM_C"
    on c tc qdisc replace dev eth0 root netem $NETEM_C \
        || fail "netem unavailable (host kernel needs sch_netem: sudo modprobe sch_netem)"
    on a ping -c 5 -q dev-c | tail -1
fi

say "run logs → runs/$RUN_ID-<device>.jsonl"
mkdir -p runs
for d in a b c; do
    dc exec -d "dev-$d" sh -c "enox --circle $CIRCLE watch --json --log /runs/$RUN_ID-$d.jsonl \
        --run-id $RUN_ID >/dev/null 2>>/runs/$RUN_ID-$d.err"
done
sleep 1

say "task: A creates, B claims, C is refused, B finishes"
on a enox --circle "$CIRCLE" task-create "smoke: claim and finish" >/dev/null
T1=$(task_id "smoke: claim and finish")
for d in b c; do wait_for 20 sh -c "docker compose exec -T dev-$d enox --json --circle $CIRCLE tasks | grep -q $T1" || fail "$d never saw task $T1"; done
as b enox --circle "$CIRCLE" claim "$T1"
wait_for 20 sh -c "docker compose exec -T dev-c enox --json --circle $CIRCLE tasks | grep -q agent-b" || fail "C never saw B's claim"
if as c enox --circle "$CIRCLE" claim "$T1"; then fail "C claimed a task B holds"; fi
as b enox --circle "$CIRCLE" done "$T1"
for d in a b c; do
    wait_for 20 sh -c "[ \"\$(docker compose exec -T dev-$d enox --json --circle $CIRCLE tasks | jq -r '.[] | select(.task_id==\"$T1\") | .status')\" = done ]" \
        || fail "$d does not show $T1 done"
    echo "  $d: $(status_on "$d" "$T1")"
done

say "task: B and C claim at the same moment"
on a enox --circle "$CIRCLE" task-create "smoke: concurrent claim" >/dev/null
T2=$(task_id "smoke: concurrent claim")
for d in b c; do wait_for 20 sh -c "docker compose exec -T dev-$d enox --json --circle $CIRCLE tasks | grep -q $T2" || fail "$d never saw task $T2"; done
as b enox --circle "$CIRCLE" claim "$T2" & as c enox --circle "$CIRCLE" claim "$T2" & wait || true
sleep 3
holders=$(for d in a b c; do status_on "$d" "$T2"; done | sort -u)
echo "  holder on every device: $holders"
[ "$(echo "$holders" | wc -l)" = 1 ] || fail "devices disagree on who holds $T2"

say "run logs"
sleep 2
for d in a b c; do
    f=runs/$RUN_ID-$d.jsonl
    [ -s "$f" ] || fail "$f is empty"
    printf '  %s: %s lines, kinds: %s\n' "$d" "$(wc -l <"$f")" \
        "$(jq -r .kind "$f" | sort | uniq -c | awk '{printf "%s×%s ", $2, $1}')"
done

say "ok — circle $CIRCLE_ID, run $RUN_ID"
