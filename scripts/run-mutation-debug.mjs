#!/usr/bin/env bun
// Contain the expensive mutation runner so an OOM or hung mutant leaves useful
// evidence without monopolising a hosted runner.
import {
  appendFile,
  mkdir,
  readFile,
  readdir,
  stat,
  writeFile,
} from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { notice } from "./lib/ci.mjs";
import { runCommand } from "./lib/process.mjs";

export { runCommand };

export const SNAPSHOT_INTERVAL_MS = 15_000;
export const SNAPSHOT_TIMEOUT_MS = 10_000;
export const DIAGNOSTIC_COMMAND_TIMEOUT_MS = 3_000;
export const DIAGNOSTIC_COMMAND_MAX_BUFFER_BYTES = 256 * 1024;
export const POSTMORTEM_COLLECTION_TIMEOUT_MS = 10_000;
export const SNAPSHOT_PROCESS_LIMIT = 30;
export const SNAPSHOT_TAIL_LINES = 80;
export const SNAPSHOT_LINE_LIMIT = 1_000;
export const MEMORY_MAX_BYTES = 6 * 1024 * 1024 * 1024;
export const PIDS_MAX = 2_048;
/**
 * Per-process heap ceiling for the contained mutation tree.
 *
 * A mutant can turn a terminating loop into an unbounded one, and the test
 * process then grows until the cgroup limit kills the whole shard. Run
 * 33577273491 shard 2 lost every process this way: one test binary reached
 * 5.91 GiB while the largest legitimate process in the same run was a 422 MiB
 * rustc (rust-lld 225 MiB, cargo 63 MiB). Bounding each process well above
 * anything legitimate but well below {@link MEMORY_MAX_BYTES} makes the
 * runaway allocation fail in that one process, so cargo-mutants records a
 * caught mutant instead of the cgroup killing the run.
 */
export const PROCESS_HEAP_MAX_BYTES = 2 * 1024 * 1024 * 1024;
export const INNER_WALL_LIMIT = "25m";
export const INNER_KILL_GRACE = "30s";
export const OUTER_WALL_LIMIT_MS = 26 * 60 * 1_000;
export const OUTER_FINALIZATION_RESERVE_MS = 30 * 1_000;
export const OUTER_TERMINATION_START_MS =
  OUTER_WALL_LIMIT_MS - OUTER_FINALIZATION_RESERVE_MS;
export const CGROUP_COMMAND_TIMEOUT_MS = 5_000;
export const FORCE_CGROUP_KILL_START_MS =
  OUTER_WALL_LIMIT_MS - CGROUP_COMMAND_TIMEOUT_MS;

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const CGROUP_ROOT = "/sys/fs/cgroup";
const MEMORY_EVENTS = "memory.events";
const USAGE = [
  "usage: bun scripts/run-mutation-debug.mjs shard <index> <total>",
  "       bun scripts/run-mutation-debug.mjs diff-shard <index> <total> <diff>",
  "       bun scripts/run-mutation-debug.mjs ffi",
  "       bun scripts/run-mutation-debug.mjs --collect shard <index>",
  "       bun scripts/run-mutation-debug.mjs --collect diff-shard <index>",
  "       bun scripts/run-mutation-debug.mjs --collect ffi",
].join("\n");
const SHARD_TARGETS = {
  shard: {
    labelPrefix: "shard",
    recipe: "test-mutation-shard",
    reportPrefix: "default-shard",
  },
  "diff-shard": {
    labelPrefix: "diff-shard",
    recipe: "test-mutation-diff-shard",
    reportPrefix: "diff-shard",
  },
};

class UsageError extends Error {
  constructor(message = USAGE) {
    super(message);
    this.code = 64;
  }
}

class CommandError extends Error {
  constructor(command, result) {
    const stderr = result.stderr.trim();
    super(
      `\`${command.join(" ")}\` exited with status ${result.code}${
        stderr ? `: ${stderr}` : ""
      }`,
    );
    this.code = result.code || 1;
  }
}

class DiagnosticAbortError extends Error {
  constructor() {
    super("optional mutation diagnostics were aborted");
  }
}

function throwIfAborted(signal) {
  if (signal?.aborted) throw new DiagnosticAbortError();
}

function abortable(promise, signal) {
  if (!signal) return promise;
  throwIfAborted(signal);
  return new Promise((resolve, reject) => {
    const onAbort = () => reject(new DiagnosticAbortError());
    signal.addEventListener("abort", onAbort, { once: true });
    promise.then(
      (value) => {
        signal.removeEventListener("abort", onAbort);
        resolve(value);
      },
      (error) => {
        signal.removeEventListener("abort", onAbort);
        reject(error);
      },
    );
  });
}

