#!/usr/bin/env bash
set -Eeuo pipefail

readonly SNAPSHOT_INTERVAL_SECONDS=15
readonly SNAPSHOT_PROCESS_LIMIT=30
readonly SNAPSHOT_TAIL_LINES=80

usage() {
  printf 'usage: %s <shard-index> <shard-total>\n' "$0" >&2
  printf '       %s --collect <shard-index>\n' "$0" >&2
  exit 64
}

is_nonnegative_integer() {
  [[ "$1" =~ ^[0-9]+$ ]]
}

if [[ "${1:-}" == "--collect" ]]; then
  [[ $# -eq 2 ]] || usage
  readonly MODE=collect
  readonly SHARD_INDEX="$2"
  readonly SHARD_TOTAL=""
else
  [[ $# -eq 2 ]] || usage
  readonly MODE=run
  readonly SHARD_INDEX="$1"
  readonly SHARD_TOTAL="$2"
fi

is_nonnegative_integer "$SHARD_INDEX" || usage
if [[ "$MODE" == "run" ]]; then
  is_nonnegative_integer "$SHARD_TOTAL" || usage
  ((SHARD_TOTAL > 0 && SHARD_INDEX < SHARD_TOTAL)) || usage
fi

readonly REPORT_ROOT="target/mutants-default-shard-${SHARD_INDEX}"
readonly REPORT_DIR="${REPORT_ROOT}/mutants.out"
readonly DIAGNOSTICS_DIR="target/mutation-diagnostics-shard-${SHARD_INDEX}"
readonly TELEMETRY_LOG="${DIAGNOSTICS_DIR}/telemetry.log"
readonly MUTATION_LOG="${DIAGNOSTICS_DIR}/mutation.log"
readonly TIME_LOG="${DIAGNOSTICS_DIR}/time-v.txt"

mkdir -p "$DIAGNOSTICS_DIR"

find_cgroup_dir() {
  local relative
  relative="$(awk -F: '$1 == "0" { print $3; exit }' /proc/self/cgroup 2>/dev/null)"
  if [[ -n "$relative" && -d "/sys/fs/cgroup${relative}" ]]; then
    printf '/sys/fs/cgroup%s\n' "$relative"
  else
    printf '/sys/fs/cgroup\n'
  fi
}

tail_if_readable() {
  local path="$1"
  if [[ -r "$path" ]]; then
    printf '%s\n' "--- tail (${SNAPSHOT_TAIL_LINES} lines): ${path} ---"
    tail -n "$SNAPSHOT_TAIL_LINES" "$path" | cut -c 1-1000
  fi
}

snapshot() {
  local reason="$1"
  local cgroup_dir
  local latest_scenario_log
  cgroup_dir="$(find_cgroup_dir)"
  latest_scenario_log="$({
    find "${REPORT_DIR}/log" -maxdepth 1 -type f -printf '%T@ %p\n' \
      2>/dev/null || true
  } | sort -nr | head -n 1 | cut -d' ' -f2- || true)"

  (
    set +e
    flock 9
    {
      printf '\n===== %s | shard %s | %s =====\n' \
        "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$SHARD_INDEX" "$reason"
      printf '%s\n' '--- free ---'
      free -h
      printf '%s\n' '--- /proc/meminfo ---'
      cat /proc/meminfo
      for pressure_file in /proc/pressure/memory /proc/pressure/cpu /proc/pressure/io; do
        if [[ -r "$pressure_file" ]]; then
          printf '%s\n' "--- ${pressure_file} ---"
          cat "$pressure_file"
        fi
      done
      printf '%s\n' "--- cgroup: ${cgroup_dir} ---"
      for metric in \
        memory.current memory.peak memory.max memory.high memory.stat \
        memory.events memory.events.local memory.swap.current \
        memory.swap.peak memory.swap.max memory.pressure cpu.pressure \
        cpu.stat io.pressure pids.current pids.peak pids.max; do
        if [[ -r "${cgroup_dir}/${metric}" ]]; then
          printf '%s\n' "${metric}:"
          cat "${cgroup_dir}/${metric}"
        fi
      done
      printf '%s\n' '--- top processes by RSS (KiB) ---'
      ps -eo pid,ppid,stat,etimes,rss,vsz,comm,args --sort=-rss \
        | head -n "$SNAPSHOT_PROCESS_LIMIT" \
        | cut -c 1-500
      printf '%s\n' '--- disk ---'
      df -h "${GITHUB_WORKSPACE:-.}" /tmp
      printf '%s\n' '--- ulimit ---'
      ulimit -a
      printf '%s\n' '--- kernel warnings ---'
      dmesg --ctime --level=emerg,alert,crit,err,warn 2>&1 \
        | tail -n "$SNAPSHOT_TAIL_LINES"
      journalctl -k -n "$SNAPSHOT_TAIL_LINES" --no-pager 2>&1
      printf '%s\n' '--- core dumps ---'
      cat /proc/sys/kernel/core_pattern 2>&1
      if command -v coredumpctl >/dev/null 2>&1; then
        coredumpctl --no-pager --no-legend list 2>&1 | tail -n 30
      fi
      printf '%s\n' '--- cargo-mutants state ---'
      tail_if_readable "${REPORT_DIR}/lock.json"
      tail_if_readable "${REPORT_DIR}/outcomes.json"
      tail_if_readable "${REPORT_DIR}/timeout.txt"
      tail_if_readable "${REPORT_DIR}/missed.txt"
      tail_if_readable "${REPORT_DIR}/debug.log"
      if [[ -n "$latest_scenario_log" ]]; then
        tail_if_readable "$latest_scenario_log"
      fi
    } 2>&1 | tee -a "$TELEMETRY_LOG"
  ) 9>"${DIAGNOSTICS_DIR}/snapshot.lock"
}

collect_file_inventory() {
  find "$REPORT_ROOT" "$DIAGNOSTICS_DIR" -maxdepth 4 -type f \
    -printf '%s %p\n' 2>/dev/null \
    | sort -nr \
    | head -n 200 \
    >"${DIAGNOSTICS_DIR}/file-inventory.txt" || true
}

if [[ "$MODE" == "collect" ]]; then
  snapshot 'always collection step'
  collect_file_inventory
  exit 0
fi

: >"$TELEMETRY_LOG"
: >"$MUTATION_LOG"
: >"$TIME_LOG"

telemetry_pid=""
# Invoked from signal and exit traps.
# shellcheck disable=SC2329
stop_telemetry() {
  if [[ -n "$telemetry_pid" ]]; then
    pkill -TERM -P "$telemetry_pid" 2>/dev/null || true
    kill "$telemetry_pid" 2>/dev/null || true
    wait "$telemetry_pid" 2>/dev/null || true
    telemetry_pid=""
  fi
}

# Invoked by the TERM, INT, and HUP traps.
# shellcheck disable=SC2329
on_signal() {
  local signal="$1"
  local status="$2"
  stop_telemetry
  snapshot "received signal ${signal}"
  exit "$status"
}

# Invoked by the EXIT trap.
# shellcheck disable=SC2329
on_exit() {
  local status=$?
  trap - EXIT TERM INT HUP
  stop_telemetry
  snapshot "exit status ${status}"
  collect_file_inventory
  exit "$status"
}

trap 'on_signal TERM 143' TERM
trap 'on_signal INT 130' INT
trap 'on_signal HUP 129' HUP
trap on_exit EXIT

(
  while true; do
    snapshot 'periodic sample'
    sleep "$SNAPSHOT_INTERVAL_SECONDS"
  done
) &
telemetry_pid=$!

set +e
/usr/bin/time -v -o "$TIME_LOG" \
  stdbuf -oL -eL just test-mutation-shard "$SHARD_INDEX" "$SHARD_TOTAL" \
  2>&1 | tee "$MUTATION_LOG"
pipeline_status=("${PIPESTATUS[@]}")
set -e

command_status="${pipeline_status[0]}"
tee_status="${pipeline_status[1]}"
printf 'command_status=%s tee_status=%s\n' "$command_status" "$tee_status" \
  | tee -a "$MUTATION_LOG"
if ((command_status != 0)); then
  exit "$command_status"
fi
exit "$tee_status"
