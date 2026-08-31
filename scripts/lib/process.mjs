// Bounded child-process execution shared by Bun automation. A process's two
// output streams share one cap so a noisy diagnostic cannot exhaust CI memory.

class OutputCapture {
  constructor(maxBytes, terminate) {
    this.maxBytes = maxBytes;
    this.terminate = terminate;
    this.bytes = 0;
    this.exceeded = false;
  }

  take(chunk) {
    const available = Math.max(this.maxBytes - this.bytes, 0);
    const captured = chunk.slice(0, available);
    this.bytes += captured.byteLength;
    if (captured.byteLength !== chunk.byteLength && !this.exceeded) {
      this.exceeded = true;
      this.terminate();
    }
    return captured;
  }
}

function decodeChunks(chunks, byteLength) {
  const output = new Uint8Array(byteLength);
  let offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder().decode(output);
}

async function captureStream(stream, capture, onChunk) {
  if (!stream) return "";
  const reader = stream.getReader();
  const chunks = onChunk ? undefined : [];
  let byteLength = 0;
  try {
    while (!capture.exceeded) {
      const { done, value } = await reader.read();
      if (done) break;
      const chunk = capture.take(value);
      if (chunk.byteLength > 0) {
        if (onChunk) await onChunk(chunk);
        else {
          chunks.push(chunk);
          byteLength += chunk.byteLength;
        }
      }
    }
  } catch (error) {
    if (!capture.exceeded) throw error;
  } finally {
    if (capture.exceeded) await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
  return chunks ? decodeChunks(chunks, byteLength) : "";
}

/** Run a command with bounded, shared captured output and optional stdout streaming. */
export async function runCommand(command, options = {}) {
  const child = Bun.spawn(command, {
    cwd: options.cwd,
    env: options.env,
    stdin: options.stdin ?? "ignore",
    stdout: options.stdout ?? "pipe",
    stderr: options.stderr ?? "pipe",
    signal: options.signal,
    timeout: options.timeoutMs,
    killSignal: options.killSignal ?? "SIGKILL",
  });
  const capture = new OutputCapture(
    options.maxBuffer ?? Number.POSITIVE_INFINITY,
    () => {
      try {
        child.kill("SIGKILL");
      } catch {
        // The command may have exited while the stream was being drained.
      }
    },
  );
  const stdout =
    options.stdout === "pipe" || options.stdout === undefined
      ? captureStream(child.stdout, capture, options.onStdout)
      : "";
  const stderr =
    options.stderr === "pipe" || options.stderr === undefined
      ? captureStream(child.stderr, capture)
      : "";
  let code;
  let out;
  let err;
  try {
    [code, out, err] = await Promise.all([child.exited, stdout, stderr]);
  } catch (error) {
    try {
      child.kill("SIGKILL");
    } catch {
      // The command may have exited while the stream callback failed.
    }
    await Promise.allSettled([child.exited, stdout, stderr]);
    throw error;
  }
  return {
    code,
    stdout: out,
    stderr: err,
    outputLimitExceeded: capture.exceeded,
  };
}