/** Keep telemetry to one bounded, abortable collection at a time. */
export class SnapshotCoordinator {
  constructor(timeoutMs = SNAPSHOT_TIMEOUT_MS) {
    this.timeoutMs = timeoutMs;
    this.current = undefined;
  }

  get active() {
    return this.current !== undefined;
  }

  start(task) {
    if (this.current) return undefined;

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    const clearTimer = () => clearTimeout(timer);
    controller.signal.addEventListener("abort", clearTimer, { once: true });
    const work = Promise.resolve().then(() => task(controller.signal));
    // A bounded caller may observe an abort before a diagnostic cooperates, but
    // that work still owns the single-flight slot until it has actually settled.
    const release = () => {
      clearTimer();
      controller.signal.removeEventListener("abort", clearTimer);
      if (this.current?.controller === controller) this.current = undefined;
    };
    work.then(release, release);
    const completion = abortable(work, controller.signal)
      .then(
        () => ({ aborted: false }),
        (error) => {
          if (error instanceof DiagnosticAbortError) return { aborted: true };
          throw error;
        },
      );

    this.current = { controller, completion };
    return completion;
  }

  abort() {
    this.current?.controller.abort();
  }

  async settle() {
    const completion = this.current?.completion;
    if (completion) await completion;
  }
}

/** Parse the public runner invocation without touching the host. */
export function parseInvocation(argv) {
  const args = [...argv];
  const mode = args[0] === "--collect" ? (args.shift(), "collect") : "run";
  const target = args.shift();

  if (target === "ffi" && args.length === 0) {
    return {
      mode,
      label: "ffi",
      reportRoot: "target/mutants-ffi",
      mutationCommand: mode === "run" ? ["just", "test-mutation-ffi"] : [],
    };
  }

  const shardTarget = SHARD_TARGETS[target];
  if (!shardTarget) throw new UsageError();
  const expectedArguments = mode === "run" ? (target === "diff-shard" ? 3 : 2) : 1;
  if (args.length !== expectedArguments) throw new UsageError();

  const [index, total, diff] = args;
  if (!/^\d+$/.test(index)) throw new UsageError();
  if (mode === "collect") {
    return {
      mode,
      label: `${shardTarget.labelPrefix}-${index}`,
      reportRoot: `target/mutants-${shardTarget.reportPrefix}-${index}`,
      mutationCommand: [],
    };
  }
  if (!/^\d+$/.test(total)) throw new UsageError();

  const shardIndex = Number(index);
  const shardTotal = Number(total);
  if (!Number.isSafeInteger(shardIndex) || !Number.isSafeInteger(shardTotal)) {
    throw new UsageError();
  }
  if (shardTotal === 0 || shardIndex >= shardTotal) throw new UsageError();
  if (target === "diff-shard" && (!diff || diff.includes("\0"))) {
    throw new UsageError();
  }

  return {
    mode,
    label: `${shardTarget.labelPrefix}-${index}`,
    reportRoot: `target/mutants-${shardTarget.reportPrefix}-${index}`,
    mutationCommand: [
      "just",
      shardTarget.recipe,
      index,
      total,
      ...(target === "diff-shard" ? [diff] : []),
    ],
  };
}

/** Build the bounded command run by the already-contained child process. */
export function containedCommand({ wallLimit, killGrace, mutationCommand }) {
  return [
    "/usr/bin/timeout",
    "--verbose",
    "--signal=TERM",
    `--kill-after=${killGrace}`,
    wallLimit,
    "prlimit",
    `--data=${PROCESS_HEAP_MAX_BYTES}`,
    "stdbuf",
    "-oL",
    "-eL",
    ...mutationCommand,
  ];
}

/** Write to an inherited stream without buffering excess child output here. */
export function writeWithBackpressure(output, value) {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      output.removeListener("drain", onDrain);
      output.removeListener("error", onError);
    };
    const onDrain = () => {
      cleanup();
      resolve();
    };
    const onError = (error) => {
      cleanup();
      reject(error);
    };
    output.once("drain", onDrain);
    output.once("error", onError);
    try {
      if (output.write(value)) {
        cleanup();
        resolve();
      }
    } catch (error) {
      cleanup();
      reject(error);
    }
  });
}

async function runChecked(command, options) {
  const result = await runCommand(command, options);
  if (result.code !== 0) throw new CommandError(command, result);
  return result;
}

async function runWithInheritedStdio(command) {
  const child = Bun.spawn(command, {
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  });
  return child.exited;
}

async function exists(path, { signal } = {}) {
  try {
    await abortable(stat(path), signal);
    return true;
  } catch (error) {
    if (error instanceof DiagnosticAbortError) throw error;
    return false;
  }
}

