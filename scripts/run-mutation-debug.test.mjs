import { expect, test } from "bun:test";

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
} from "./run-mutation-debug.mjs";

test("plans the zero-based mutation shard command", () => {
  expect(parseInvocation(["shard", "0", "12"])).toEqual({
    mode: "run",
    label: "shard-0",
    reportRoot: "target/mutants-default-shard-0",
    mutationCommand: ["just", "test-mutation-shard", "0", "12"],
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
});

test("rejects an invalid shard boundary", () => {
  expect(() => parseInvocation(["shard", "12", "12"])).toThrow("usage:");
  expect(() => parseInvocation(["shard", "0", "0"])).toThrow("usage:");
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

test("coalesces telemetry and makes an in-flight snapshot abortable", async () => {
  const coordinator = new SnapshotCoordinator(60_000);
  let startFirst;
  const firstStarted = new Promise((resolve) => {
    startFirst = resolve;
  });
  const neverCompletes = new Promise(() => {});
  const first = coordinator.start(async () => {
    startFirst();
    await neverCompletes;
  });

  await firstStarted;
  expect(coordinator.active).toBeTrue();
  expect(coordinator.start(async () => {})).toBeUndefined();

  coordinator.abort();
  await expect(first).resolves.toEqual({ aborted: true });
  expect(coordinator.active).toBeFalse();

  await expect(coordinator.start(async () => {})).resolves.toEqual({
    aborted: false,
  });
});

test("bounds a snapshot that does not cooperate with its abort signal", async () => {
  const coordinator = new SnapshotCoordinator(1);
  const result = await coordinator.start(async () => Bun.sleep(20));

  expect(result).toEqual({ aborted: true });
  expect(coordinator.active).toBeFalse();
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

  expect(messages).toEqual([
    "optional mutation diagnostic file inventory failed: inventory unavailable",
    "optional mutation diagnostic telemetry stopped before completion",
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
