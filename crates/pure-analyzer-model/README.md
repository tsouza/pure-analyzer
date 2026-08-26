# pure-analyzer-model

`pure-analyzer-model` normalizes Legend
`PureModelContextData` (PMCD) JSON into the deterministic `ModelGraph` consumed
by analyzer resolution and lint passes. It performs no network or Legend-engine
calls; PMCD production is an offline concern outside this crate.

## Loading PMCD

Use `load_pmcd_files` when the model is on disk, or `PmcdDocument` plus
`load_pmcd_documents` when a host already owns the JSON text:

```rust
use pure_analyzer_model::{PmcdDocument, load_pmcd_documents};

let json = r#"{
  "_type": "data",
  "elements": [{
    "_type": "class", "package": "example", "name": "Trade",
    "superTypes": [], "stereotypes": [],
    "properties": [], "qualifiedProperties": []
  }]
}"#;

let graph = load_pmcd_documents(&[PmcdDocument::new("model.pmcd.json", json)])?;
let trade = graph.class("example::Trade").expect("fixture class");
assert_eq!(trade.path().as_str(), "example::Trade");
# Ok::<(), pure_analyzer_model::ModelError>(())
```

Sources merge in argument order. A later class or association with the same
qualified path replaces the earlier element and emits one `PUR9000`
`Diagnostic`. Classes, properties, qualified properties, and path-to-ID indexes
use lexical `BTreeMap` order; association output is path-sorted. Equivalent
PMCD element ordering therefore produces the same graph.

## Normalized facts

The graph retains:

- class paths, ordered supertypes, own temporal stereotypes, simple properties,
  qualified properties, source provenance, and coverage policy;
- property raw types plus generic arguments and validated multiplicities;
- user-qualified signatures when PMCD provides `parameters`;
- generated milestoning classifications (`MilestonedPoint`, `AllVersions`,
  `AllVersionsInRange`, and `EdgePoint`);
- associations with both directed ends. Each property is navigable from the
  class targeted by the opposite end, and is materialized on that owning class;
- source metadata and structured loader diagnostics.

The loader accepts both protocol objects
(`rawType: {fullPath: ...}`, `superTypes: [{path: ...}]`,
`returnGenericType`) and compact string spellings. Protocol serializers may omit
empty class collections, and may
spell built-in profiles in short (`temporal`, `milestoning`) or fully-qualified
form; both forms normalize identically. It ignores unrelated packageable
element kinds, but rejects a malformed class or association rather than
returning incomplete resolver facts.

## Product boundary

This is an analyzer crate. It does not depend on or reuse PureCARD runtime code
or fixtures. The normalized API includes `Provenance::PureFile` and
`ModelSource::PureModelFile` because Pure Domain source and PMCD share the same
normalized graph. This crate's loading entry points accept PMCD only.