async function isDirectory(path, { signal } = {}) {
  try {
    return (await abortable(stat(path), signal)).isDirectory();
  } catch (error) {
    if (error instanceof DiagnosticAbortError) throw error;
    return false;
  }
}

async function readOptional(path, { signal } = {}) {
  try {
    return await abortable(readFile(path, "utf8"), signal);
  } catch (error) {
    if (error instanceof DiagnosticAbortError) throw error;
    return "";
  }
}

async function sudoWrite(path, value, options = {}) {
  const result = await runCommand(["sudo", "-n", "tee", "--", path], {
    ...options,
    stdin: new Blob([`${value}\n`]),
    stdout: "ignore",
  });
  if (result.code !== 0)
    throw new CommandError(["sudo", "-n", "tee", "--", path], result);
}

function eventValue(contents, name) {
  for (const line of contents.split("\n")) {
    const [key, value] = line.trim().split(/\s+/, 2);
    if (key === name) return Number(value) || 0;
  }
  return 0;
}

function timestamp() {
  return new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
}

function truncateLines(text, limit = SNAPSHOT_LINE_LIMIT) {
  return text
    .split("\n")
    .map((line) => line.slice(0, limit))
    .join("\n");
}

async function commandOutput(command, { signal } = {}) {
  try {
    const result = await runCommand(command, {
      signal,
      timeoutMs: DIAGNOSTIC_COMMAND_TIMEOUT_MS,
      maxBuffer: DIAGNOSTIC_COMMAND_MAX_BUFFER_BYTES,
    });
    throwIfAborted(signal);
    const output = `${result.stdout}${result.stderr}`;
    const capped = result.outputLimitExceeded
      ? `diagnostic command output exceeded ${DIAGNOSTIC_COMMAND_MAX_BUFFER_BYTES} bytes; process killed\n`
      : "";
    return result.code === 0
      ? `${output}${capped}`
      : `${output}${capped}diagnostic command exited with status ${result.code}\n`;
  } catch (error) {
    if (error instanceof DiagnosticAbortError) throw error;
    throwIfAborted(signal);
    return `${error instanceof Error ? error.message : String(error)}\n`;
  }
}

async function commandExists(command) {
  return Bun.which(command) !== null;
}

export class MutationRunner {
  constructor(invocation) {
    this.invocation = invocation;
    this.reportDir = join(invocation.reportRoot, "mutants.out");
    this.diagnosticsDir = `target/mutation-diagnostics-${invocation.label}`;
    this.telemetryLog = join(this.diagnosticsDir, "telemetry.log");
    this.mutationLog = join(this.diagnosticsDir, "mutation.log");
    this.timeLog = join(this.diagnosticsDir, "time-v.txt");
    this.classificationLog = join(this.diagnosticsDir, "classification.txt");
    this.eventsBeforeLog = join(
      this.diagnosticsDir,
      "memory-events-before.txt",
    );
    this.eventsAfterLog = join(this.diagnosticsDir, "memory-events-after.txt");
    this.inventoryLog = join(this.diagnosticsDir, "file-inventory.txt");
    this.snapshots = new SnapshotCoordinator();
    this.finalization = undefined;
    this.mutationCgroupDir = "";
    this.telemetryTimer = undefined;
    this.watchdogTimer = undefined;
    this.forceCgroupTimer = undefined;
    this.forceTimer = undefined;
    this.terminationSignal = "";
    this.diagnosticsAborted = false;
    this.postmortemControllers = new Set();
  }

  async initialise() {
    await mkdir(this.diagnosticsDir, { recursive: true });
    if (this.invocation.mode === "collect") return;
    await Promise.all(
      [
        this.telemetryLog,
        this.mutationLog,
        this.timeLog,
        this.classificationLog,
        this.eventsBeforeLog,
        this.eventsAfterLog,
      ].map((path) => writeFile(path, "")),
    );
  }

  async findSelfCgroupDir(signal) {
    const cgroup = await readOptional("/proc/self/cgroup", { signal });
    const unified = cgroup
      .split("\n")
      .map((line) => line.split(":", 3))
      .find(([hierarchy]) => hierarchy === "0");
    const relative = unified?.[2];
    const candidate = relative ? `${CGROUP_ROOT}${relative}` : "";
    return candidate && (await isDirectory(candidate, { signal }))
      ? candidate
      : CGROUP_ROOT;
  }

  async telemetryCgroupDir(signal) {
    return this.mutationCgroupDir &&
      (await isDirectory(this.mutationCgroupDir, { signal }))
      ? this.mutationCgroupDir
      : this.findSelfCgroupDir(signal);
  }

