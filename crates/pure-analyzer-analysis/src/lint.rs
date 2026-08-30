//! Model-aware, conservative navigation lints.

use pure_analyzer_diagnostics::{DiagCode, Diagnostic, Label, Severity};
use pure_analyzer_resolve::{LocalValueKind, NavigationResolution};

use crate::{AnalysisInput, AnalysisPass, LocalResolution, analyze_m3_locals};

/// Emits findings that are provable from locally resolved closed-world model facts.
///
/// This pass intentionally does not turn under-resolution, ambiguity, recovery,
/// or Pure-file coverage gaps into missing-property findings.
#[derive(Debug, Default, Clone, Copy)]
pub struct NavigationLintPass;

/// Emits findings for generated milestoning navigations whose supplied date
/// arguments have a conclusively wrong arity.
#[derive(Debug, Default, Clone, Copy)]
pub struct MilestoningArityLintPass;

impl AnalysisPass for NavigationLintPass {
    fn name(&self) -> &'static str {
        "navigation-lints"
    }

    fn analyze(&self, input: AnalysisInput<'_, '_>) -> Vec<Diagnostic> {
        let Some(model) = input.model() else {
            return Vec::new();
        };

        analyze_m3_locals(input.tree(), model)
            .sites()
            .iter()
            .filter_map(|site| {
                let LocalResolution::Navigation(NavigationResolution::Missing(missing)) =
                    site.outcome()
                else {
                    return None;
                };
                if !matches!(
                    missing.failure().completed().value().kind(),
                    LocalValueKind::Class(_)
                ) {
                    return None;
                }
                Some(
                    Diagnostic::builder(
                        DiagCode::UnknownProperty,
                        Severity::Error,
                        "property is not declared on this closed-world class",
                        Label::new(input.file(), site.span()),
                    )
                    .build(),
                )
            })
            .collect()
    }
}

