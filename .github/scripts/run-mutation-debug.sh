#!/usr/bin/env bash
set -Eeuo pipefail

readonly SNAPSHOT_INTERVAL_SECONDS=15
readonly SNAPSHOT_PROCESS_LIMIT=30
readonly SNAPSHOT_TAIL_LINES=80
readonly MEMORY_MAX_BYTES=6442450944
readonly PIDS_MAX=2048
readonly INNER_WALL_LIMIT=25m
readonly INNER_KILL_GRACE=30s

usage() {
  printf 'usage: %s shard <index> <total>\n' "$0" >&2
  printf '       %s ffi\n' "$0" >&2
  printf '       %s --collect shard <index>\n' "$0" >&2
  printf '       %s --collect ffi\n' "$0" >&2
  exit 64
}

is_nonnegative_integer() {
  [[ "$1" =~ ^[0-9]+$ ]]
}

mode=run
if [[ "${1:-}" == "--collect" ]]; then
  mode=collect
  shift
fi

case "${1:-}" in
  shard)
    if [[ "$mode" == "run" ]]; then
      [[ $# -eq 3 ]] || usage
      is_nonnegative_integer "$2" || usage
      is_nonnegative_integer "$3" || usage
      ((10#$3 > 0 && 10#$2 < 10#$3)) || usage
      readonly LABEL="shard-$2"
      readonly REPORT_ROOT="target/mutants-default-shard-$2"
      readonly -a MUTATION_COMMAND=(just test-mutation-shard "$2" "$3")
    else
      [[ $# -eq 2 ]] || usage
      is_nonnegative_integer "$2" || usage
      readonly LABEL="shard-$2"
      readonly REPORT_ROOT="target/mutants-default-shard-$2"
      readonly -a MUTATION_COMMAND=()
    fi
    ;;
  ffi)
    [[ $# -eq 1 ]] || usage
    readonly LABEL=ffi
    readonly REPORT_ROOT=target/mutants-ffi
    if [[ "$mode" == "run" ]]; then
      readonly -a MUTATION_COMMAND=(just test-mutation-ffi)
    else
      readonly -a MUTATION_COMMAND=()
    fi
    ;;
  *) usage ;;
esac

readonly MODE="$mode"
readonly REPORT_DIR="${REPORT_ROOT}/mutants.out"
readonly DIAGNOSTICS_DIR="target/mutation-diagnostics-${LABEL}"
readonly TELEMETRY_LOG="${DIAGNOSTICS_DIR}/telemetry.log"
readonly MUTATION_LOG="${DIAGNOSTICS_DIR}/mutation.log"
readonly TIME_LOG="${DIAGNOSTICS_DIR}/time-v.txt"
readonly CLASSIFICATION_LOG="${DIAGNOSTICS_DIR}/classification.txt"
readonly EVENTS_BEFORE_LOG="${DIAGNOSTICS_DIR}/memory-events-before.txt"
readonly EVENTS_AFTER_LOG="${DIAGNOSTICS_DIR}/memory-events-after.txt"

mkdir -p "$DIAGNOSTICS_DIR"

mutation_cgroup_dir=""
telemetry_pid=""
termination_signal=""

find_self_cgroup_dir() {
  local relative
  relative="$(awk -F: '$1 == "0" { print $3; exit }' /proc/self/cgroup 2>/dev/null)"
  if [[ -n "$relative" && -d "/sys/fs/cgroup${relative}" ]]; then
    printf '/sys/fs/cgroup%s\n' "$relative"
  else
    printf '/sys/fs/cgroup\n'
  fi
}

telemetry_cgroup_dir() {
  if [[ -n "$mutation_cgroup_dir" && -d "$mutation_cgroup_dir" ]]; then
    printf '%s\n' "$mutation_cgroup_dir"
  else
    find_self_cgroup_dir
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
  cgroup_dir="$(telemetry_cgroup_dir)"
  latest_scenario_log="$({
    find "${REPORT_DIR}/log" -maxdepth 1 -type f -printf '%T@ %p\n' \
      2>/dev/null || true
  } | sort -nr | head -n 1 | cut -d' ' -f2- || true)"

  (
    set +e
    flock 9
    {
      printf '\n===== %s | %s | %s =====\n' \
        "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$LABEL" "$reason"
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
      printf '%s\n' "--- mutation cgroup: ${cgroup_dir} ---"
      for metric in \
        memory.current memory.peak memory.max memory.high memory.stat \
        memory.events memory.events.local memory.swap.current \
        memory.swap.peak memory.swap.max memory.oom.group memory.pressure \
        cpu.pressure cpu.stat io.pressure pids.current pids.peak pids.max \
        cgroup.procs; do
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
: >"$CLASSIFICATION_LOG"
: >"$EVENTS_BEFORE_LOG"
: >"$EVENTS_AFTER_LOG"

setup_cgroup() {
  local self_dir
  local parent_dir
  local run_component
  local cgroup_name
  local controller

  command -v sudo >/dev/null 2>&1 || {
    printf '%s\n' 'sudo is required for mutation cgroup containment' >&2
    return 1
  }
  sudo -n true || {
    printf '%s\n' 'passwordless sudo is required for mutation containment' >&2
    return 1
  }
  [[ -r /sys/fs/cgroup/cgroup.controllers ]] || {
    printf '%s\n' 'cgroup v2 unified hierarchy is required' >&2
    return 1
  }

  self_dir="$(find_self_cgroup_dir)"
  parent_dir="$(dirname -- "$self_dir")"
  [[ "$parent_dir" == /sys/fs/cgroup* && -d "$parent_dir" ]] || {
    printf 'invalid cgroup parent: %s\n' "$parent_dir" >&2
    return 1
  }

  for controller in memory pids; do
    grep -qw "$controller" "${parent_dir}/cgroup.controllers" || {
      printf 'required controller %s is unavailable in %s\n' \
        "$controller" "$parent_dir" >&2
      return 1
    }
  done
  printf '+memory +pids\n' \
    | sudo -n tee "${parent_dir}/cgroup.subtree_control" >/dev/null || {
      printf 'could not enable memory/pids controllers in %s\n' "$parent_dir" >&2
      return 1
    }
  for controller in memory pids; do
    grep -qw "$controller" "${parent_dir}/cgroup.subtree_control" || {
      printf 'controller %s was not enabled in %s\n' \
        "$controller" "$parent_dir" >&2
      return 1
    }
  done

  run_component="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}-${GITHUB_JOB:-job}"
  run_component="${run_component//[^a-zA-Z0-9_.-]/_}"
  cgroup_name="pure-analyzer-mutation-${run_component}-${LABEL}-$$"
  mutation_cgroup_dir="${parent_dir}/${cgroup_name}"
  sudo -n mkdir -- "$mutation_cgroup_dir"

  printf '%s\n' "$MEMORY_MAX_BYTES" \
    | sudo -n tee "${mutation_cgroup_dir}/memory.max" >/dev/null
  printf '0\n' \
    | sudo -n tee "${mutation_cgroup_dir}/memory.swap.max" >/dev/null
  printf '1\n' \
    | sudo -n tee "${mutation_cgroup_dir}/memory.oom.group" >/dev/null
  printf '%s\n' "$PIDS_MAX" \
    | sudo -n tee "${mutation_cgroup_dir}/pids.max" >/dev/null

  [[ "$(<"${mutation_cgroup_dir}/memory.max")" == "$MEMORY_MAX_BYTES" ]]
  [[ "$(<"${mutation_cgroup_dir}/memory.swap.max")" == "0" ]]
  [[ "$(<"${mutation_cgroup_dir}/memory.oom.group")" == "1" ]]
  [[ "$(<"${mutation_cgroup_dir}/pids.max")" == "$PIDS_MAX" ]]
  cat "${mutation_cgroup_dir}/memory.events" >"$EVENTS_BEFORE_LOG"
}

prepare_sccache() {
  command -v sccache >/dev/null 2>&1 || {
    printf '%s\n' 'sccache is required by the mutation CI environment' >&2
    return 1
  }
  sccache --stop-server >"${DIAGNOSTICS_DIR}/sccache-preflight.log" 2>&1 || true
  # Process-name lookup is intentional: the server can outlive its original PID.
  # shellcheck disable=SC2009
  if ps -C sccache -o stat= 2>/dev/null | grep -qv '^Z'; then
    printf '%s\n' 'sccache server remained outside the mutation cgroup' >&2
    return 1
  fi
}

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

# Reached through the EXIT trap cleanup path.
# shellcheck disable=SC2329
cgroup_has_processes() {
  [[ -n "$mutation_cgroup_dir" && -r "${mutation_cgroup_dir}/cgroup.procs" ]] \
    && read -r _ <"${mutation_cgroup_dir}/cgroup.procs"
}

# Reached through the EXIT trap cleanup path.
# shellcheck disable=SC2329
cleanup_cgroup() {
  [[ -n "$mutation_cgroup_dir" && -d "$mutation_cgroup_dir" ]] || return 0

  if [[ -e "${mutation_cgroup_dir}/cgroup.kill" ]]; then
    printf '1\n' | sudo -n tee "${mutation_cgroup_dir}/cgroup.kill" >/dev/null
  else
    while read -r pid; do
      sudo -n kill -KILL -- "$pid" 2>/dev/null || true
    done <"${mutation_cgroup_dir}/cgroup.procs"
  fi
  for _ in {1..50}; do
    cgroup_has_processes || break
    sleep 0.1
  done
  if cgroup_has_processes; then
    printf '%s\n' 'mutation cgroup still has processes after cgroup.kill' >&2
    cat "${mutation_cgroup_dir}/cgroup.procs" >&2
    return 1
  fi
  sudo -n rmdir -- "$mutation_cgroup_dir"
  mutation_cgroup_dir=""
}

# Reached through the EXIT trap classification path.
# shellcheck disable=SC2329
event_value() {
  local file="$1"
  local name="$2"
  if [[ ! -r "$file" ]]; then
    printf '0\n'
    return 0
  fi
  awk -v name="$name" '$1 == name { print $2; found = 1; exit }
    END { if (!found) print 0 }' "$file" 2>/dev/null || printf '0\n'
}

classified_status=0
# Reached through the EXIT trap classification path.
# shellcheck disable=SC2329
classify_result() {
  local status="$1"
  local before_oom
  local before_oom_kill
  local after_oom
  local after_oom_kill
  local oom_delta
  local oom_kill_delta
  local classification

  classified_status="$status"
  if [[ -n "$mutation_cgroup_dir" && -r "${mutation_cgroup_dir}/memory.events" ]]; then
    cat "${mutation_cgroup_dir}/memory.events" >"$EVENTS_AFTER_LOG"
  else
    : >"$EVENTS_AFTER_LOG"
  fi
  before_oom="$(event_value "$EVENTS_BEFORE_LOG" oom)"
  before_oom_kill="$(event_value "$EVENTS_BEFORE_LOG" oom_kill)"
  after_oom="$(event_value "$EVENTS_AFTER_LOG" oom)"
  after_oom_kill="$(event_value "$EVENTS_AFTER_LOG" oom_kill)"
  oom_delta=$((after_oom - before_oom))
  oom_kill_delta=$((after_oom_kill - before_oom_kill))

  if ((oom_delta > 0 || oom_kill_delta > 0)); then
    classification=oom
    ((classified_status != 0)) || classified_status=137
  elif grep -Fq 'timeout: sending signal TERM' "$MUTATION_LOG"; then
    classification=wall-time-limit
    ((classified_status != 0)) || classified_status=124
  elif [[ -n "$termination_signal" ]]; then
    classification="signal-${termination_signal}"
    ((classified_status != 0)) || classified_status=143
  elif ((status == 0)); then
    classification=success
  else
    classification=command-failure
  fi

  {
    printf 'classification=%s\n' "$classification"
    printf 'command_status=%s\n' "$status"
    printf 'reported_status=%s\n' "$classified_status"
    printf 'termination_signal=%s\n' "$termination_signal"
    printf 'memory_oom_delta=%s\n' "$oom_delta"
    printf 'memory_oom_kill_delta=%s\n' "$oom_kill_delta"
    printf 'memory_max=%s\n' "$MEMORY_MAX_BYTES"
    printf 'memory_swap_max=0\n'
    printf 'inner_wall_limit=%s\n' "$INNER_WALL_LIMIT"
    printf 'inner_kill_grace=%s\n' "$INNER_KILL_GRACE"
  } | tee "$CLASSIFICATION_LOG"
}

# Invoked by the TERM, INT, and HUP traps.
# shellcheck disable=SC2329
on_signal() {
  local signal="$1"
  local status="$2"
  termination_signal="$signal"
  stop_telemetry
  snapshot "received signal ${signal}" || true
  exit "$status"
}

# Invoked by the EXIT trap.
# shellcheck disable=SC2329
on_exit() {
  local status=$?
  trap - EXIT TERM INT HUP
  stop_telemetry
  snapshot "exit status ${status}" || true
  classify_result "$status"
  status="$classified_status"
  collect_file_inventory
  if ! cleanup_cgroup; then
    ((status != 0)) || status=70
  fi
  exit "$status"
}

trap 'on_signal TERM 143' TERM
trap 'on_signal INT 130' INT
trap 'on_signal HUP 129' HUP
trap on_exit EXIT

setup_cgroup
prepare_sccache
snapshot 'containment established'

(
  while true; do
    snapshot 'periodic sample'
    sleep "$SNAPSHOT_INTERVAL_SECONDS"
  done
) &
telemetry_pid=$!

set +e
# Variables in this child program intentionally expand only after entering Bash.
# shellcheck disable=SC2016
/usr/bin/time -v -o "$TIME_LOG" bash -c '
  set -Eeuo pipefail
  cgroup_procs="$1"
  wall_limit="$2"
  kill_grace="$3"
  shift 3
  child_pid="$BASHPID"
  printf "%s\n" "$child_pid" | sudo -n tee "$cgroup_procs" >/dev/null
  grep -Fqx "$child_pid" "$cgroup_procs"
  sccache --start-server
  set +e
  /usr/bin/timeout --verbose --signal=TERM --kill-after="$kill_grace" "$wall_limit" \
    stdbuf -oL -eL "$@"
  status=$?
  set -e
  sccache --show-stats || true
  sccache --stop-server || true
  exit "$status"
' mutation-cgroup-child "${mutation_cgroup_dir}/cgroup.procs" \
  "$INNER_WALL_LIMIT" "$INNER_KILL_GRACE" \
  "${MUTATION_COMMAND[@]}" 2>&1 | tee "$MUTATION_LOG"
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