  async tailIfReadable(path, signal) {
    if (!(await exists(path, { signal }))) return "";
    const output = await commandOutput(
      ["tail", "-n", String(SNAPSHOT_TAIL_LINES), path],
      { signal },
    );
    return `--- tail (${SNAPSHOT_TAIL_LINES} lines): ${path} ---\n${truncateLines(output)}`;
  }

  async latestScenarioLog(signal) {
    const logDir = join(this.reportDir, "log");
    try {
      const entries = await abortable(
        readdir(logDir, { withFileTypes: true }),
        signal,
      );
      const candidates = await Promise.all(
        entries
          .filter((entry) => entry.isFile())
          .map(async (entry) => {
            const path = join(logDir, entry.name);
            return {
              path,
              mtimeMs: (await abortable(stat(path), signal)).mtimeMs,
            };
          }),
      );
      return (
        candidates.sort((left, right) => right.mtimeMs - left.mtimeMs)[0]
          ?.path ?? ""
      );
    } catch (error) {
      if (error instanceof DiagnosticAbortError) throw error;
      throwIfAborted(signal);
      return "";
    }
  }

  async snapshotContents(reason, signal) {
    const cgroupDir = await this.telemetryCgroupDir(signal);
    const latestScenarioLog = await this.latestScenarioLog(signal);
    const pieces = [
      `\n===== ${timestamp()} | ${this.invocation.label} | ${reason} =====\n`,
    ];

    pieces.push(
      "--- free ---\n",
      await commandOutput(["free", "-h"], { signal }),
    );
    pieces.push(
      "--- /proc/meminfo ---\n",
      await readOptional("/proc/meminfo", { signal }),
    );
    for (const pressureFile of [
      "/proc/pressure/memory",
      "/proc/pressure/cpu",
      "/proc/pressure/io",
    ]) {
      const contents = await readOptional(pressureFile, { signal });
      if (contents) pieces.push(`--- ${pressureFile} ---\n`, contents);
    }

    pieces.push(`--- mutation cgroup: ${cgroupDir} ---\n`);
    for (const metric of [
      "memory.current",
      "memory.peak",
      "memory.max",
      "memory.high",
      "memory.stat",
      MEMORY_EVENTS,
      "memory.events.local",
      "memory.swap.current",
      "memory.swap.peak",
      "memory.swap.max",
      "memory.oom.group",
      "memory.pressure",
      "cpu.pressure",
      "cpu.stat",
      "io.pressure",
      "pids.current",
      "pids.peak",
      "pids.max",
      "cgroup.procs",
    ]) {
      const contents = await readOptional(join(cgroupDir, metric), { signal });
      if (contents) pieces.push(`${metric}:\n`, contents);
    }

    const processes = truncateLines(
      await commandOutput(
        ["ps", "-eo", "pid,ppid,stat,etimes,rss,vsz,comm,args", "--sort=-rss"],
        { signal },
      ),
      500,
    )
      .split("\n")
      .slice(0, SNAPSHOT_PROCESS_LIMIT)
      .join("\n");
    pieces.push("--- top processes by RSS (KiB) ---\n", processes, "\n");
    pieces.push(
      "--- disk ---\n",
      await commandOutput(
        ["df", "-h", process.env.GITHUB_WORKSPACE || ".", "/tmp"],
        { signal },
      ),
    );
    pieces.push(
      "--- process limits ---\n",
      await readOptional("/proc/self/limits", { signal }),
    );
    pieces.push(
      "--- kernel warnings ---\n",
      truncateLines(
        await commandOutput(
          ["dmesg", "--ctime", "--level=emerg,alert,crit,err,warn"],
          { signal },
        ),
      ),
      truncateLines(
        await commandOutput(
          ["journalctl", "-k", "-n", String(SNAPSHOT_TAIL_LINES), "--no-pager"],
          { signal },
        ),
      ),
    );
    pieces.push(
      "--- core dumps ---\n",
      await commandOutput(["cat", "/proc/sys/kernel/core_pattern"], { signal }),
    );
    if (await commandExists("coredumpctl")) {
      const coredumps = await commandOutput(
        ["coredumpctl", "--no-pager", "--no-legend", "list"],
        { signal },
      );
      pieces.push(
        truncateLines(coredumps.split("\n").slice(-30).join("\n")),
        "\n",
      );
    }

    pieces.push("--- cargo-mutants state ---\n");
    for (const name of [
      "lock.json",
      "outcomes.json",
      "timeout.txt",
      "missed.txt",
      "debug.log",
    ]) {
      pieces.push(
        await this.tailIfReadable(join(this.reportDir, name), signal),
      );
    }
    if (latestScenarioLog)
      pieces.push(await this.tailIfReadable(latestScenarioLog, signal));
    return pieces.join("");
  }

