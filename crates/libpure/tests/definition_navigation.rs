//! Front-end-neutral definition-navigation contracts for the libpure facade.

use libpure::{
    AnalysisDriver, DefinitionPosition, DefinitionResult, DefinitionUnavailable, FileId,
    LintRequest, ModelInput, SourceInput, SourceRequest, TextRange, TextSize,
};

const PERSON_MODEL: &str = r#"
Class model::Person
{
  manager: model::Manager[0..1];
}
"#;
const MANAGER_MODEL: &str = r#"
Class model::Manager
{
  name: String[1];
}
"#;
const PMCD_MODEL: &str = r#"{
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
}"#;
const PARTIAL_MODEL: &str = r#"
Enum model::Future { enabled }
Class model::Partial
{
  known: String[1];
}
"#;
const AMBIGUOUS_MODEL: &str = r#"
Class model::Left
{
  shared: String[1];
}
Class model::Right
{
  shared: Integer[1];
}
Class model::Child extends model::Left, model::Right
{
}
"#;
const CYCLIC_MODEL: &str = r#"
Class model::A extends model::B
{
}
Class model::B extends model::A
{
}
"#;

fn position(file: FileId, source: &str, reference: &str) -> DefinitionPosition {
    let offset = source.find(reference).expect("reference must occur once")
        + reference
            .find(|character: char| character.is_alphabetic())
            .expect("reference has a name");
    position_at_offset(file, offset)
}

fn position_at(file: FileId, source: &str, fragment: &str) -> DefinitionPosition {
    let offset = source.find(fragment).expect("fragment must occur once");
    position_at_offset(file, offset)
}

fn position_at_offset(file: FileId, offset: usize) -> DefinitionPosition {
    DefinitionPosition::new(
        file,
        u32::try_from(offset)
            .expect("fixture source fits TextSize")
            .into(),
    )
}

fn exact_span(source: &str, declaration: &str) -> TextRange {
    let start = source
        .find(declaration)
        .expect("declaration must occur once");
    let end = start + declaration.len();
    TextRange::new(
        u32::try_from(start)
            .expect("fixture source fits TextRange")
            .into(),
        u32::try_from(end)
            .expect("fixture source fits TextRange")
            .into(),
    )
}

fn assert_found(result: DefinitionResult, expected_file: FileId, expected_span: Option<TextRange>) {
    let DefinitionResult::Found(target) = result else {
        panic!("expected a definition target, got {result:#?}");
    };
    assert_eq!(target.file(), expected_file);
    assert_eq!(target.span(), expected_span);
}

#[test]
fn model_navigation_returns_deterministic_cross_source_anchors() {
    let query = "model::Person.all()->filter(x| $x.manager.name)";
    let request = LintRequest::new(
        SourceRequest::new([SourceInput::in_memory("query.pure", query)]),
        [
            ModelInput::pure(SourceInput::in_memory("person.pure", PERSON_MODEL)),
            ModelInput::pure(SourceInput::in_memory("manager.pure", MANAGER_MODEL)),
        ],
    );
    let driver = AnalysisDriver;
    let query_file = request.query_file_id(0).expect("query input has a file ID");
    let manager_file = request
        .model_file_id(1)
        .expect("manager input has a file ID");
    let person_file = request
        .model_file_id(0)
        .expect("person input has a file ID");
    let name = position(query_file, query, ".name");
    let class = position(query_file, query, "model::Person");

    let first = driver
        .definition(&request, name)
        .expect("resolve name definition");
    let repeated = driver
        .definition(&request, name)
        .expect("resolve name definition again");
    assert_eq!(first, repeated);
    assert_found(
        first,
        manager_file,
        Some(exact_span(MANAGER_MODEL, "name: String[1];")),
    );
    assert_found(
        driver
            .definition(&request, class)
            .expect("resolve class definition"),
        person_file,
        Some(exact_span(PERSON_MODEL, PERSON_MODEL.trim())),
    );
}

#[test]
fn request_file_ids_match_lint_and_definition_source_identity() {
    let person_query = "model::Person.all()";
    let manager_query = "model::Manager.all()->filter(x| $x.name)";
    let request = LintRequest::new(
        SourceRequest::new([
            SourceInput::in_memory("person-query.pure", person_query),
            SourceInput::in_memory("manager-query.pure", manager_query),
        ]),
        [
            ModelInput::pure(SourceInput::in_memory("person.pure", PERSON_MODEL)),
            ModelInput::pure(SourceInput::in_memory("manager.pure", MANAGER_MODEL)),
        ],
    );

    let person_model = request.model_file_id(0).expect("first model has a file ID");
    let manager_model = request
        .model_file_id(1)
        .expect("second model has a file ID");
    let person_query_file = request.query_file_id(0).expect("first query has a file ID");
    let manager_query_file = request
        .query_file_id(1)
        .expect("second query has a file ID");
    assert_eq!(person_model, FileId::new(0));
    assert_eq!(manager_model, FileId::new(1));
    assert_eq!(person_query_file, FileId::new(2));
    assert_eq!(manager_query_file, FileId::new(3));
    assert_eq!(request.model_file_id(2), None);
    assert_eq!(request.query_file_id(2), None);

    let output = AnalysisDriver.lint(&request).expect("lint request loads");
    for (file, name) in [
        (person_model, "person.pure"),
        (manager_model, "manager.pure"),
        (person_query_file, "person-query.pure"),
        (manager_query_file, "manager-query.pure"),
    ] {
        assert_eq!(
            output.sources().get(file).map(|source| source.name()),
            Some(name)
        );
    }

    assert_found(
        AnalysisDriver
            .definition(
                &request,
                position(manager_query_file, manager_query, ".name"),
            )
            .expect("definition request loads"),
        manager_model,
        Some(exact_span(MANAGER_MODEL, "name: String[1];")),
    );
}

