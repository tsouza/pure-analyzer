// Shared exact-pin Legend execution client for analyzer-owned frozen corpora.

export const INFO_ENDPOINT = "/api/server/v1/info";
export const HTTP_OK = 200;
// A healthy engine can still need its first model-compilation/JIT pass.
export const REQUEST_TIMEOUT_MS = 30_000;

/** A connection or timeout prevented an engine request from completing. */
export class EngineUnavailableError extends Error {}

/** The live engine is not the exact immutable corpus pin. */
export class PinnedEngineError extends Error {}

function nonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

/** Canonical JSON used only for exact frozen-result comparisons. */
export function canonicalJson(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(",")}}`;
}

/** Return whether two JSON values are structurally equal, independent of object key order. */
export function jsonEqual(left, right) {
  return canonicalJson(left) === canonicalJson(right);
}

/** Read the configured engine base URL, rejecting non-HTTP endpoints. */
export function legendEngineBaseUrl() {
  const raw = process.env.LEGEND_ENGINE_URL ?? "http://localhost:6300";
  let url;
  try {
    url = new URL(raw);
  } catch (error) {
    throw new Error(`invalid LEGEND_ENGINE_URL ${JSON.stringify(raw)}: ${error.message}`);
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("LEGEND_ENGINE_URL must use http or https");
  }
  return raw.replace(/\/+$/, "");
}

function endpointUrl(base, endpoint) {
  return `${base}${endpoint}`;
}

/** Request one JSON Legend endpoint with a finite timeout and typed outage errors. */
export async function requestJson(base, endpoint, method, body, contentType, label) {
  let response;
  try {
    response = await fetch(endpointUrl(base, endpoint), {
      method,
      headers: { "content-type": contentType },
      body,
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
  } catch (error) {
    throw new EngineUnavailableError(`${label}: request failed: ${error.message}`);
  }
  const text = await response.text();
  if (response.status !== HTTP_OK) {
    throw new Error(`${label}: expected HTTP ${HTTP_OK}, got ${response.status}: ${text}`);
  }
  try {
    return JSON.parse(text);
  } catch (error) {
    throw new Error(`${label}: response was not JSON: ${error.message}`);
  }
}

/** Reject a missing or version-different engine before asking it to execute anything. */
export function assertEngineVersion(version, expectedVersion) {
  if (!nonEmptyString(version)) {
    throw new Error("Legend engine info did not contain git.build.version");
  }
  if (version !== expectedVersion) {
    throw new PinnedEngineError(
      `Legend engine version ${version} does not match pinned ${expectedVersion}. ` +
        "The frozen corpus is immutable; create a new versioned corpus for a deliberate re-pin.",
    );
  }
  return version;
}

/** Verify the live engine's immutable version pin. */
export async function checkedEngineVersion(base, expectedVersion) {
  const info = await requestJson(
    base,
    INFO_ENDPOINT,
    "GET",
    undefined,
    "application/json",
    "engine info",
  );
  assertEngineVersion(info?.info?.legendSDLC?.["git.build.version"], expectedVersion);
}

/** Execute one fixture side after compiling its model and lambda with the pinned engine. */
export async function executeEvidence(base, metadata, fixture, side) {
  const model = await requestJson(
    base,
    metadata.model_endpoint,
    "POST",
    fixture.model,
    "text/plain",
    `${fixture.id}: model`,
  );
  const lambda = await requestJson(
    base,
    metadata.lambda_endpoint,
    "POST",
    fixture[side].lambda,
    "text/plain",
    `${fixture.id}: ${side} lambda`,
  );
  return requestJson(
    base,
    metadata.execution_endpoint,
    "POST",
    JSON.stringify({
      model,
      function: lambda,
      mapping: null,
      runtime: null,
      context: { _type: "BaseExecutionContext" },
      parameterValues: [],
    }),
    "application/json",
    `${fixture.id}: ${side} execution`,
  );
}