  async snapshot(reason) {
    if (this.diagnosticsAborted) return false;
    const completion = this.snapshots.start(async (signal) => {
      try {
        const contents = await this.snapshotContents(reason, signal);
        await abortable(appendFile(this.telemetryLog, contents), signal);
      } catch (error) {
        if (error instanceof DiagnosticAbortError) throw error;
        const message = error instanceof Error ? error.message : String(error);
        await abortable(
          appendFile(this.telemetryLog, `snapshot failed: ${message}\n`),
          signal,
        ).catch((appendError) => {
          if (appendError instanceof DiagnosticAbortError) throw appendError;
        });
      }
    });
    if (!completion) return false;
    try {
      const result = await completion;
      if (result.aborted)
        notice(
          `optional mutation diagnostic snapshot ${reason} stopped before completion`,
        );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      notice(`optional mutation diagnostic snapshot ${reason} failed: ${message}`);
    }
    return true;
  }

  async collectFileInventory(signal) {
    const files = [];
    const visit = async (path, depth) => {
      if (depth > 4) return;
      let entries;
      try {
        entries = await abortable(
          readdir(path, { withFileTypes: true }),
          signal,
        );
      } catch (error) {
        if (error instanceof DiagnosticAbortError) throw error;
        throwIfAborted(signal);
        return;
      }
      for (const entry of entries) {
        throwIfAborted(signal);
        const child = join(path, entry.name);
        if (entry.isFile()) {
          try {
            files.push({
              path: child,
              size: (await abortable(stat(child), signal)).size,
            });
          } catch (error) {
            if (error instanceof DiagnosticAbortError) throw error;
            // A cargo-mutants cleanup race only makes this optional inventory less complete.
          }
        } else if (entry.isDirectory()) {
          await visit(child, depth + 1);
        }
      }
    };
    await Promise.all([
      visit(this.invocation.reportRoot, 0),
      visit(this.diagnosticsDir, 0),
    ]);
    const contents = files
      .sort(
        (left, right) =>
          right.size - left.size || left.path.localeCompare(right.path),
      )
      .slice(0, 200)
      .map(({ path, size }) => `${size} ${path}`)
      .join("\n");
    await abortable(
      writeFile(this.inventoryLog, contents ? `${contents}\n` : ""),
      signal,
    );
  }

  async setupCgroup() {
    if (!(await commandExists("sudo"))) {
      throw new Error("sudo is required for mutation cgroup containment");
    }
    const sudo = await runCommand(["sudo", "-n", "true"]);
    if (sudo.code !== 0) {
      throw new Error("passwordless sudo is required for mutation containment");
    }
    if (!(await exists(join(CGROUP_ROOT, "cgroup.controllers")))) {
      throw new Error("cgroup v2 unified hierarchy is required");
    }

    const parentDir = dirname(await this.findSelfCgroupDir());
    if (!parentDir.startsWith(CGROUP_ROOT) || !(await isDirectory(parentDir))) {
      throw new Error(`invalid cgroup parent: ${parentDir}`);
    }
    for (const controller of ["memory", "pids"]) {
      const available = (
        await readOptional(join(parentDir, "cgroup.controllers"))
      ).split(/\s+/);
      if (!available.includes(controller)) {
        throw new Error(
          `required controller ${controller} is unavailable in ${parentDir}`,
        );
      }
    }
    await sudoWrite(join(parentDir, "cgroup.subtree_control"), "+memory +pids");
    for (const controller of ["memory", "pids"]) {
      const enabled = (
        await readOptional(join(parentDir, "cgroup.subtree_control"))
      ).split(/\s+/);
      if (!enabled.includes(controller)) {
        throw new Error(
          `controller ${controller} was not enabled in ${parentDir}`,
        );
      }
    }

    const runComponent =
      `${process.env.GITHUB_RUN_ID || "local"}-${process.env.GITHUB_RUN_ATTEMPT || "0"}-${process.env.GITHUB_JOB || "job"}`.replace(
        /[^a-zA-Z0-9_.-]/g,
        "_",
      );
    this.mutationCgroupDir = join(
      parentDir,
      `pure-analyzer-mutation-${runComponent}-${this.invocation.label}-${process.pid}`,
    );
    await runChecked(["sudo", "-n", "mkdir", "--", this.mutationCgroupDir]);
    await sudoWrite(
      join(this.mutationCgroupDir, "memory.max"),
      String(MEMORY_MAX_BYTES),
    );
    await sudoWrite(join(this.mutationCgroupDir, "memory.swap.max"), "0");
    await sudoWrite(join(this.mutationCgroupDir, "memory.oom.group"), "1");
    await sudoWrite(join(this.mutationCgroupDir, "pids.max"), String(PIDS_MAX));

    const expected = new Map([
      ["memory.max", String(MEMORY_MAX_BYTES)],
      ["memory.swap.max", "0"],
      ["memory.oom.group", "1"],
      ["pids.max", String(PIDS_MAX)],
    ]);
    for (const [name, value] of expected) {
      const actual = (
        await readOptional(join(this.mutationCgroupDir, name))
      ).trim();
      if (actual !== value)
        throw new Error(
          `${name} was ${actual || "unreadable"}, expected ${value}`,
        );
    }
    await writeFile(
      this.eventsBeforeLog,
      await readOptional(join(this.mutationCgroupDir, MEMORY_EVENTS)),
    );
  }

