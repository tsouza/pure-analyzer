import { expect, test } from "bun:test";
import { EventEmitter } from "node:events";

import {
  CGROUP_COMMAND_TIMEOUT_MS,
  INNER_KILL_GRACE,
  INNER_WALL_LIMIT,
  MEMORY_MAX_BYTES,
  OUTER_FINALIZATION_RESERVE_MS,
  OUTER_TERMINATION_START_MS,
  OUTER_WALL_LIMIT_MS,
  PIDS_MAX,
  MutationRunner,
  SnapshotCoordinator,
  containedCommand,
  parseInvocation,
  runCommand,
  writeWithBackpressure,
} from "./run-mutation-debug.mjs";

test("plans the zero-based mutation shard command", () => {
  expect(parseInvocation(["shard", "0", "12"])).toEqual({
    mode: "run",
    label: "shard-0",
    reportRoot: "target/mutants-default-shard-0",
    mutationCommand: ["just", "test-mutation-shard", "0", "12"],
  });
});

test("plans the verified diff-scoped mutation shard command", () => {
  expect(
    parseInvocation(["diff-shard", "2", "3", "target/mutation-scope.diff"]),
  ).toEqual({
    mode: "run",
    label: "diff-shard-2",
    reportRoot: "target/mutants-diff-shard-2",
    mutationCommand: [
      "just",
      "test-mutation-diff-shard",
      "2",
      "3",
      "target/mutation-scope.diff",
    ],
  });
});

test("plans the diagnostics-only forms without a mutation command", () => {
  expect(parseInvocation(["--collect", "shard", "9"])).toEqual({
    mode: "collect",
    label: "shard-9",
    reportRoot: "target/mutants-default-shard-9",
    mutationCommand: [],
  });
  expect(parseInvocation(["--collect", "ffi"])).toEqual({
    mode: "collect",
    label: "ffi",
    reportRoot: "target/mutants-ffi",
    mutationCommand: [],
  });
  expect(parseInvocation(["--collect", "diff-shard", "2"])).toEqual({
    mode: "collect",
    label: "diff-shard-2",
    reportRoot: "target/mutants-diff-shard-2",
    mutationCommand: [],
  });
});

test("rejects an invalid shard boundary", () => {
  expect(() => parseInvocation(["shard", "12", "12"])).toThrow("usage:");
  expect(() => parseInvocation(["shard", "0", "0"])).toThrow("usage:");
  expect(() => parseInvocation(["diff-shard", "0", "1", ""])).toThrow("usage:");
  expect(() => parseInvocation(["--collect", "shard", "not-a-number"])).toThrow(
    "usage:",
  );
});

test("the contained command retains the hard timeout contract", () => {
  expect(
    containedCommand({
      wallLimit: INNER_WALL_LIMIT,
      killGrace: INNER_KILL_GRACE,
      mutationCommand: ["just", "test-mutation-ffi"],
    }),
  ).toEqual([
    "/usr/bin/timeout",
    "--verbose",
    "--signal=TERM",
    "--kill-after=30s",
    "25m",
    "stdbuf",
    "-oL",
    "-eL",
    "just",
    "test-mutation-ffi",
  ]);
  expect(MEMORY_MAX_BYTES).toBe(6 * 1024 * 1024 * 1024);
  expect(PIDS_MAX).toBe(2_048);
});

test("reserves cleanup time inside the immutable outer wall limit", () => {
  expect(OUTER_TERMINATION_START_MS + OUTER_FINALIZATION_RESERVE_MS).toBe(
    OUTER_WALL_LIMIT_MS,
  );
  expect(CGROUP_COMMAND_TIMEOUT_MS).toBeLessThanOrEqual(
    OUTER_FINALIZATION_RESERVE_MS,
  );
});

test("keeps a timed-out snapshot single-flight until ignored work settles", async () => {
  const coordinator = new SnapshotCoordinator(1);
  let releaseIgnoredWork;
  const ignoredWork = new Promise((resolve) => {
    releaseIgnoredWork = resolve;
  });
  const first = coordinator.start(() => ignoredWork);

  await expect(first).resolves.toEqual({ aborted: true });
  expect(coordinator.active).toBeTrue();
  expect(coordinator.start(async () => {})).toBeUndefined();

  releaseIgnoredWork();
  await Bun.sleep(0);
  expect(coordinator.active).toBeFalse();
  await expect(coordinator.start(async () => {})).resolves.toEqual({
    aborted: false,
  });
});

