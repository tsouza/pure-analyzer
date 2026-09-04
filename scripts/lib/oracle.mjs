// Shared bounded-oracle JSON schema for the analyzer's frozen Legend corpora
// (issue #245). Originally private to analysis-comparison-corpus.mjs;
// factored out so analysis-canonical-corpus.mjs can validate canonical-
// emission fixtures against the same four bounded, independently executable
// oracle shapes without duplicating the schema or lambda-rendering logic —
// see crates/pure-analyzer-analysis/tests/support/legend_oracle.rs for the
// Rust-side counterpart these two must stay in lockstep with.

export function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function nonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

export function assertExactFields(value, fields, path) {
  const actual = Object.keys(value).sort();
  const expected = [...fields].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${path}: unexpected corpus fields`);
  }
}

function assertIntegerValues(value, path) {
  if (!Array.isArray(value) || value.length === 0 || value.some((item) => !Number.isSafeInteger(item))) {
    throw new Error(`${path}: values must be a non-empty integer list`);
  }
  return value;
}

function assertStringValues(value, path) {
  if (!Array.isArray(value) || value.length === 0 || value.some((item) => !nonEmptyString(item))) {
    throw new Error(`${path}: values must be a non-empty string list`);
  }
  return value;
}

function pureString(value) {
  return `'${value.replaceAll("'", "''")}'`;
}

/** Render the bounded engine oracle that is mechanically tied to one M3 source shape. */
export function oracleLambda(value, path = "oracle") {
  if (!isObject(value)) throw new Error(`${path}: oracle must be a JSON object`);
  switch (value.kind) {
    case "scan": {
      assertExactFields(value, ["kind", "values"], path);
      return `|[${assertIntegerValues(value.values, `${path}:values`).join(", ")}]`;
    }
    case "filter_true": {
      assertExactFields(value, ["kind", "values"], path);
      return `|[${assertIntegerValues(value.values, `${path}:values`).join(", ")}]->filter(x: Integer[1]|true)`;
    }
    case "ordered_columns": {
      assertExactFields(value, ["kind", "columns"], path);
      const columns = assertStringValues(value.columns, `${path}:columns`);
      if (new Set(columns).size !== columns.length) {
        throw new Error(`${path}: ordered column names must be unique`);
      }
      return `|[${columns.map(pureString).join(", ")}]`;
    }
    case "literal_filter": {
      assertExactFields(value, ["kind", "values", "value"], path);
      const values = assertStringValues(value.values, `${path}:values`);
      if (!nonEmptyString(value.value) || !values.includes(value.value)) {
        throw new Error(`${path}: literal filter value must be one of its input values`);
      }
      return `|[${values.map(pureString).join(", ")}]->filter(x: String[1]|$x == ${pureString(value.value)})`;
    }
    default:
      throw new Error(`${path}: unsupported bounded oracle ${JSON.stringify(value.kind)}`);
  }
}