  async prepareSccache() {
    if (!(await commandExists("sccache"))) {
      throw new Error("sccache is required by the mutation CI environment");
    }
    const preflight = await runCommand(["sccache", "--stop-server"]);
    await writeFile(
      this.diagnosticsDir + "/sccache-preflight.log",
      `${preflight.stdout}${preflight.stderr}`,
    );
    const processes = await runCommand(["ps", "-C", "sccache", "-o", "stat="]);
    if (
      processes.stdout
        .split("\n")
        .some((line) => line.trim() && !line.trim().startsWith("Z"))
    ) {
      throw new Error("sccache server remained outside the mutation cgroup");
    }
  }

  startWatchdog(onTimeout) {
    this.watchdogTimer = setTimeout(() => {
      console.error(
        `mutation watchdog is reserving ${OUTER_FINALIZATION_RESERVE_MS / 1_000}s for cleanup before the ${OUTER_WALL_LIMIT_MS / 1_000}s hard limit`,
      );
      void onTimeout();
    }, OUTER_TERMINATION_START_MS);
    this.forceCgroupTimer = setTimeout(() => {
      this.abortDiagnostics();
      void this.killMutationCgroup({ timeoutMs: CGROUP_COMMAND_TIMEOUT_MS });
    }, FORCE_CGROUP_KILL_START_MS);
    this.forceTimer = setTimeout(() => {
      this.abortDiagnostics();
      process.kill(process.pid, "SIGKILL");
    }, OUTER_WALL_LIMIT_MS);
  }

  stopSoftWatchdog() {
    if (this.watchdogTimer) clearTimeout(this.watchdogTimer);
    this.watchdogTimer = undefined;
  }

  stopWatchdog() {
    this.stopSoftWatchdog();
    if (this.forceCgroupTimer) clearTimeout(this.forceCgroupTimer);
    this.forceCgroupTimer = undefined;
    if (this.forceTimer) clearTimeout(this.forceTimer);
    this.forceTimer = undefined;
  }

  async startTelemetry() {
    await this.snapshot("containment established");
    this.telemetryTimer = setInterval(
      () => void this.snapshot("periodic sample"),
      SNAPSHOT_INTERVAL_MS,
    );
  }

  stopTelemetry() {
    if (this.telemetryTimer) clearInterval(this.telemetryTimer);
    this.telemetryTimer = undefined;
    this.snapshots.abort();
  }

  abortDiagnostics() {
    this.diagnosticsAborted = true;
    this.snapshots.abort();
    for (const controller of this.postmortemControllers) controller.abort();
  }

  async killMutationCgroup(options = {}) {
    if (!this.mutationCgroupDir) return;
    await sudoWrite(
      join(this.mutationCgroupDir, "cgroup.kill"),
      "1",
      options,
    ).catch(() => {});
  }

  async cgroupHasProcesses() {
    if (!this.mutationCgroupDir) return false;
    return Boolean(
      (await readOptional(join(this.mutationCgroupDir, "cgroup.procs"))).trim(),
    );
  }

  async cleanupCgroup() {
    try {
      if (
        !this.mutationCgroupDir ||
        !(await isDirectory(this.mutationCgroupDir))
      ) {
        return true;
      }
      if (await exists(join(this.mutationCgroupDir, "cgroup.kill"))) {
        await sudoWrite(join(this.mutationCgroupDir, "cgroup.kill"), "1", {
          timeoutMs: CGROUP_COMMAND_TIMEOUT_MS,
        });
      } else {
        for (const pid of (
          await readOptional(join(this.mutationCgroupDir, "cgroup.procs"))
        ).split("\n")) {
          if (/^\d+$/.test(pid)) {
            await runCommand(["sudo", "-n", "kill", "-KILL", "--", pid], {
              timeoutMs: CGROUP_COMMAND_TIMEOUT_MS,
            });
          }
        }
      }
      for (
        let attempt = 0;
        attempt < 50 && (await this.cgroupHasProcesses());
        attempt += 1
      ) {
        await Bun.sleep(100);
      }
      if (await this.cgroupHasProcesses()) {
        console.error("mutation cgroup still has processes after cgroup.kill");
        console.error(
          await readOptional(join(this.mutationCgroupDir, "cgroup.procs")),
        );
        return false;
      }
      await runChecked(["sudo", "-n", "rmdir", "--", this.mutationCgroupDir], {
        timeoutMs: CGROUP_COMMAND_TIMEOUT_MS,
      });
      this.mutationCgroupDir = "";
      return true;
    } catch (error) {
      console.error(
        `mutation cgroup cleanup failed: ${error instanceof Error ? error.message : String(error)}`,
      );
      return false;
    }
  }