#[test]
fn recovered_queries_do_not_return_fabricated_definition_anchors() {
    for (query, reference) in [
        ("model::Person.all(", "model::Person"),
        ("model::Person.all()->filter(x| $x.name(", ".name"),
    ] {
        let request = LintRequest::new(
            SourceRequest::new([SourceInput::in_memory("query.pure", query)]),
            [ModelInput::pmcd(SourceInput::in_memory(
                "model.json",
                PMCD_MODEL,
            ))],
        );
        let query_file = request.query_file_id(0).expect("query input has a file ID");
        let parsed = AnalysisDriver
            .parse(request.sources())
            .expect("recovered query can retain a syntax tree");
        assert!(
            !parsed.diagnostics().is_empty(),
            "fixture must recover: {query}"
        );

        assert_eq!(
            AnalysisDriver
                .definition(&request, position(query_file, query, reference))
                .expect("definition request loads"),
            DefinitionResult::Unavailable(DefinitionUnavailable::Recovery),
        );
    }
}

#[test]
fn ambiguous_and_cyclic_model_navigation_remain_unavailable() {
    for (model, query, reference, unavailable) in [
        (
            AMBIGUOUS_MODEL,
            "model::Child.all()->filter(x| $x.shared)",
            ".shared",
            DefinitionUnavailable::Ambiguous,
        ),
        (
            CYCLIC_MODEL,
            "model::A.all()->filter(x| $x.missing)",
            ".missing",
            DefinitionUnavailable::Cycle,
        ),
    ] {
        let request = LintRequest::new(
            SourceRequest::new([SourceInput::in_memory("query.pure", query)]),
            [ModelInput::pure(SourceInput::in_memory(
                "model.pure",
                model,
            ))],
        );
        let query_file = request.query_file_id(0).expect("query input has a file ID");

        assert_eq!(
            AnalysisDriver
                .definition(&request, position(query_file, query, reference))
                .expect("definition request loads"),
            DefinitionResult::Unavailable(unavailable),
        );
    }
}

#[test]
fn local_relation_navigation_keeps_query_source_identity_and_declaration_span() {
    let query = "{row: Relation<(zeta:String[1], alpha:Integer[0..1])>| $row.alpha}";
    let request = LintRequest::new(
        SourceRequest::new([SourceInput::in_memory("query.pure", query)]),
        [],
    );

    let query_file = request.query_file_id(0).expect("query input has a file ID");
    assert_found(
        AnalysisDriver
            .definition(&request, position(query_file, query, ".alpha"))
            .expect("resolve local relation column"),
        query_file,
        Some(exact_span(query, "alpha:Integer[0..1]")),
    );
}

#[test]
fn pmcd_navigation_retains_source_identity_when_the_model_has_no_span() {
    let query = "model::Person.all()->filter(x| $x.name)";
    let request = LintRequest::new(
        SourceRequest::new([SourceInput::in_memory("query.pure", query)]),
        [ModelInput::pmcd(SourceInput::in_memory(
            "model.json",
            PMCD_MODEL,
        ))],
    );

    let query_file = request.query_file_id(0).expect("query input has a file ID");
    let model_file = request.model_file_id(0).expect("model input has a file ID");
    assert_found(
        AnalysisDriver
            .definition(&request, position(query_file, query, ".name"))
            .expect("resolve PMCD member"),
        model_file,
        None,
    );
}

#[test]
fn navigation_selects_the_reference_identifier_not_the_surrounding_call() {
    let query = "model::Person.all()->filter(x| $x.name(1))";
    let request = LintRequest::new(
        SourceRequest::new([SourceInput::in_memory("query.pure", query)]),
        [ModelInput::pmcd(SourceInput::in_memory(
            "model.json",
            PMCD_MODEL,
        ))],
    );

    let query_file = request.query_file_id(0).expect("query input has a file ID");
    let model_file = request.model_file_id(0).expect("model input has a file ID");
    assert_found(
        AnalysisDriver
            .definition(&request, position(query_file, query, ".name"))
            .expect("wrong-arity member still has its definition"),
        model_file,
        None,
    );
    assert_eq!(
        AnalysisDriver
            .definition(&request, position_at(query_file, query, "1"))
            .expect("call argument is not a definition reference"),
        DefinitionResult::Unavailable(DefinitionUnavailable::NoReference),
    );
}

