//! Cross-loader parity for temporal-stereotype casing (issue #270).
//!
//! PMCD JSON and Pure source are two independent ingestion paths into the
//! same [`pure_analyzer_model::ModelGraph`] semantics, and Legend Pure
//! stereotypes are case-sensitive. Each case below states one expectation
//! and checks the *same logical stereotype application* — expressed once as
//! PMCD JSON and once as the equivalent Pure source atom — against both
//! loader entry points, so a regression that makes them disagree fails here
//! regardless of which loader drifts.

#![allow(clippy::disallowed_methods)]

use pure_analyzer_model::{
    ModelError, ModelErrorKind, PmcdDocument, PureDocument, Temporal, load_pmcd_documents,
    load_pure_documents,
};
use serde_json::json;

/// What both loaders must agree on for a given `profile`/`value` spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    /// Both loaders resolve the stereotype to this temporal variant.
    Accepted(Temporal),
    /// Neither loader recognizes the profile spelling at all: no temporal
    /// fact, and (for the Pure loader) no coverage gap either, exactly as
    /// if the stereotype had never been written.
    IgnoredProfile,
    /// The profile is recognized but the value is not one of the three
    /// canonical spellings: PMCD hard-fails with `UnknownTemporalStereotype`,
    /// and the Pure loader must not silently accept a temporal value it
    /// invented from the malformed casing — it leaves the class open-world
    /// (a coverage gap) instead.
    RejectedValue,
}

fn pmcd_outcome(profile: &str, value: &str) -> Result<Option<Temporal>, ModelErrorKind> {
    let document = json!({
        "_type": "data",
        "elements": [{
            "_type": "class",
            "package": "demo",
            "name": "Target",
            "superTypes": [],
            "stereotypes": [{"profile": profile, "value": value}],
            "properties": [],
            "qualifiedProperties": []
        }]
    })
    .to_string();
    match load_pmcd_documents(&[PmcdDocument::new("pmcd", &document)]) {
        Ok(graph) => Ok(graph.class("demo::Target").expect("pmcd class").temporal()),
        Err(ModelError::InvalidElement { kind, .. }) => Err(*kind),
        Err(other) => panic!("expected an element error, got {other:?}"),
    }
}

/// `(temporal, coverage_gap)` for the class carrying `<<{atom}>>`.
fn pure_outcome(atom: &str) -> (Option<Temporal>, bool) {
    let source = format!(
        r#"
Class <<{atom}>> demo::Target
{{
  value: String[1];
}}
"#
    );
    let graph =
        load_pure_documents(&[PureDocument::new("memory:model.pure", &source)]).expect("load");
    let class = graph.class("demo::Target").expect("pure class");
    (class.temporal(), class.coverage_gap())
}

fn assert_parity(profile: &str, value: &str, atom: &str, expectation: Expectation) {
    let pmcd = pmcd_outcome(profile, value);
    let (pure_temporal, pure_coverage_gap) = pure_outcome(atom);

    match expectation {
        Expectation::Accepted(temporal) => {
            assert_eq!(
                pmcd,
                Ok(Some(temporal)),
                "PMCD `{profile}.{value}` must resolve to {temporal:?}"
            );
            assert_eq!(
                pure_temporal,
                Some(temporal),
                "Pure `{atom}` must resolve to {temporal:?}"
            );
            assert!(
                !pure_coverage_gap,
                "Pure `{atom}` resolved a canonical stereotype; it must not be an open-world gap"
            );
        }
        Expectation::IgnoredProfile => {
            assert_eq!(
                pmcd,
                Ok(None),
                "PMCD `{profile}.{value}` has an unrecognized profile; it must be silently ignored, not errored"
            );
            assert_eq!(
                pure_temporal, None,
                "Pure `{atom}` has an unrecognized profile; it must not resolve a temporal fact"
            );
            assert!(
                !pure_coverage_gap,
                "Pure `{atom}` has an unrecognized profile, exactly like PMCD; it must not open a coverage gap"
            );
        }
        Expectation::RejectedValue => {
            assert!(
                matches!(pmcd, Err(ModelErrorKind::UnknownTemporalStereotype { .. })),
                "PMCD `{profile}.{value}` must reject the non-canonical value, got {pmcd:?}"
            );
            assert_eq!(
                pure_temporal, None,
                "Pure `{atom}` must not silently accept a temporal stereotype derived from non-canonical casing"
            );
            assert!(
                pure_coverage_gap,
                "Pure `{atom}` must leave the class open-world instead of silently accepting it"
            );
        }
    }
}

#[test]
fn canonical_protocol_form_is_accepted_by_both_loaders() {
    assert_parity(
        "temporal",
        "businesstemporal",
        "temporal.businesstemporal",
        Expectation::Accepted(Temporal::BusinessTemporal),
    );
}

#[test]
fn canonical_qualified_form_is_accepted_by_both_loaders() {
    assert_parity(
        "meta::pure::profiles::temporal",
        "processingtemporal",
        "meta::pure::profiles::temporal.processingtemporal",
        Expectation::Accepted(Temporal::ProcessingTemporal),
    );
}

#[test]
fn uppercase_value_under_a_canonical_profile_is_rejected_by_both_loaders() {
    // Issue #270: `pure.rs::annotation_facts` used to lowercase the atom
    // before matching, so `temporal.BUSINESSTEMPORAL` silently resolved to
    // `Temporal::BusinessTemporal` — a model the real Legend engine (and
    // this crate's own PMCD loader) rejects outright.
    assert_parity(
        "temporal",
        "BUSINESSTEMPORAL",
        "temporal.BUSINESSTEMPORAL",
        Expectation::RejectedValue,
    );
}

#[test]
fn titlecase_value_under_a_canonical_profile_is_rejected_by_both_loaders() {
    assert_parity(
        "meta::pure::profiles::temporal",
        "Bitemporal",
        "meta::pure::profiles::temporal.Bitemporal",
        Expectation::RejectedValue,
    );
}

#[test]
fn uppercase_protocol_profile_is_ignored_by_both_loaders() {
    assert_parity(
        "TEMPORAL",
        "businesstemporal",
        "TEMPORAL.businesstemporal",
        Expectation::IgnoredProfile,
    );
}

#[test]
fn titlecase_protocol_profile_is_ignored_by_both_loaders() {
    assert_parity(
        "Temporal",
        "bitemporal",
        "Temporal.bitemporal",
        Expectation::IgnoredProfile,
    );
}

#[test]
fn wrong_case_profile_and_value_together_are_ignored_by_both_loaders() {
    assert_parity(
        "TEMPORAL",
        "BUSINESSTEMPORAL",
        "TEMPORAL.BUSINESSTEMPORAL",
        Expectation::IgnoredProfile,
    );
}