  async timeoutObserved() {
    return (await readOptional(this.mutationLog)).includes(
      "timeout: sending signal TERM",
    );
  }

  async classifyResult(status) {
    const eventsPath = this.mutationCgroupDir
      ? join(this.mutationCgroupDir, MEMORY_EVENTS)
      : "";
    const after = eventsPath ? await readOptional(eventsPath) : "";
    await writeFile(this.eventsAfterLog, after);
    const before = await readOptional(this.eventsBeforeLog);
    const oomDelta = eventValue(after, "oom") - eventValue(before, "oom");
    const oomKillDelta =
      eventValue(after, "oom_kill") - eventValue(before, "oom_kill");
    let classification;
    let reportedStatus = status;
    if (oomDelta > 0 || oomKillDelta > 0) {
      classification = "oom";
      if (reportedStatus === 0) reportedStatus = 137;
    } else if (await this.timeoutObserved()) {
      classification = "wall-time-limit";
      if (reportedStatus === 0) reportedStatus = 124;
    } else if (this.terminationSignal) {
      classification = `signal-${this.terminationSignal}`;
      if (reportedStatus === 0) reportedStatus = 143;
    } else if (status === 0) {
      classification = "success";
    } else {
      classification = "command-failure";
    }
    const contents = [
      `classification=${classification}`,
      `command_status=${status}`,
      `reported_status=${reportedStatus}`,
      `termination_signal=${this.terminationSignal}`,
      `memory_oom_delta=${oomDelta}`,
      `memory_oom_kill_delta=${oomKillDelta}`,
      `memory_max=${MEMORY_MAX_BYTES}`,
      "memory_swap_max=0",
      `inner_wall_limit=${INNER_WALL_LIMIT}`,
      `inner_kill_grace=${INNER_KILL_GRACE}`,
      "",
    ].join("\n");
    await writeFile(this.classificationLog, contents);
    await writeWithBackpressure(process.stdout, contents);
    return reportedStatus;
  }

  async teeChildOutput(child) {
    let teeStatus = 0;
    const pump = async (stream, output) => {
      if (!stream) return;
      const reader = stream.getReader();
      while (true) {
        const { done, value } = await reader.read();
        if (done) return;
        try {
          await writeWithBackpressure(output, value);
        } catch (error) {
          teeStatus = 1;
          console.error(
            `mutation output write failed: ${error instanceof Error ? error.message : String(error)}`,
          );
        }
        try {
          await appendFile(this.mutationLog, value);
        } catch (error) {
          teeStatus = 1;
          console.error(
            `mutation log write failed: ${error instanceof Error ? error.message : String(error)}`,
          );
        }
      }
    };
    const [commandStatus] = await Promise.all([
      child.exited,
      pump(child.stdout, process.stdout),
      pump(child.stderr, process.stderr),
    ]);
    return { commandStatus, teeStatus };
  }

  async runMutation() {
    const cgroupProcs = join(this.mutationCgroupDir, "cgroup.procs");
    const command = [
      "/usr/bin/time",
      "-v",
      "-o",
      this.timeLog,
      process.execPath,
      SCRIPT_PATH,
      "--run-child",
      cgroupProcs,
      INNER_WALL_LIMIT,
      INNER_KILL_GRACE,
      "--",
      ...this.invocation.mutationCommand,
    ];
    const child = Bun.spawn(command, {
      stdin: "inherit",
      stdout: "pipe",
      stderr: "pipe",
    });
    const { commandStatus, teeStatus } = await this.teeChildOutput(child);
    const summary = `command_status=${commandStatus} tee_status=${teeStatus}\n`;
    await appendFile(this.mutationLog, summary);
    await writeWithBackpressure(process.stdout, summary);
    return commandStatus !== 0 ? commandStatus : teeStatus;
  }