impl AnalysisPass for MilestoningArityLintPass {
    fn name(&self) -> &'static str {
        "milestoning-arity-lints"
    }

    fn analyze(&self, input: AnalysisInput<'_, '_>) -> Vec<Diagnostic> {
        let Some(model) = input.model() else {
            return Vec::new();
        };

        analyze_m3_locals(input.tree(), model)
            .sites()
            .iter()
            .filter_map(|site| {
                let LocalResolution::Navigation(NavigationResolution::WrongArity(mismatch)) =
                    site.outcome()
                else {
                    return None;
                };
                if !mismatch.is_generated_milestoned() {
                    return None;
                }
                Some(
                    Diagnostic::builder(
                        DiagCode::WrongMilestoningArity,
                        Severity::Error,
                        "generated milestoned navigation has the wrong number of dates",
                        Label::new(input.file(), site.span()),
                    )
                    .build(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use pure_analyzer_diagnostics::FileId;
    use pure_analyzer_model::{
        ModelGraph, PmcdDocument, PureDocument, load_pmcd_documents, load_pure_documents,
    };
    use pure_analyzer_parser::parse_query;
    use serde_json::json;

    use super::*;
    use crate::{AnalysisEngine, FindingPolicy};

    fn graph() -> ModelGraph {
        let source = json!({
            "_type": "data",
            "elements": [{
                "_type": "class",
                "package": "model",
                "name": "Person",
                "stereotypes": [],
                "superTypes": [],
                "properties": [{
                    "name": "name",
                    "genericType": {"rawType": "String", "typeArguments": []},
                    "multiplicity": {"lowerBound": 0, "upperBound": 1}
                }],
                "qualifiedProperties": []
            }]
        })
        .to_string();
        load_pmcd_documents(&[PmcdDocument::new("closed-world", &source)])
            .expect("fixture model must load")
    }

    fn milestoning_graph(temporal: Option<&str>) -> ModelGraph {
        let target_stereotypes = temporal
            .map(|value| {
                json!({
                    "profile": "meta::pure::profiles::temporal",
                    "value": value,
                })
            })
            .into_iter()
            .collect::<Vec<_>>();
        let source = json!({
            "_type": "data",
            "elements": [
                {
                    "_type": "class",
                    "package": "model",
                    "name": "TemporalTarget",
                    "stereotypes": target_stereotypes,
                    "superTypes": [],
                    "properties": [],
                    "qualifiedProperties": [],
                },
                {
                    "_type": "class",
                    "package": "model",
                    "name": "Source",
                    "stereotypes": [],
                    "superTypes": [],
                    "properties": [],
                    "qualifiedProperties": [{
                        "name": "point",
                        "returnGenericType": {"rawType": "model::TemporalTarget", "typeArguments": []},
                        "returnMultiplicity": {"lowerBound": 0, "upperBound": 1},
                        "stereotypes": [{
                            "profile": "meta::pure::profiles::milestoning",
                            "value": "generatedmilestoningproperty",
                        }],
                        "parameters": [],
                    }],
                },
            ],
        })
        .to_string();
        load_pmcd_documents(&[PmcdDocument::new("milestoning", &source)])
            .expect("fixture model must load")
    }

    fn pure_milestoning_graph(temporal: &str) -> ModelGraph {
        let source = format!(
            r#"
Class <<temporal.{temporal}>> model::TemporalTarget
{{
}}

Class model::Source
{{
  <<milestoning.generatedmilestoningproperty>>
  point(): model::TemporalTarget[0..1] {{}};
}}
"#
        );
        load_pure_documents(&[PureDocument::new("milestoning.pure", &source)])
            .expect("fixture model must load")
    }

    fn inherited_association_milestoning_graph(temporal: &str) -> ModelGraph {
        let property = |name, target| {
            json!({
                "name": name,
                "genericType": {"rawType": target, "typeArguments": []},
                "multiplicity": {"lowerBound": 0, "upperBound": 1},
            })
        };
        let generated_point = || {
            json!({
                "name": "point",
                "returnGenericType": {"rawType": "model::TemporalTarget", "typeArguments": []},
                "returnMultiplicity": {"lowerBound": 0, "upperBound": 1},
                "stereotypes": [{
                    "profile": "meta::pure::profiles::milestoning",
                    "value": "generatedmilestoningproperty",
                }],
                "parameters": [],
            })
        };
        let user_point = || {
            json!({
                "name": "point",
                "returnGenericType": {"rawType": "String", "typeArguments": []},
                "returnMultiplicity": {"lowerBound": 0, "upperBound": 1},
                "stereotypes": [],
                "parameters": [{
                    "genericType": {"rawType": "Integer", "typeArguments": []},
                }],
            })
        };
        let source = json!({
            "_type": "data",
            "elements": [
                {
                    "_type": "class",
                    "package": "model",
                    "name": "TemporalTarget",
                    "stereotypes": [{
                        "profile": "meta::pure::profiles::temporal",
                        "value": temporal,
                    }],
                    "superTypes": [],
                    "properties": [],
                    "qualifiedProperties": [],
                },
                {
                    "_type": "class",
                    "package": "model",
                    "name": "GeneratedParent",
                    "stereotypes": [],
                    "superTypes": [],
                    "properties": [],
                    "qualifiedProperties": [generated_point()],
                },
                {
                    "_type": "class",
                    "package": "model",
                    "name": "InheritedChild",
                    "stereotypes": [],
                    "superTypes": ["model::GeneratedParent"],
                    "properties": [],
                    "qualifiedProperties": [],
                },
                {
                    "_type": "class",
                    "package": "model",
                    "name": "OverrideChild",
                    "stereotypes": [],
                    "superTypes": ["model::GeneratedParent"],
                    "properties": [],
                    "qualifiedProperties": [user_point()],
                },
                {
                    "_type": "class",
                    "package": "model",
                    "name": "Source",
                    "stereotypes": [],
                    "superTypes": [],
                    "properties": [],
                    "qualifiedProperties": [],
                },
                {
                    "_type": "association",
                    "package": "model",
                    "name": "Source_Child",
                    "stereotypes": [],
                    "properties": [
                        property("inheritedChild", "model::InheritedChild"),
                        property("source", "model::Source"),
                    ],
                },
                {
                    "_type": "association",
                    "package": "model",
                    "name": "Source_Override",
                    "stereotypes": [],
                    "properties": [
                        property("overrideChild", "model::OverrideChild"),
                        property("overrideSource", "model::Source"),
                    ],
                },
            ],
        })
        .to_string();
        load_pmcd_documents(&[PmcdDocument::new("inherited-association", &source)])
            .expect("fixture model must load")
    }

    fn chained_milestoning_graph() -> ModelGraph {
        let generated = |name| {
            json!({
                "name": name,
                "returnGenericType": {"rawType": "model::Temporal", "typeArguments": []},
                "returnMultiplicity": {"lowerBound": 0, "upperBound": 1},
                "stereotypes": [{
                    "profile": "meta::pure::profiles::milestoning",
                    "value": "generatedmilestoningproperty",
                }],
                "parameters": [],
            })
        };
        let source = json!({
            "_type": "data",
            "elements": [
                {
                    "_type": "class",
                    "package": "model",
                    "name": "Temporal",
                    "stereotypes": [{
                        "profile": "meta::pure::profiles::temporal",
                        "value": "processingtemporal",
                    }],
                    "superTypes": [],
                    "properties": [{
                        "name": "plain",
                        "genericType": {"rawType": "model::Temporal", "typeArguments": []},
                        "multiplicity": {"lowerBound": 0, "upperBound": 1},
                    }],
                    "qualifiedProperties": [
                        generated("next"),
                        {
                            "name": "zero",
                            "returnGenericType": {"rawType": "model::Temporal", "typeArguments": []},
                            "returnMultiplicity": {"lowerBound": 0, "upperBound": 1},
                            "stereotypes": [],
                            "parameters": [],
                        },
                    ],
                },
                {
                    "_type": "class",
                    "package": "model",
                    "name": "Source",
                    "stereotypes": [],
                    "superTypes": [],
                    "properties": [],
                    "qualifiedProperties": [generated("first")],
                },
            ],
        })
        .to_string();
        load_pmcd_documents(&[PmcdDocument::new("chained-milestoning", &source)])
            .expect("fixture model must load")
    }

    fn member_aware_graph() -> ModelGraph {
        let property = |name, target| {
            json!({
                "name": name,
                "genericType": {"rawType": target, "typeArguments": []},
                "multiplicity": {"lowerBound": 0, "upperBound": 1}
            })
        };
        let class = |name, supertypes, properties, qualified_properties| {
            json!({
                "_type": "class",
                "package": "model",
                "name": name,
                "stereotypes": [],
                "superTypes": supertypes,
                "properties": properties,
                "qualifiedProperties": qualified_properties,
            })
        };
        let source = json!({
            "_type": "data",
            "elements": [
                class("Base", Vec::<&str>::new(), vec![property("inherited", "String")], Vec::new()),
                class("Child", vec!["model::Base"], Vec::new(), Vec::new()),
                class("Person", Vec::<&str>::new(), Vec::new(), vec![json!({
                    "name": "byKey",
                    "returnGenericType": {"rawType": "String", "typeArguments": []},
                    "returnMultiplicity": {"lowerBound": 0, "upperBound": 1},
                    "stereotypes": [],
                    "parameters": [{"genericType": {"rawType": "Integer", "typeArguments": []}}],
                })]),
                class("Manager", Vec::<&str>::new(), vec![property("name", "String")], Vec::new()),
                json!({
                    "_type": "association",
                    "package": "model",
                    "name": "Person_Manager",
                    "stereotypes": [],
                    "properties": [
                        property("manager", "model::Manager"),
                        property("reports", "model::Person"),
                    ],
                }),
            ],
        })
        .to_string();
        load_pmcd_documents(&[PmcdDocument::new("member-aware", &source)])
            .expect("fixture model must load")
    }

    fn diagnostics(source: &str, model: Option<&ModelGraph>) -> Vec<Diagnostic> {
        let parsed = parse_query(source, FileId::new(8)).expect("fixture must parse");
        AnalysisEngine::new(vec![Box::new(NavigationLintPass)], FindingPolicy::new())
            .analyze(AnalysisInput::new(
                FileId::new(8),
                source,
                &parsed.green,
                &parsed.diagnostics,
                model,
            ))
            .into_diagnostics()
    }

    fn milestoning_diagnostics(source: &str, model: Option<&ModelGraph>) -> Vec<Diagnostic> {
        let parsed = parse_query(source, FileId::new(8)).expect("fixture must parse");
        AnalysisEngine::new(
            vec![Box::new(MilestoningArityLintPass)],
            FindingPolicy::new(),
        )
        .analyze(AnalysisInput::new(
            FileId::new(8),
            source,
            &parsed.green,
            &parsed.diagnostics,
            model,
        ))
        .into_diagnostics()
    }

    #[test]
    fn reports_only_closed_world_missing_class_properties_at_the_navigation_span() {
        let model = graph();
        let source = "model::Person.all()->filter(x| $x.missing)";
        let findings = diagnostics(source, Some(&model));

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, DiagCode::UnknownProperty);
        let span = findings[0].primary.span;
        assert_eq!(
            &source[usize::from(span.start())..usize::from(span.end())],
            ".missing"
        );
        assert!(diagnostics("model::Person.all()->filter(x| $x.name)", Some(&model)).is_empty());
        assert!(diagnostics(source, None).is_empty());
    }

    #[test]
    fn respects_inherited_association_and_qualified_members() {
        let model = member_aware_graph();
        for source in [
            "model::Child.all()->filter(x| $x.inherited)",
            "model::Person.all()->filter(x| $x.manager.name)",
            "model::Person.all()->filter(x| $x.byKey(25))",
        ] {
            assert!(
                diagnostics(source, Some(&model)).is_empty(),
                "known member must not be linted: {source}"
            );
        }
    }

    #[test]
    fn reports_only_confirmed_generated_milestoning_arity_mismatches() {
        let model = milestoning_graph(Some("processingtemporal"));
        let source = "model::Source.all()->filter(x| $x.point())";
        let findings = milestoning_diagnostics(source, Some(&model));

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, DiagCode::WrongMilestoningArity);
        let span = findings[0].primary.span;
        assert_eq!(
            &source[usize::from(span.start())..usize::from(span.end())],
            ".point()"
        );
        assert!(
            milestoning_diagnostics(
                "model::Source.all()->filter(x| $x.point(%latest))",
                Some(&model),
            )
            .is_empty()
        );
        assert!(
            milestoning_diagnostics("model::Person.all()->filter(x| $x.name(1))", Some(&graph()))
                .is_empty()
        );
        assert!(milestoning_diagnostics(source, None).is_empty());

        let business_model = milestoning_graph(Some("businesstemporal"));
        assert_eq!(
            milestoning_diagnostics(source, Some(&business_model))
                .into_iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            vec![DiagCode::WrongMilestoningArity]
        );
        assert!(
            milestoning_diagnostics(
                "model::Source.all()->filter(x| $x.point(%latest))",
                Some(&business_model),
            )
            .is_empty()
        );
    }

    #[test]
    fn applies_the_non_temporal_zero_date_arity() {
        let model = milestoning_graph(None);

        assert!(
            milestoning_diagnostics("model::Source.all()->filter(x| $x.point())", Some(&model),)
                .is_empty()
        );
        assert_eq!(
            milestoning_diagnostics(
                "model::Source.all()->filter(x| $x.point(%latest))",
                Some(&model),
            )
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
            vec![DiagCode::WrongMilestoningArity]
        );
    }

    #[test]
    fn applies_the_bitemporal_two_date_arity() {
        let model = milestoning_graph(Some("bitemporal"));
        let one_date = "model::Source.all()->filter(x| $x.point(%latest))";
        let findings = milestoning_diagnostics(one_date, Some(&model));

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, DiagCode::WrongMilestoningArity);
        assert!(
            milestoning_diagnostics(
                "model::Source.all()->filter(x| $x.point(%latest, %latest))",
                Some(&model),
            )
            .is_empty()
        );
    }

    #[test]
    fn generated_milestoning_lint_has_complete_pmcd_pure_truth_table_parity() {
        let cases = [
            (None, [false, true, true, true]),
            (Some("businesstemporal"), [true, false, false, true]),
            (Some("processingtemporal"), [true, false, false, true]),
            (Some("bitemporal"), [true, true, true, false]),
        ];
        let sources = [
            "model::Source.all()->filter(x| $x.point())",
            "model::Source.all()->filter(x| $x.point(%latest))",
            "model::Source.all()->filter(x| $x.point(%2020-01-01))",
            "model::Source.all()->filter(x| $x.point(%latest, %2020-01-01))",
        ];

        for (temporal, expected_findings) in cases {
            let pmcd = milestoning_graph(temporal);
            let pure = pure_milestoning_graph(temporal.unwrap_or(""));
            for (source, expected_finding) in sources.iter().zip(expected_findings) {
                let pmcd_findings = milestoning_diagnostics(source, Some(&pmcd))
                    .into_iter()
                    .map(|diagnostic| diagnostic.code)
                    .collect::<Vec<_>>();
                let pure_findings = milestoning_diagnostics(source, Some(&pure))
                    .into_iter()
                    .map(|diagnostic| diagnostic.code)
                    .collect::<Vec<_>>();
                assert_eq!(pmcd_findings, pure_findings, "loader parity: {source}");
                assert_eq!(
                    !pmcd_findings.is_empty(),
                    expected_finding,
                    "temporal={temporal:?}: {source}"
                );
            }
        }
    }

    #[test]
    fn applies_only_confirmed_generated_arity_after_association_and_inheritance() {
        let model = inherited_association_milestoning_graph("processingtemporal");
        let missing_date = "model::Source.all()->filter(x| $x.inheritedChild.point())";

        assert_eq!(
            milestoning_diagnostics(missing_date, Some(&model))
                .into_iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            vec![DiagCode::WrongMilestoningArity]
        );
        assert!(
            milestoning_diagnostics(
                "model::Source.all()->filter(x| $x.inheritedChild.point(%latest))",
                Some(&model),
            )
            .is_empty()
        );
        assert!(
            milestoning_diagnostics(
                "model::Source.all()->filter(x| $x.overrideChild.point())",
                Some(&model),
            )
            .is_empty()
        );
        assert_eq!(
            milestoning_diagnostics(
                "model::InheritedChild.all()->filter(x| $x.source.inheritedChild.point())",
                Some(&model),
            )
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
            vec![DiagCode::WrongMilestoningArity]
        );

        let bitemporal = inherited_association_milestoning_graph("bitemporal");
        assert_eq!(
            milestoning_diagnostics(
                "model::Source.all()->filter(x| $x.inheritedChild.point(%latest))",
                Some(&bitemporal),
            )
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
            vec![DiagCode::WrongMilestoningArity]
        );
        assert!(
            milestoning_diagnostics(
                "model::Source.all()->filter(x| $x.inheritedChild.point(%latest, %latest))",
                Some(&bitemporal),
            )
            .is_empty()
        );
    }

    #[test]
    fn requires_a_fresh_date_at_each_generated_hop_after_resets() {
        let model = chained_milestoning_graph();
        for source in [
            "model::Source.all()->filter(x| $x.first(%latest).next())",
            "model::Source.all()->filter(x| $x.first(%2020-01-01).plain.next())",
            "model::Source.all()->filter(x| $x.first(%latest).zero().next())",
        ] {
            assert_eq!(
                milestoning_diagnostics(source, Some(&model))
                    .into_iter()
                    .map(|diagnostic| diagnostic.code)
                    .collect::<Vec<_>>(),
                vec![DiagCode::WrongMilestoningArity],
                "each generated hop needs a fresh date after reset: {source}"
            );
        }
        for source in [
            "model::Source.all()->filter(x| $x.first(%latest).next(%latest))",
            "model::Source.all()->filter(x| $x.first(%2020-01-01).plain.next(%latest))",
            "model::Source.all()->filter(x| $x.first(%latest).zero().next(%2020-01-01))",
        ] {
            assert!(
                milestoning_diagnostics(source, Some(&model)).is_empty(),
                "fresh date is accepted at every generated hop: {source}"
            );
        }
    }

    #[test]
    fn navigation_lint_pass_name_is_stable() {
        assert_eq!(NavigationLintPass.name(), "navigation-lints");
    }

    #[test]
    fn milestoning_arity_lint_pass_name_is_stable() {
        assert_eq!(MilestoningArityLintPass.name(), "milestoning-arity-lints");
    }
}
