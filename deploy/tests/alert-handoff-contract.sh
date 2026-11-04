#!/usr/bin/env bash
# Alert handoff contract (Phase 10 Task 28, plan section 6.4).
#
#   bash deploy/tests/alert-handoff-contract.sh
#       Static ABI checks of the reviewed native unit and policy bytes in
#       tests/rust_contract/phase10/alert-handoff/ (portable, no root).
#   bash deploy/tests/alert-handoff-contract.sh --disposable-identities
#       The static checks, then the three-identity suite: a disposable systemd
#       container (Linux, rootless podman or docker) gets the real
#       verdaxis-health, verdaxis-backup-reader:verdaxis-backup-readers and
#       verdaxis-monitor-alert identities from the exact sysusers, tmpfiles and
#       ACL bytes, the native units and the Linux verdaxis-read binary named by
#       VERDAXIS_READ_BIN. Producers and the dispatcher run under those
#       identities and unit sandboxes against loopback fake services and fake
#       credentials. Missing Linux, container, systemd or ACL support fails.
#
# Root is used only inside the disposable container, to stage fixtures. No
# monitor runs as root, no installed host path or live service is touched.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
policy="$root/tests/rust_contract/phase10/alert-handoff"
binary_path=/usr/local/libexec/verdaxis/verdaxis-read
failures=0

pass() { printf 'PASS %s\n' "$1"; }
fail() {
    printf 'FAIL %s: %s\n' "$1" "$2"
    failures=$((failures + 1))
}

# check LABEL COMMAND...: one assertion; the label names the cell. A failure
# is reported and counted, and the remaining assertions still run.
check() {
    local label=$1
    shift
    if ! "$@"; then
        fail "$label" "$*"
    fi
}

has_line() { grep -Fxq -- "$2" "$1"; }
lacks_text() { ! grep -Fq -- "$2" "$1"; }

# ---- static contract --------------------------------------------------------------