  async runPostmortem(label, task) {
    if (this.diagnosticsAborted) return false;
    const controller = new AbortController();
    const timer = setTimeout(
      () => controller.abort(),
      POSTMORTEM_COLLECTION_TIMEOUT_MS,
    );
    this.postmortemControllers.add(controller);
    try {
      await abortable(
        Promise.resolve().then(() => task(controller.signal)),
        controller.signal,
      );
      return true;
    } catch (error) {
      if (error instanceof DiagnosticAbortError) {
        notice(`optional mutation diagnostic ${label} stopped before completion`);
      } else {
        const message = error instanceof Error ? error.message : String(error);
        notice(`optional mutation diagnostic ${label} failed: ${message}`);
      }
      return false;
    } finally {
      clearTimeout(timer);
      this.postmortemControllers.delete(controller);
    }
  }

  async collectPostmortem(status) {
    await this.snapshots.settle().catch(() => {});
    await Promise.all([
      this.snapshot(`post-cleanup exit status ${status}`),
      this.runPostmortem("file inventory", (signal) =>
        this.collectFileInventory(signal),
      ),
    ]);
  }

  async finalize(status) {
    if (this.finalization) return this.finalization;
    this.finalization = (async () => {
      this.stopSoftWatchdog();
      this.stopTelemetry();
      let finalStatus = status;
      let cleanupSucceeded = false;
      try {
        try {
          finalStatus = await this.classifyResult(status);
        } catch (error) {
          const message =
            error instanceof Error ? error.message : String(error);
          console.error(`mutation result classification failed: ${message}`);
          if (finalStatus === 0) finalStatus = 70;
        }
      } finally {
        cleanupSucceeded = await this.cleanupCgroup().catch((error) => {
          const message =
            error instanceof Error ? error.message : String(error);
          console.error(`mutation cgroup cleanup failed: ${message}`);
          return false;
        });
      }
      if (!cleanupSucceeded && finalStatus === 0) finalStatus = 70;
      try {
        await this.collectPostmortem(finalStatus);
      } finally {
        this.stopWatchdog();
      }
      return finalStatus;
    })();
    return this.finalization;
  }
}

function childInvocation(argv) {
  const separator = argv.indexOf("--");
  if (argv[0] !== "--run-child" || separator !== 4 || argv.length < 6)
    throw new UsageError();
  return {
    cgroupProcs: argv[1],
    wallLimit: argv[2],
    killGrace: argv[3],
    mutationCommand: argv.slice(5),
  };
}

async function runContainedChild(argv) {
  const invocation = childInvocation(argv);
  await sudoWrite(invocation.cgroupProcs, String(process.pid));
  const attached = (await readOptional(invocation.cgroupProcs)).split("\n");
  if (!attached.includes(String(process.pid))) {
    throw new Error(
      `mutation child ${process.pid} was not attached to ${invocation.cgroupProcs}`,
    );
  }
  await runChecked(["sccache", "--start-server"], {
    stdout: "inherit",
    stderr: "inherit",
  });
  const status = await runWithInheritedStdio(containedCommand(invocation));
  await runWithInheritedStdio(["sccache", "--show-stats"]).catch(() => {});
  await runWithInheritedStdio(["sccache", "--stop-server"]).catch(() => {});
  return status;
}

async function main(argv) {
  const invocation = parseInvocation(argv);
  const runner = new MutationRunner(invocation);
  await runner.initialise();
  if (invocation.mode === "collect") {
    await Promise.all([
      runner.snapshot("always step"),
      runner.runPostmortem("file inventory", (signal) =>
        runner.collectFileInventory(signal),
      ),
    ]);
    return 0;
  }

  const terminate = async (signal, status) => {
    runner.terminationSignal = signal;
    const finalStatus = await runner.finalize(status);
    process.exit(finalStatus);
  };
  process.once("SIGTERM", () => void terminate("TERM", 143));
  process.once("SIGINT", () => void terminate("INT", 130));
  process.once("SIGHUP", () => void terminate("HUP", 129));

  runner.startWatchdog(() => terminate("TERM", 143));
  let status = 1;
  try {
    await runner.setupCgroup();
    await runner.prepareSccache();
    await runner.startTelemetry();
    status = await runner.runMutation();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    status = error instanceof CommandError ? error.code : 1;
  }
  return runner.finalize(status);
}

async function entrypoint() {
  try {
    const argv = process.argv.slice(2);
    const status =
      argv[0] === "--run-child"
        ? await runContainedChild(argv)
        : await main(argv);
    process.exitCode = status;
  } catch (error) {
    if (error instanceof UsageError) {
      console.error(error.message);
      process.exitCode = error.code;
      return;
    }
    console.error(
      error instanceof Error ? error.stack || error.message : String(error),
    );
    process.exitCode = error instanceof CommandError ? error.code : 1;
  }
}

if (import.meta.main) await entrypoint();