test("caps multi-megabyte diagnostic output before it can accumulate", async () => {
  const maxBuffer = 64 * 1024;
  const result = await runCommand(
    [
      process.execPath,
      "-e",
      `process.stdout.write("x".repeat(${3 * 1024 * 1024}));`,
    ],
    { maxBuffer },
  );

  expect(result.outputLimitExceeded).toBeTrue();
  expect(new TextEncoder().encode(result.stdout).byteLength).toBeLessThanOrEqual(
    maxBuffer,
  );
});

test("shares one output cap across streamed stdout and captured stderr", async () => {
  const maxBuffer = 64 * 1024;
  let streamedBytes = 0;
  const result = await runCommand(
    [
      process.execPath,
      "-e",
      `process.stdout.write("x".repeat(${48 * 1024})); process.stderr.write("y".repeat(${48 * 1024}));`,
    ],
    {
      maxBuffer,
      onStdout: (chunk) => {
        streamedBytes += chunk.byteLength;
      },
    },
  );

  const capturedBytes = new TextEncoder().encode(result.stderr).byteLength;
  expect(result.outputLimitExceeded).toBeTrue();
  expect(streamedBytes + capturedBytes).toBeLessThanOrEqual(maxBuffer);
});

test("waits for a backpressured output stream to drain", async () => {
  const output = new EventEmitter();
  const writes = [];
  output.write = (value) => {
    writes.push(value);
    return false;
  };

  const completion = writeWithBackpressure(output, "classification=success\n");
  let resolved = false;
  void completion.then(() => {
    resolved = true;
  });
  await Promise.resolve();
  expect(resolved).toBeFalse();
  expect(writes).toEqual(["classification=success\n"]);

  output.emit("drain");
  await expect(completion).resolves.toBeUndefined();
});

test("reports optional postmortem aborts and failures without failing the runner", async () => {
  const runner = new MutationRunner(parseInvocation(["ffi"]));
  const messages = [];
  const originalError = console.error;
  console.error = (message) => messages.push(message);

  try {
    await expect(
      runner.runPostmortem("file inventory", async () => {
        throw new Error("inventory unavailable");
      }),
    ).resolves.toBeFalse();

    const aborted = runner.runPostmortem(
      "telemetry",
      async () => new Promise(() => {}),
    );
    runner.abortDiagnostics();
    await expect(aborted).resolves.toBeFalse();
  } finally {
    console.error = originalError;
  }

  const noticePrefix = process.env.GITHUB_ACTIONS ? "::notice::" : "";
  expect(messages).toEqual([
    `${noticePrefix}optional mutation diagnostic file inventory failed: inventory unavailable`,
    `${noticePrefix}optional mutation diagnostic telemetry stopped before completion`,
  ]);
});

test("cleans the cgroup before awaiting optional postmortem work", async () => {
  const runner = new MutationRunner(parseInvocation(["ffi"]));
  const events = [];
  runner.stopSoftWatchdog = () => events.push("stop-soft-watchdog");
  runner.stopTelemetry = () => events.push("stop-telemetry");
  runner.classifyResult = async () => {
    events.push("classify");
    return 0;
  };
  runner.cleanupCgroup = async () => {
    events.push("cleanup");
    return true;
  };
  runner.collectPostmortem = async () => {
    events.push("postmortem");
  };
  runner.stopWatchdog = () => events.push("stop-watchdog");

  await expect(runner.finalize(0)).resolves.toBe(0);
  expect(events).toEqual([
    "stop-soft-watchdog",
    "stop-telemetry",
    "classify",
    "cleanup",
    "postmortem",
    "stop-watchdog",
  ]);
});

test("cleans the cgroup when classification fails", async () => {
  const runner = new MutationRunner(parseInvocation(["ffi"]));
  const events = [];
  runner.stopSoftWatchdog = () => {};
  runner.stopTelemetry = () => {};
  runner.classifyResult = async () => {
    events.push("classify");
    throw new Error("classification failed");
  };
  runner.cleanupCgroup = async () => {
    events.push("cleanup");
    return true;
  };
  runner.collectPostmortem = async () => events.push("postmortem");
  runner.stopWatchdog = () => {};
  const originalError = console.error;
  console.error = () => {};

  try {
    await expect(runner.finalize(0)).resolves.toBe(70);
  } finally {
    console.error = originalError;
  }
  expect(events).toEqual(["classify", "cleanup", "postmortem"]);
});