static_unit_abi() {
    local cell=alert_handoff_unit_abi before=$failures check_name unit
    for check_name in health backup; do
        if [ "$check_name" = health ]; then
            unit="$policy/verdaxis-health.service"
            check "$cell" has_line "$unit" "ExecStart=$binary_path monitor health"
            check "$cell" has_line "$unit" "User=verdaxis-health"
        else
            unit="$policy/verdaxis-backup-verify.service"
            check "$cell" has_line "$unit" "ExecStart=$binary_path monitor backup-verify"
            check "$cell" has_line "$unit" "User=verdaxis-backup-reader"
            check "$cell" has_line "$unit" "Group=verdaxis-backup-readers"
        fi
        # Check-only wakeups; a refusal stays a failed unit.
        check "$cell" has_line "$unit" "OnFailure=verdaxis-monitor-alert@$check_name.service"
        check "$cell" has_line "$unit" "OnSuccess=verdaxis-monitor-alert@$check_name.service"
        check "$cell" has_line "$unit" "StateDirectoryMode=0710"
        check "$cell" lacks_text "$unit" "SuccessExitStatus"
        check "$cell" lacks_text "$unit" "ExecStart=-"
        check "$cell" lacks_text "$unit" "Restart="
        check "$cell" lacks_text "$unit" "EnvironmentFile"
    done
    unit="$policy/verdaxis-monitor-alert@.service"
    check "$cell" has_line "$unit" "ExecStart=$binary_path monitor alert --event %i"
    check "$cell" has_line "$unit" "RestartPreventExitStatus=2 3 5"
    check "$cell" has_line "$unit" "Restart=on-failure"
    check "$cell" has_line "$unit" "RestartSec=60s"
    check "$cell" has_line "$unit" "StartLimitBurst=16"
    unit="$policy/verdaxis-monitor-alert-reminder.service"
    check "$cell" has_line "$unit" "ExecStart=$binary_path monitor alert --remind all"
    for unit in "$policy/verdaxis-monitor-alert@.service" "$policy/verdaxis-monitor-alert-reminder.service"; do
        check "$cell" has_line "$unit" "User=verdaxis-monitor-alert"
        check "$cell" has_line "$unit" "EnvironmentFile=/etc/verdaxis-monitor/alert.env"
        check "$cell" has_line "$unit" "ReadWritePaths=-/var/lib/verdaxis-health/alert-handoff -/var/lib/verdaxis-backup-verify/alert-handoff -/var/lib/verdaxis-monitor-alert-status"
    done
    for unit in "$policy"/*.service; do
        check "$cell" lacks_text "$unit" "python"
        check "$cell" lacks_text "$unit" "venv"
    done
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

# Every sandbox line of the current unit survives in the reviewed unit; only
# the command, wakeup, state mode, timeout and description lines change.
static_sandbox_kept() {
    local cell=alert_handoff_unit_sandbox_kept before=$failures name line
    for name in verdaxis-health.service verdaxis-backup-verify.service \
        verdaxis-monitor-alert@.service verdaxis-monitor-alert-reminder.service; do
        while IFS= read -r line; do
            case "$line" in
                ExecStart=* | OnFailure=* | OnSuccess=* | StateDirectoryMode=* | TimeoutStartSec=* | Description=* | "") continue ;;
            esac
            check "$cell" has_line "$policy/$name" "$line"
        done < "$root/deploy/monitor/$name"
    done
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

static_policy() {
    local cell=alert_handoff_policy_bytes before=$failures acl="$policy/verdaxis-monitor-handoff.acl.tmpfiles"
    # Handoff paths are created in the ACL file itself; the base file creates
    # the producer state directories with the ACL mask mode (0710).
    check "$cell" has_line "$acl" \
        "d /var/lib/verdaxis-health/alert-handoff 0700 verdaxis-health verdaxis-health -"
    check "$cell" has_line "$acl" \
        "d /var/lib/verdaxis-backup-verify/alert-handoff 0700 verdaxis-backup-reader verdaxis-backup-readers -"
    check "$cell" lacks_text "$policy/verdaxis-monitor.tmpfiles" "alert-handoff"
    check "$cell" has_line "$policy/verdaxis-monitor.tmpfiles" \
        "d /var/lib/verdaxis-health 0710 verdaxis-health verdaxis-health -"
    check "$cell" has_line "$policy/verdaxis-monitor.tmpfiles" \
        "d /var/lib/verdaxis-monitor-alert-status 0755 verdaxis-monitor-alert verdaxis-monitor-alert -"
    check "$cell" test "$(grep -c '^a ' "$acl")" -eq 6
    check "$cell" lacks_text "$acl" "verdaxis-monitor-alert:rw-"
    check "$cell" lacks_text "$acl" "g::r"
    check "$cell" lacks_text "$acl" "o::r"
    check "$cell" lacks_text "$acl" "/var/lib/verdaxis-monitor-alert"
    # Neither producer gains a group that reaches the other or the receipts.
    check "$cell" lacks_text "$root/deploy/monitor/verdaxis-monitor.sysusers" "m verdaxis-health"
    check "$cell" lacks_text "$root/deploy/monitor/verdaxis-monitor.sysusers" "m verdaxis-monitor-alert"
    # The fence: Python opens the stable delivery lock for writing, so the
    # native read-only (0400) lock refuses a direct Python invocation.
    check "$cell" grep -Fq 'os.O_CREAT | os.O_RDWR' "$root/deploy/monitor/alert_dispatch.py"
    check "$cell" grep -Fq 'const FENCED_LOCK_MODE: u32 = 0o400;' "$root/rs/read/src/cli/monitors/alert.rs"
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

# ---- disposable target: host side -------------------------------------------------------

engine=""
container=""
volume=""
stage=""
image_tag="localhost/verdaxis-alert-handoff:bookworm"

cleanup_target() {
    if [ -n "$container" ]; then "$engine" rm -f "$container" >/dev/null 2>&1 || true; fi
    if [ -n "$volume" ]; then "$engine" volume rm -f "$volume" >/dev/null 2>&1 || true; fi
    if [ -n "$stage" ]; then rm -rf "$stage"; fi
    if [ -n "$engine" ] && [ "${VERDAXIS_KEEP_TEST_IMAGE:-0}" != 1 ]; then
        "$engine" rmi -f "$image_tag" >/dev/null 2>&1 || true
    fi
}

write_fake_services() {
    cat > "$1" <<'PY'
"""Loopback fakes: two readiness endpoints and one alert destination."""
import json, os, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STATE = "/run/fake"
SHA = sys.argv[1]

class Ready(BaseHTTPRequestHandler):
    environment = "production"
    def do_GET(self):
        failing = os.path.exists(os.path.join(STATE, "ready-fail"))
        body = json.dumps({"status": "ok", "db": "ok", "environment": self.environment,
                           "release_sha": SHA}).encode()
        self.send_response(503 if failing else 200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass

class Staging(Ready):
    environment = "staging"

class Destination(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length).decode() if length else ""
        text = json.loads(body).get("text", "") if body else ""
        try:
            with open(os.path.join(STATE, "dest-status")) as handle:
                mode = handle.read().strip()
        except OSError:
            mode = "200"
        if mode == "hang":
            time.sleep(15)
            mode = "200"
        status = int(mode)
        with open(os.path.join(STATE, "requests.log"), "a") as log:
            log.write(json.dumps({"path": self.path, "text": text, "status": status}) + "\n")
        reply = json.dumps({"ok": status == 200}).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(reply)))
        self.end_headers()
        self.wfile.write(reply)
    def log_message(self, *args):
        pass

servers = [ThreadingHTTPServer(("127.0.0.1", 8000), Ready),
           ThreadingHTTPServer(("127.0.0.1", 8001), Staging),
           ThreadingHTTPServer(("127.0.0.1", 18080), Destination)]
for server in servers[1:]:
    threading.Thread(target=server.serve_forever, daemon=True).start()
servers[0].serve_forever()
PY
}

build_image() {
    if "$engine" image exists "$image_tag" >/dev/null 2>&1 \
        || "$engine" image inspect "$image_tag" >/dev/null 2>&1; then
        return 0
    fi
    local context="$stage/image"
    mkdir -p "$context"
    printf 'FROM docker.io/library/debian:bookworm-slim\nRUN apt-get update && apt-get install -y --no-install-recommends systemd systemd-sysv acl ca-certificates python3 util-linux procps && rm -rf /var/lib/apt/lists/*\n' \
        > "$context/Containerfile"
    "$engine" build -t "$image_tag" -f "$context/Containerfile" "$context" >/dev/null
}

start_target() {
    local -a options=(-d --name "$container" --network none --hostname alert-handoff
        -v "$volume:/var/lib" -v "$stage:/stage:ro")
    if [ "$engine" = podman ]; then
        options+=(--systemd=always)
    else
        options+=(--privileged --cgroupns=private --tmpfs /run --tmpfs /run/lock)
    fi
    "$engine" run "${options[@]}" "$image_tag" /sbin/init >/dev/null
    wait_target
}

wait_target() {
    local state
    for _ in $(seq 1 90); do
        state=$("$engine" exec "$container" systemctl is-system-running 2>/dev/null || true)
        case "$state" in
            running | degraded) return 0 ;;
        esac
        sleep 1
    done
    echo "systemd in the disposable target did not start (state: ${state:-none})" >&2
    return 1
}

inside() {
    "$engine" exec -e VERDAXIS_DISPOSABLE_TARGET=1 "$container" \
        bash /stage/alert-handoff-contract.sh --inside-disposable-target "$@"
}

disposable_identities() {
    local cell=alert_handoff_disposable_target
    # The engine supplies Linux (including Colima on macOS). The target's
    # systemd/ACL checks below remain mandatory; the client OS is irrelevant.
    engine=${VERDAXIS_CONTAINER_ENGINE:-}
    if [ -z "$engine" ]; then
        if command -v podman >/dev/null 2>&1; then engine=podman; else engine=docker; fi
    fi
    if ! command -v "$engine" >/dev/null 2>&1; then
        fail "$cell" "no container engine ($engine)"
        return
    fi
    local binary=${VERDAXIS_READ_BIN:-${CARGO_TARGET_DIR:-$root/rs/target}/debug/verdaxis-read}
    if [ ! -x "$binary" ] || [ "$(head -c 4 "$binary" | od -An -c | tr -d ' ')" != '177ELF' ]; then
        fail "$cell" "VERDAXIS_READ_BIN must name a Linux verdaxis-read binary"
        return
    fi
    container="verdaxis-alert-handoff-$$"
    volume="verdaxis-alert-handoff-$$"
    stage=$(mktemp -d "${TMPDIR:-/tmp}/alert-handoff.XXXXXX")
    trap cleanup_target EXIT
    cp "$binary" "$stage/verdaxis-read"
    cp "$policy"/* "$stage/"
    cp "$root/deploy/monitor/verdaxis-monitor.sysusers" "$root/deploy/monitor/verdaxis-monitor-alert-reminder.timer" \
        "$root/deploy/monitor/alert_dispatch.py" "$root/deploy/monitor/status_state.py" "$stage/"
    cp "$root/tests/rust_contract/phase10/backup/sample-dump.sql" "$stage/"
    cp "${BASH_SOURCE[0]}" "$stage/alert-handoff-contract.sh"
    write_fake_services "$stage/fake_services.py"
    chmod -R a+rX "$stage"
    if ! build_image; then
        fail "$cell" "could not build the disposable systemd image"
        return
    fi
    if ! start_target; then
        fail "$cell" "could not start the disposable systemd target"
        return
    fi
    pass "$cell"
    local status=0
    inside --before-reboot || status=$?
    # A reboot of the disposable target: state survives, tmpfiles reapplies.
    if "$engine" restart "$container" >/dev/null && wait_target; then
        inside --after-reboot || status=$?
    else
        fail alert_handoff_restart_and_reboot "the disposable target did not reboot"
    fi
    if [ "$status" -ne 0 ]; then
        failures=$((failures + 1))
    fi
}

# ---- disposable target: inside the container (root, disposable only) -----------------------

sha=0123456789abcdef0123456789abcdef01234567
alert_dir=/var/lib/verdaxis-monitor-alert
health_dir=/var/lib/verdaxis-health
backup_dir=/var/lib/verdaxis-backup-verify
requests=/run/fake/requests.log

as_user() {
    local user=$1
    shift
    runuser -u "$user" -- "$@"
}

# denied LABEL USER COMMAND...: the command must fail with EACCES or EPERM.
denied() {
    local label=$1 user=$2 output
    shift 2
    if output=$(runuser -u "$user" -- "$@" 2>&1); then
        fail "$label" "allowed for $user: $*"
        return 1
    fi
    case "$output" in
        *"Permission denied"* | *"Operation not permitted"*) return 0 ;;
    esac
    fail "$label" "failed for another reason ($user: $*): $output"
    return 1
}

allowed() {
    local label=$1 user=$2
    shift 2
    if ! runuser -u "$user" -- "$@" >/dev/null 2>&1; then
        fail "$label" "denied for $user: $*"
        return 1
    fi
}

start_fakes() {
    mkdir -p /run/fake
    chmod 0777 /run/fake
    systemctl stop fake-services.service >/dev/null 2>&1 || true
    systemctl reset-failed fake-services.service >/dev/null 2>&1 || true
    systemd-run --unit fake-services -p User=nobody python3 /stage/fake_services.py "$sha" >/dev/null
    for _ in $(seq 1 50); do
        if python3 -c 'import socket; [socket.create_connection(("127.0.0.1", p), 1).close() for p in (8000, 8001, 18080)]' 2>/dev/null; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

ready_fail() { touch /run/fake/ready-fail; }
ready_ok() { rm -f /run/fake/ready-fail; }
destination_status() { echo "$1" > /run/fake/dest-status; }
request_count() { if [ -f "$requests" ]; then wc -l < "$requests"; else echo 0; fi; }
texts_since() { tail -n +"$(($1 + 1))" "$requests" 2>/dev/null | python3 -c 'import json,sys; [print(json.loads(l)["text"] or json.loads(l)["path"]) for l in sys.stdin]'; }

# Wait until a unit has no job, is not activating and is not waiting to restart.
wait_settled() {
    local unit=$1 state sub
    for _ in $(seq 1 120); do
        state=$(systemctl show -p ActiveState --value "$unit")
        sub=$(systemctl show -p SubState --value "$unit")
        # A unit waiting for its restart has finished this run.
        if [ "$sub" = auto-restart ]; then
            return 0
        fi
        if [ "$state" != activating ] \
            && ! systemctl list-jobs --no-legend 2>/dev/null | grep -Fq "$unit"; then
            return 0
        fi
        sleep 1
    done
    echo "unit $unit did not settle" >&2
    return 1
}

# run_producer SERVICE: start the oneshot, wait for its wakeup, print its exit status.
run_producer() {
    local service=$1 check=$2
    systemctl reset-failed "$service" "verdaxis-monitor-alert@$check.service" >/dev/null 2>&1 || true
    systemctl start "$service" >/dev/null 2>&1 || true
    wait_settled "verdaxis-monitor-alert@$check.service" || true
    local status
    status=$(systemctl show -p ExecMainStatus --value "$service")
    trace_run "$service" "$check" "$status" >&2
    printf '%s\n' "$status"
}

# One trace line per producer run: its exit, its status codes (fixed codes
# only) and the dispatcher's last report line.
trace_run() {
    local service=$1 check=$2 status=$3 directory codes
    if [ "$check" = health ]; then directory=$health_dir; else directory=$backup_dir; fi
    codes=$(python3 -c 'import json, sys; print(",".join(c["name"] + ":" + c["code"] for c in json.load(open(sys.argv[1]))["checks"]))' \
        "$directory/status.json" 2>/dev/null)
    printf 'RUN %s exit=%s codes=%s dispatcher=%s requests=%s\n' "$service" "$status" "$codes" \
        "$(journalctl -o cat --no-pager -n 20 -u "verdaxis-monitor-alert@$check.service" 2>/dev/null | grep -F '"command"' | tail -n 1)" \
        "$(request_count)"
}

wake() {
    systemctl reset-failed "verdaxis-monitor-alert@$1.service" >/dev/null 2>&1 || true
    systemctl start "verdaxis-monitor-alert@$1.service" >/dev/null 2>&1 || true
    wait_settled "verdaxis-monitor-alert@$1.service" || true
}

pending() { find "$1/alert-handoff" -maxdepth 1 -name '[1-9]*.json' -printf '%f\n' | sort -n; }
digest() { sha256sum "$1" | cut -d' ' -f1; }
acl_of() { getfacl --omit-header --absolute-names "$1" 2>/dev/null | grep -v '^$'; }

write_backup() {
    local directory=/home/verdaxis-prod/backups
    install -d -o verdaxis-prod -g verdaxis-backup-readers -m 0750 "$directory" "$directory/attempts"
    as_user verdaxis-prod python3 - "$directory" /stage/sample-dump.sql <<'PY'
import datetime, gzip, json, os, sys
directory, dump = sys.argv[1], sys.argv[2]
at = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0) - datetime.timedelta(hours=1)
backup_id = at.strftime("%Y%m%d-%H%M%S")
data = gzip.compress(open(dump, "rb").read())
for prefix in ("verdaxis-prod", "verdaxis-staging", "umami"):
    path = os.path.join(directory, f"{prefix}-{backup_id}.sql.gz")
    with open(path, "wb") as handle:
        handle.write(data)
    os.chmod(path, 0o640)
    os.utime(path, (at.timestamp(), at.timestamp()))
def write(path, value):
    with open(path, "w") as handle:
        json.dump(value, handle)
    os.chmod(path, 0o640)
write(os.path.join(directory, "status.json"), {"ok": True, "completed_at": at.isoformat(),
      "backup_id": backup_id, "databases": ["verdaxis_prod", "verdaxis_staging", "umami"]})
write(os.path.join(directory, "attempts", f"attempt-{backup_id}.json"), {"schema_version": 1,
      "attempt_id": f"attempt-{backup_id}", "state": "succeeded",
      "started_at": (at - datetime.timedelta(minutes=5)).isoformat(),
      "finished_at": at.isoformat(), "backup_id": backup_id})
PY
    # The backup producer publishes for the readers group.
    chgrp -R verdaxis-backup-readers "$directory"
}

break_backup() { rm -rf /home/verdaxis-prod/backups/*; }

setup_target() {
    local cell=alert_handoff_identity_setup before=$failures
    id verdaxis-prod >/dev/null 2>&1 || useradd --system --home-dir /home/verdaxis-prod --create-home verdaxis-prod
    install -m 0644 /stage/verdaxis-monitor.sysusers /usr/lib/sysusers.d/verdaxis-monitor.conf
    check "$cell" systemd-sysusers
    install -m 0644 /stage/verdaxis-monitor.tmpfiles /usr/lib/tmpfiles.d/verdaxis-monitor.conf
    install -m 0644 /stage/verdaxis-monitor-handoff.acl.tmpfiles /usr/lib/tmpfiles.d/verdaxis-monitor-handoff-acl.conf
    install -d -m 0755 /etc/verdaxis-monitor
    printf 'TELEGRAM_BOT_TOKEN=fake-token-0042\nTELEGRAM_CHAT_ID=fake-chat-0042\nTELEGRAM_API_BASE=http://127.0.0.1:18080\nHEALTHCHECKS_HEALTH_URL=http://127.0.0.1:18080/hc-health\nHEALTHCHECKS_BACKUP_URL=http://127.0.0.1:18080/hc-backup\n' \
        > /etc/verdaxis-monitor/alert.env
    check "$cell" systemd-tmpfiles --create /usr/lib/tmpfiles.d/verdaxis-monitor.conf \
        /usr/lib/tmpfiles.d/verdaxis-monitor-handoff-acl.conf
    check "$cell" test "$(stat -c '%U %a' /var/lib/verdaxis-monitor-alert-status)" = "verdaxis-monitor-alert 755"
    for environment in production staging; do
        printf 'ENVIRONMENT=%s\nRELEASE_SHA=%s\n' "$environment" "$sha" \
            > "/etc/verdaxis-monitor/runtime-identities/$environment.identity"
        chmod 0644 "/etc/verdaxis-monitor/runtime-identities/$environment.identity"
    done
    # The health monitor probes only the host's installed endpoint list (Task
    # 27 I2); here it names the two loopback readiness fakes.
    printf '%s\n' '{"version": 1, "endpoints": [' \
        '{"name": "prod_ready", "url": "http://127.0.0.1:8000/health/ready", "environment": "production"},' \
        '{"name": "staging_ready", "url": "http://127.0.0.1:8001/health/ready", "environment": "staging"}]}' \
        > /etc/verdaxis-monitor/health-endpoints.json
    chmod 0644 /etc/verdaxis-monitor/health-endpoints.json
    install -d -o verdaxis-prod -g verdaxis-prod -m 0750 /home/verdaxis-prod/verdaxis
    install -o verdaxis-prod -g verdaxis-prod -m 0600 /dev/null /home/verdaxis-prod/.backup-env
    install -d -m 0755 /usr/local/libexec/verdaxis
    install -o root -g root -m 0755 /stage/verdaxis-read "$binary_path"
    for unit in verdaxis-health.service verdaxis-backup-verify.service verdaxis-monitor-alert@.service \
        verdaxis-monitor-alert-reminder.service verdaxis-monitor-alert-reminder.timer; do
        install -m 0644 "/stage/$unit" "/etc/systemd/system/$unit"
    done
    systemctl daemon-reload
    check "$cell" start_fakes
    destination_status 200
    check "$cell" test "$(stat -c %a /etc/verdaxis-monitor/alert.env)" = 600
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
    printf 'TARGET nproc=%s systemd=%s\n' "$(nproc)" "$(systemctl --version | head -n 1)"
}

# Both producers enqueue under their own identities and the dispatcher delivers.
cell_enqueue_and_dispatch() {
    local cell=alert_handoff_real_identities_enqueue_and_dispatch before=$failures count
    count=$(request_count)
    ready_fail
    check "$cell" test "$(run_producer verdaxis-health.service health)" = 2
    check "$cell" test "$(stat -c '%U %a' "$health_dir/status.json")" = "verdaxis-health 600"
    ready_ok
    check "$cell" test "$(run_producer verdaxis-health.service health)" = 0
    break_backup
    check "$cell" test "$(run_producer verdaxis-backup-verify.service backup)" = 2
    write_backup
    check "$cell" test "$(run_producer verdaxis-backup-verify.service backup)" = 0
    check "$cell" test "$(stat -c '%U %G %a' "$backup_dir/status.json")" = "verdaxis-backup-reader verdaxis-backup-readers 600"
    local texts
    texts=$(texts_since "$count" | grep -F 'Verdaxis local' | tr '\n' '|')
    check "$cell" test "$texts" = "Verdaxis local health: FAILURE|Verdaxis local health: RECOVERY|Verdaxis local backup: FAILURE|Verdaxis local backup: RECOVERY|"
    check "$cell" grep -Fq '"/hc-health/fail"' "$requests"
    check "$cell" grep -Fq '"/hc-backup"' "$requests"
    check "$cell" test -z "$(pending "$health_dir")"
    check "$cell" test -z "$(pending "$backup_dir")"
    check "$cell" test "$(stat -c '%U %a' "$alert_dir/health.json")" = "verdaxis-monitor-alert 600"
    check "$cell" grep -Fq '"state":"healthy"' "$alert_dir/health.json"
    # The accepted unit limits, with the full binary.
    check "$cell" test "$(systemctl show -p MemoryMax --value verdaxis-health.service)" = 67108864
    check "$cell" test "$(systemctl show -p MemoryMax --value verdaxis-backup-verify.service)" = 134217728
    check "$cell" test "$(systemctl show -p TasksMax --value verdaxis-health.service)" = 16
    allowed "$cell" verdaxis-health test -x "$binary_path"
    allowed "$cell" verdaxis-backup-reader test -x "$binary_path"
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

# Leave one admitted failure per check whose delivery keeps failing.
retain_failures() {
    destination_status 503
    ready_fail
    run_producer verdaxis-health.service health >/dev/null
    break_backup
    run_producer verdaxis-backup-verify.service backup >/dev/null
}

cell_producer_isolation() {
    local cell=alert_handoff_producers_cannot_read_other_check_receipts_or_credentials before=$failures
    local health_entry backup_entry user
    health_entry="$health_dir/alert-handoff/$(pending "$health_dir" | head -n 1)"
    backup_entry="$backup_dir/alert-handoff/$(pending "$backup_dir" | head -n 1)"
    check "$cell" test -f "$health_entry"
    check "$cell" test -f "$backup_entry"
    denied "$cell" verdaxis-health ls "$backup_dir"
    denied "$cell" verdaxis-health cat "$backup_entry"
    denied "$cell" verdaxis-health rm -f "$backup_entry"
    denied "$cell" verdaxis-health touch "$backup_dir/alert-handoff/99.json"
    denied "$cell" verdaxis-health cat "$backup_dir/backup.sequence.json"
    denied "$cell" verdaxis-backup-reader ls "$health_dir"
    denied "$cell" verdaxis-backup-reader cat "$health_entry"
    denied "$cell" verdaxis-backup-reader rm -f "$health_entry"
    denied "$cell" verdaxis-backup-reader touch "$health_dir/alert-handoff/99.json"
    for user in verdaxis-health verdaxis-backup-reader; do
        denied "$cell" "$user" ls "$alert_dir"
        denied "$cell" "$user" cat "$alert_dir/health.json"
        denied "$cell" "$user" cat "$alert_dir/backup.delivery.json"
        denied "$cell" "$user" cat /etc/verdaxis-monitor/alert.env
    done
    # Unrelated identities, including a member of the backup readers group.
    for user in nobody verdaxis-prod; do
        denied "$cell" "$user" cat "$health_entry"
        denied "$cell" "$user" cat "$backup_entry"
        denied "$cell" "$user" ls "$health_dir/alert-handoff"
        denied "$cell" "$user" ls "$backup_dir/alert-handoff"
        denied "$cell" "$user" cat "$alert_dir/health.json"
        denied "$cell" "$user" cat /etc/verdaxis-monitor/alert.env
    done
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

expect_acl() {
    local cell=$1 path=$2 expected=$3
    if [ "$(acl_of "$path" | tr '\n' ' ')" != "$expected" ]; then
        fail "$cell" "ACL of $path: $(acl_of "$path" | tr '\n' ' ')"
    fi
}

cell_acl_modes() {
    local cell=alert_handoff_acl_modes_and_dispatcher_read_remove_only before=$failures dir check_name entry
    for check_name in health backup; do
        if [ "$check_name" = health ]; then dir=$health_dir; else dir=$backup_dir; fi
        entry="$dir/alert-handoff/$(pending "$dir" | head -n 1)"
        expect_acl "$cell" "$dir" "user::rwx user:verdaxis-monitor-alert:--x group::--- mask::--x other::--- "
        expect_acl "$cell" "$dir/.$check_name.journal.lock" "user::rw- user:verdaxis-monitor-alert:r-- group::--- mask::r-- other::--- "
        expect_acl "$cell" "$dir/alert-handoff" "user::rwx user:verdaxis-monitor-alert:rwx group::--- mask::rwx other::--- default:user::rwx default:user:verdaxis-monitor-alert:r-- default:group::--- default:mask::r-- default:other::--- "
        expect_acl "$cell" "$entry" "user::rw- user:verdaxis-monitor-alert:r-- group::--- mask::r-- other::--- "
        expect_acl "$cell" "$dir/$check_name.sequence.json" "user::rw- group::--- other::--- "
        expect_acl "$cell" "$dir/status.json" "user::rw- group::--- other::--- "
        check "$cell" test "$(stat -c %a "$entry")" = 640
        # The dispatcher reads, locks and may remove; it cannot write or list.
        allowed "$cell" verdaxis-monitor-alert cat "$entry"
        allowed "$cell" verdaxis-monitor-alert flock -n "$dir/.$check_name.journal.lock" true
        denied "$cell" verdaxis-monitor-alert sh -c "echo x >> '$entry'"
        denied "$cell" verdaxis-monitor-alert chmod 0666 "$entry"
        denied "$cell" verdaxis-monitor-alert setfacl -m u:verdaxis-monitor-alert:rw- "$entry"
        denied "$cell" verdaxis-monitor-alert sh -c "echo x >> '$dir/.$check_name.journal.lock'"
        denied "$cell" verdaxis-monitor-alert ls "$dir"
        denied "$cell" verdaxis-monitor-alert cat "$dir/$check_name.sequence.json"
        denied "$cell" verdaxis-monitor-alert sh -c "echo x > '$dir/$check_name.sequence.json'"
        denied "$cell" verdaxis-monitor-alert cat "$dir/status.json"
        as_user "$(stat -c %U "$dir")" touch "$dir/alert-handoff/scratch-remove"
        allowed "$cell" verdaxis-monitor-alert rm -f "$dir/alert-handoff/scratch-remove"
    done
    # The retained heads are spaced, not exhausted: a minute between the first
    # two attempts.
    check "$cell" grep -Fq '"blocked":null' "$alert_dir/health.delivery.json"
    check "$cell" grep -Fq '"attempts":1' "$alert_dir/health.delivery.json"
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

# Re-arm and drain every retained head with the destination working again.
drain_all() {
    local check_name dir sequence
    destination_status 200
    for check_name in health backup; do
        if [ "$check_name" = health ]; then dir=$health_dir; else dir=$backup_dir; fi
        for _ in 1 2 3 4; do
            sequence=$(pending "$dir" | head -n 1)
            [ -n "$sequence" ] || break
            as_user verdaxis-monitor-alert "$binary_path" monitor alert --rearm "$check_name:${sequence%.json}" >/dev/null 2>&1 || true
            wake "$check_name"
        done
    done
}

# refused_run CELL SERVICE CHECK DIR: the producer exits 4, fails, publishes nothing.
refused_run() {
    local cell=$1 service=$2 check_name=$3 dir=$4 status_before entries_before
    status_before=$(digest "$dir/status.json")
    entries_before=$(pending "$dir" | tr '\n' ' ')
    systemctl reset-failed "$service" >/dev/null 2>&1 || true
    systemctl start "$service" >/dev/null 2>&1 || true
    wait_settled "verdaxis-monitor-alert@$check_name.service" || true
    check "$cell" test "$(systemctl show -p ExecMainStatus --value "$service")" = 4
    check "$cell" test "$(systemctl show -p Result --value "$service")" = exit-code
    check "$cell" systemctl is-failed --quiet "$service"
    check "$cell" test "$(digest "$dir/status.json")" = "$status_before"
    check "$cell" test "$(pending "$dir" | tr '\n' ' ')" = "$entries_before"
}

cell_failed_enqueue() {
    local cell=alert_failed_enqueue_has_failed_unit_and_no_status_publication before=$failures holder
    drain_all
    ready_ok
    write_backup
    run_producer verdaxis-health.service health >/dev/null
    run_producer verdaxis-backup-verify.service backup >/dev/null
    check "$cell" grep -Fq '"category":"healthy"' "$health_dir/status.json"
    # Journal-lock timeout on a healthy observation, for both producers: the
    # failed unit is what the external monitor's unit check reads.
    as_user verdaxis-health flock "$health_dir/.health.journal.lock" sleep 40 &
    holder=$!
    sleep 1
    refused_run "$cell" verdaxis-health.service health "$health_dir"
    kill "$holder" 2>/dev/null || true
    wait "$holder" 2>/dev/null || true
    as_user verdaxis-backup-reader flock "$backup_dir/.backup.journal.lock" sleep 40 &
    holder=$!
    sleep 1
    refused_run "$cell" verdaxis-backup-verify.service backup "$backup_dir"
    kill "$holder" 2>/dev/null || true
    wait "$holder" 2>/dev/null || true
    # A full journal: 256 admitted healthy entries and a failing observation.
    systemctl mask --runtime verdaxis-monitor-alert@health.service >/dev/null
    as_user verdaxis-health python3 -c '
import json, os, sys
d = sys.argv[1]
for n in range(10**15, 10**15 + 256):
    p = os.path.join(d, f"{n}.json")
    with open(p, "w") as h:
        h.write(json.dumps({"check": "health", "observed_at": "2026-07-20T04:00:00Z",
                            "schema_version": 1, "sequence": n, "state": "healthy"}, sort_keys=True))
' "$health_dir/alert-handoff"
    ready_fail
    refused_run "$cell" verdaxis-health.service health "$health_dir"
    find "$health_dir/alert-handoff" -name '1000000000*.json' -delete
    systemctl unmask --runtime verdaxis-monitor-alert@health.service >/dev/null
    # A permission refusal: the handoff directory is not the producer's.
    chown root "$backup_dir/alert-handoff"
    break_backup
    refused_run "$cell" verdaxis-backup-verify.service backup "$backup_dir"
    chown verdaxis-backup-reader "$backup_dir/alert-handoff"
    # Released, the fresh observations converge.
    ready_ok
    write_backup
    check "$cell" test "$(run_producer verdaxis-health.service health)" = 0
    check "$cell" test "$(run_producer verdaxis-backup-verify.service backup)" = 0
    check "$cell" test -z "$(pending "$health_dir")"
    check "$cell" test -z "$(pending "$backup_dir")"
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

cell_acknowledged_crash() {
    local cell=alert_handoff_acknowledged_before_unlink_resumes before=$failures entry sequence count
    systemctl mask --runtime verdaxis-monitor-alert@health.service >/dev/null
    ready_fail
    run_producer verdaxis-health.service health >/dev/null
    sequence=$(pending "$health_dir" | head -n 1)
    entry="$health_dir/alert-handoff/$sequence"
    check "$cell" test -f "$entry"
    # The state a crash after the durable acknowledgement and before the unlink leaves.
    printf '{"acknowledged":{"digest":"%s","sequence":%s,"state":"failure"},"check":"health","head":null,"schema_version":1}' \
        "$(digest "$entry")" "${sequence%.json}" > "$alert_dir/health.delivery.json"
    chown verdaxis-monitor-alert:verdaxis-monitor-alert "$alert_dir/health.delivery.json"
    chmod 0600 "$alert_dir/health.delivery.json"
    systemctl unmask --runtime verdaxis-monitor-alert@health.service >/dev/null
    count=$(request_count)
    wake health
    check "$cell" test ! -e "$entry"
    check "$cell" test "$(request_count)" = "$count"
    ready_ok
    check "$cell" test "$(run_producer verdaxis-health.service health)" = 0
    check "$cell" test -z "$(pending "$health_dir")"
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

# A blocked head stays visible: the alert units stay failed after later
# wakeups and reminders, the fixed-code handoff status says so to any reader,
# and the run line is logged at error level. Only the operator clears it.
cell_blocked_head_visible() {
    local cell=alert_blocked_head_leaves_alert_units_failed_until_resolved before=$failures
    local sequence status_file=/var/lib/verdaxis-monitor-alert-status/health.json
    destination_status hang
    ready_fail
    systemctl start verdaxis-health.service >/dev/null 2>&1 || true
    wait_settled verdaxis-monitor-alert@health.service
    sequence=$(pending "$health_dir" | head -n 1)
    sequence=${sequence%.json}
    check "$cell" test -n "$sequence"
    check "$cell" test "$(systemctl show -p ExecMainStatus --value verdaxis-monitor-alert@health.service)" = 5
    check "$cell" systemctl is-failed --quiet verdaxis-monitor-alert@health.service
    check "$cell" grep -Fq '"code":"handoff_uncertain"' "$status_file"
    allowed "$cell" nobody cat "$status_file"
    check "$cell" test "$(stat -c %a "$status_file")" = 644
    check "$cell" sh -c "journalctl --no-pager -o cat -p err -u verdaxis-monitor-alert@health.service | grep -Fq '\"command\":\"monitor alert\"'"
    # A later producer run and its wakeup, then the reminder: still failed.
    destination_status 200
    ready_ok
    systemctl start verdaxis-health.service >/dev/null 2>&1 || true
    wait_settled verdaxis-monitor-alert@health.service
    check "$cell" test "$(systemctl show -p ExecMainStatus --value verdaxis-monitor-alert@health.service)" = 5
    systemctl start verdaxis-monitor-alert-reminder.service >/dev/null 2>&1 || true
    check "$cell" test "$(systemctl show -p ExecMainStatus --value verdaxis-monitor-alert-reminder.service)" = 5
    check "$cell" systemctl is-failed --quiet verdaxis-monitor-alert-reminder.service
    check "$cell" grep -Fq '"code":"handoff_uncertain"' "$status_file"
    # The operator reconciles both destinations; the queue drains in order.
    for destination in healthchecks telegram; do
        check "$cell" as_user verdaxis-monitor-alert "$binary_path" monitor alert \
            --reconcile "health:$sequence:$destination:not-delivered"
    done
    drain_all
    check "$cell" test -z "$(pending "$health_dir")"
    check "$cell" grep -Fq '"code":"ok"' "$status_file"
    systemctl reset-failed verdaxis-monitor-alert-reminder.service >/dev/null 2>&1 || true
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

# Sandbox directives systemd could not apply in this rootless target must be
# on the list carried to the Task 31 root-VM rehearsal; any other fails.
cell_sandbox_directives() {
    local cell=alert_handoff_sandbox_directives_applied_or_listed before=$failures directive
    local allowed_list=" PrivateNetwork ProtectHostname IPAddressDeny DevicePolicy "
    while IFS= read -r directive; do
        [ -n "$directive" ] || continue
        case "$allowed_list" in
            *" $directive "*) printf 'IGNORED_DIRECTIVE %s (carried to the Task 31 root-VM check)\n' "$directive" ;;
            *) fail "$cell" "systemd ignored $directive" ;;
        esac
    done < <(journalctl --no-pager -o cat 2>/dev/null \
        | grep -E 'verdaxis-(health|backup-verify|monitor-alert)' \
        | sed -n -E 's/.*: ([A-Za-z]+)=[^ ]* is configured, but.*/\1/p; s/.*configures an IP firewall, but.*/IPAddressDeny/p; s/.*([A-Za-z]+Policy)=.*not supported.*/\1/p' \
        | sort -u)
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

snapshot_state() {
    local path
    for path in "$health_dir" "$health_dir/.health.journal.lock" "$health_dir/alert-handoff" \
        "$backup_dir" "$backup_dir/.backup.journal.lock" "$backup_dir/alert-handoff" \
        "$alert_dir/.health.lock" "$alert_dir/.backup.lock"; do
        printf '%s %s %s\n' "$path" "$(stat -c '%i %U %a' "$path")" "$(acl_of "$path" | tr '\n' ' ')"
    done
    for path in "$health_dir/health.sequence.json" "$backup_dir/backup.sequence.json" \
        "$alert_dir/health.delivery.json" "$alert_dir/backup.delivery.json"; do
        printf '%s %s\n' "$path" "$(digest "$path")"
    done
    pending "$health_dir"
}

prepare_reboot() {
    destination_status 200
    # An admitted failure whose wakeup is lost with the reboot.
    systemctl mask --runtime verdaxis-monitor-alert@health.service >/dev/null
    ready_fail
    run_producer verdaxis-health.service health >/dev/null
    snapshot_state > /var/lib/alert-handoff-before-reboot.txt
}

cell_after_reboot() {
    local cell=alert_handoff_restart_and_reboot before=$failures count texts
    check "$cell" test "$(snapshot_state)" = "$(cat /var/lib/alert-handoff-before-reboot.txt)"
    check "$cell" start_fakes
    destination_status 200
    count=$(request_count)
    # The producer restarts healthy: its state directory mode keeps the
    # dispatcher's traversal entry effective.
    ready_ok
    run_producer verdaxis-health.service health >/dev/null
    expect_acl "$cell" "$health_dir" "user::rwx user:verdaxis-monitor-alert:--x group::--- mask::--x other::--- "
    # The wakeup drained the failure; the reminder drains the healthy entry.
    systemctl start verdaxis-monitor-alert-reminder.service >/dev/null 2>&1 || true
    texts=$(texts_since "$count" | grep -F 'Verdaxis local health' | tr '\n' '|')
    check "$cell" test "$texts" = "Verdaxis local health: FAILURE|Verdaxis local health: RECOVERY|"
    check "$cell" test -z "$(pending "$health_dir")"
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

cell_revert() {
    local cell=alert_handoff_revert_blocked_then_released before=$failures acl_before count output
    local -a alert_env
    mapfile -t alert_env < /etc/verdaxis-monitor/alert.env
    acl_before=$(snapshot_state | grep -v 'json ' | grep -v '^[0-9]')
    destination_status 503
    ready_fail
    run_producer verdaxis-health.service health >/dev/null
    check "$cell" test -n "$(pending "$health_dir")"
    # Blocked revert: the journal is not drained.
    output=0
    as_user verdaxis-monitor-alert "$binary_path" monitor alert --release health >/dev/null 2>&1 || output=$?
    check "$cell" test "$output" = 3
    check "$cell" test "$(stat -c %a "$alert_dir/.health.lock")" = 400
    # The unchanged Python dispatcher cannot run while the native journal is pending.
    count=$(request_count)
    destination_status 200
    if as_user verdaxis-monitor-alert env "${alert_env[@]}" \
        python3 /stage/alert_dispatch.py --event health-healthy --state-directory "$alert_dir" >/dev/null 2>&1; then
        fail "$cell" "fenced Python dispatcher ran"
    fi
    check "$cell" test "$(request_count)" = "$count"
    drain_all
    check "$cell" test -z "$(pending "$health_dir")"
    output=0
    as_user verdaxis-monitor-alert "$binary_path" monitor alert --release health >/dev/null 2>&1 || output=$?
    check "$cell" test "$output" = 0
    check "$cell" test "$(stat -c %a "$alert_dir/.health.lock")" = 600
    ready_ok
    count=$(request_count)
    check "$cell" as_user verdaxis-monitor-alert env "${alert_env[@]}" \
        python3 /stage/alert_dispatch.py --event health-healthy --state-directory "$alert_dir"
    check "$cell" test "$(texts_since "$count")" = "/hc-health
Verdaxis local health: RECOVERY"
    # No permission was broadened on the way.
    check "$cell" test "$(snapshot_state | grep -v 'json ' | grep -v '^[0-9]' | grep -v '\.health\.lock')" \
        = "$(printf '%s\n' "$acl_before" | grep -v '\.health\.lock')"
    if [ "$failures" -eq "$before" ]; then pass "$cell"; fi
}

# On a failed cell: the unit results, the monitors' status codes (no secrets:
# fixed codes only), the dispatcher progress and the recent unit journal.
diagnose() {
    local unit
    echo "---- diagnostics"
    for unit in verdaxis-health.service verdaxis-backup-verify.service \
        verdaxis-monitor-alert@health.service verdaxis-monitor-alert@backup.service; do
        printf '%s %s\n' "$unit" "$(systemctl show -p ActiveState -p Result -p ExecMainStatus "$unit" | tr '\n' ' ')"
    done
    for path in "$health_dir/status.json" "$backup_dir/status.json" \
        "$alert_dir/health.delivery.json" "$alert_dir/backup.delivery.json"; do
        printf '%s: %s\n' "$path" "$(cat "$path" 2>/dev/null)"
    done
    ls -la /home/verdaxis-prod /home/verdaxis-prod/backups /home/verdaxis-prod/backups/attempts 2>&1
    cat /home/verdaxis-prod/backups/attempts/*.json 2>&1
    echo
    runuser -u verdaxis-backup-reader -- ls /home/verdaxis-prod/backups/attempts 2>&1
    printf 'pending health: %s\n' "$(pending "$health_dir" | tr '\n' ' ')"
    printf 'pending backup: %s\n' "$(pending "$backup_dir" | tr '\n' ' ')"
    journalctl --no-pager -o short-monotonic -n 120 -u verdaxis-health.service -u verdaxis-backup-verify.service \
        -u 'verdaxis-monitor-alert@*' -u fake-services.service 2>&1 | tail -n 120
    echo "---- end of diagnostics"
}

inside_target() {
    # Every assertion reports and counts; one failed cell does not stop the rest.
    set +e
    if [ "${VERDAXIS_DISPOSABLE_TARGET:-0}" != 1 ] || { [ ! -e /run/.containerenv ] && [ ! -e /.dockerenv ]; } \
        || [ "$(id -u)" != 0 ]; then
        echo "refusing: this mode runs only as root inside the disposable target" >&2
        exit 2
    fi
    case "$1" in
        --before-reboot)
            setup_target
            cell_enqueue_and_dispatch
            retain_failures
            cell_producer_isolation
            cell_acl_modes
            cell_failed_enqueue
            cell_acknowledged_crash
            cell_blocked_head_visible
            cell_sandbox_directives
            prepare_reboot
            ;;
        --after-reboot)
            cell_after_reboot
            cell_revert
            cell_sandbox_directives
            ;;
        *)
            echo "unknown phase $1" >&2
            exit 2
            ;;
    esac
    if [ "$failures" -ne 0 ]; then
        diagnose
    fi
}

# ---- main ---------------------------------------------------------------------------------------

case "${1:-}" in
    --inside-disposable-target)
        inside_target "${2:-}"
        ;;
    "" | --disposable-identities)
        static_unit_abi
        static_sandbox_kept
        static_policy
        if [ "${1:-}" = --disposable-identities ]; then
            disposable_identities
        fi
        ;;
    *)
        echo "usage: $0 [--disposable-identities]" >&2
        exit 2
        ;;
esac
printf 'ALERT_HANDOFF_FAILURES=%s\n' "$failures"
[ "$failures" -eq 0 ]
