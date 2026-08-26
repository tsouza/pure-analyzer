//! Property tests for deterministic PMCD normalization and multiplicity bounds.

#![allow(clippy::disallowed_methods)]

use std::collections::BTreeSet;

use proptest::prelude::*;
use pure_analyzer_model::{ModelError, ModelErrorKind, PmcdDocument, load_pmcd_documents};
use serde_json::{Value, json};

fn class(name: &str, properties: &[String]) -> Value {
    json!({
        "_type": "class",
        "package": "property_test",
        "name": name,
        "superTypes": [],
        "stereotypes": [],
        "properties": properties.iter().map(|property| json!({
            "name": property,
            "genericType": {"rawType": "String"},
            "multiplicity": {"lowerBound": 0, "upperBound": 1}
        })).collect::<Vec<_>>(),
        "qualifiedProperties": []
    })
}

fn load(value: &Value) -> Result<pure_analyzer_model::ModelGraph, ModelError> {
    let json = value.to_string();
    load_pmcd_documents(&[PmcdDocument::new("property", &json)])
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn btree_normalization_is_invariant_to_element_and_property_order(
        raw_names in prop::collection::vec("[a-z][a-z0-9]{0,7}", 0..24)
    ) {
        let names = raw_names.into_iter().collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
        let mut reversed_names = names.clone();
        reversed_names.reverse();

        let forward = json!({
            "_type": "data",
            "elements": [class("Alpha", &names), class("Omega", &reversed_names)]
        });
        let reverse = json!({
            "_type": "data",
            "elements": [class("Omega", &names), class("Alpha", &reversed_names)]
        });

        prop_assert_eq!(load(&forward).expect("forward"), load(&reverse).expect("reverse"));
    }

    #[test]
    fn finite_multiplicity_acceptance_exactly_matches_bound_order(lower in any::<u32>(), upper in any::<u32>()) {
        let value = json!({
            "_type": "data",
            "elements": [{
                "_type": "class",
                "package": "property_test",
                "name": "Bounds",
                "superTypes": [],
                "stereotypes": [],
                "properties": [{
                    "name": "value",
                    "genericType": {"rawType": "String"},
                    "multiplicity": {"lowerBound": lower, "upperBound": upper}
                }],
                "qualifiedProperties": []
            }]
        });
        let result = load(&value);
        if lower <= upper {
            let graph = result.expect("ordered bounds are valid");
            let multiplicity = graph.class("property_test::Bounds").expect("class").properties()["value"].multiplicity();
            prop_assert_eq!(multiplicity.lower(), lower);
            prop_assert_eq!(multiplicity.upper(), Some(upper));
        } else {
            let error = result.expect_err("reversed bounds are invalid");
            let is_invalid_multiplicity = match error {
                ModelError::InvalidElement { kind, .. } => {
                    matches!(*kind, ModelErrorKind::InvalidMultiplicity(_))
                }
                _ => false,
            };
            prop_assert!(is_invalid_multiplicity);
        }
    }
}