#[test]
fn unavailable_results_do_not_fabricate_definition_anchors() {
    let no_model_query = "model::Person.all()";
    let no_model = LintRequest::new(
        SourceRequest::new([SourceInput::in_memory("query.pure", no_model_query)]),
        [],
    );
    let no_model_query_file = no_model
        .query_file_id(0)
        .expect("query input has a file ID");
    assert_eq!(
        AnalysisDriver
            .definition(
                &no_model,
                position(no_model_query_file, no_model_query, "model::Person")
            )
            .expect("class lookup without a model"),
        DefinitionResult::Unavailable(DefinitionUnavailable::NoModel),
    );

    let missing_query = "model::Person.all()->filter(x| $x.missing)";
    let missing = LintRequest::new(
        SourceRequest::new([SourceInput::in_memory("query.pure", missing_query)]),
        [ModelInput::pmcd(SourceInput::in_memory(
            "model.json",
            PMCD_MODEL,
        ))],
    );
    let missing_query_file = missing.query_file_id(0).expect("query input has a file ID");
    assert_eq!(
        AnalysisDriver
            .definition(
                &missing,
                position(missing_query_file, missing_query, ".missing")
            )
            .expect("closed-world miss"),
        DefinitionResult::Unavailable(DefinitionUnavailable::Missing),
    );

    let partial_query = "model::Partial.all()->filter(x| $x.known)";
    let partial = LintRequest::new(
        SourceRequest::new([SourceInput::in_memory("query.pure", partial_query)]),
        [ModelInput::pure(SourceInput::in_memory(
            "partial.pure",
            PARTIAL_MODEL,
        ))],
    );
    let partial_query_file = partial.query_file_id(0).expect("query input has a file ID");
    assert_eq!(
        AnalysisDriver
            .definition(
                &partial,
                position(partial_query_file, partial_query, ".known")
            )
            .expect("open-world member lookup"),
        DefinitionResult::Unavailable(DefinitionUnavailable::UnderResolved),
    );

    let unresolved_query = "model::Person.all()->filter(x| $unbound.name)";
    let unresolved = LintRequest::new(
        SourceRequest::new([SourceInput::in_memory("query.pure", unresolved_query)]),
        [ModelInput::pmcd(SourceInput::in_memory(
            "model.json",
            PMCD_MODEL,
        ))],
    );
    let unresolved_query_file = unresolved
        .query_file_id(0)
        .expect("query input has a file ID");
    assert_eq!(
        AnalysisDriver
            .definition(
                &unresolved,
                position(unresolved_query_file, unresolved_query, ".name")
            )
            .expect("unbound local lookup"),
        DefinitionResult::Unavailable(DefinitionUnavailable::UnderResolved),
    );
}

#[test]
fn definition_positions_are_checked_against_retained_query_snapshots() {
    let unicode_query = "'é'";
    let request = LintRequest::new(
        SourceRequest::new([SourceInput::in_memory("query.pure", unicode_query)]),
        [],
    );
    let query_file = request.query_file_id(0).expect("query input has a file ID");

    assert_eq!(
        AnalysisDriver
            .definition(
                &request,
                DefinitionPosition::new(FileId::new(9), TextSize::from(0)),
            )
            .expect("unknown file is a normal outcome"),
        DefinitionResult::Unavailable(DefinitionUnavailable::UnknownFile {
            file: FileId::new(9),
        }),
    );
    assert_eq!(
        AnalysisDriver
            .definition(
                &request,
                DefinitionPosition::new(query_file, TextSize::from(2)),
            )
            .expect("mid-code-point position is a normal outcome"),
        DefinitionResult::Unavailable(DefinitionUnavailable::InvalidPosition {
            file: query_file,
            offset: TextSize::from(2),
        }),
    );
    assert_eq!(
        AnalysisDriver
            .definition(
                &request,
                DefinitionPosition::new(query_file, TextSize::from(0)),
            )
            .expect("non-reference position is a normal outcome"),
        DefinitionResult::Unavailable(DefinitionUnavailable::NoReference),
    );
}

#[test]
fn model_source_positions_are_not_treated_as_query_references() {
    let request = LintRequest::new(
        SourceRequest::new([SourceInput::in_memory("query.pure", "value")]),
        [ModelInput::pmcd(SourceInput::in_memory(
            "model.json",
            PMCD_MODEL,
        ))],
    );
    let model_file = request.model_file_id(0).expect("model input has a file ID");

    assert_eq!(
        AnalysisDriver
            .definition(
                &request,
                DefinitionPosition::new(model_file, TextSize::from(0)),
            )
            .expect("model file selection is a normal outcome"),
        DefinitionResult::Unavailable(DefinitionUnavailable::NonQueryFile { file: model_file }),
    );
}
