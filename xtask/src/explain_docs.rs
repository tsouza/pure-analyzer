//! Generated user-facing documentation for registered explain identifiers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use pure_analyzer_diagnostics::{
    ALL_DIAG_CODES, ALL_REASON_CODES, ExplainClassification, ExplainContent, ExplainKind,
};

use crate::process::run_stdout;

/// Repository-relative directory containing the generated reference pages.
const EXPLAIN_DIRECTORY: &str = "docs/explain";
/// Name of the generated index in [`EXPLAIN_DIRECTORY`].
const INDEX_PAGE: &str = "README.md";
/// Repository-relative M4a comparison corpus that supplies explain examples.
const M4A_COMPARISON_CORPUS: &str =
    "crates/pure-analyzer-analysis/corpus/legend-4.113.0/comparison.jsonl";

/// One explain-page projection of a verified M4a corpus entry.
///
/// Query text, models, oracles, and executable comparison expectations remain
/// exclusively in [`M4A_COMPARISON_CORPUS`]. This mapping only states which
/// existing explain pages should link to each verified result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct M4aExplainExample {
    corpus_id: &'static str,
    expected_outcome: &'static str,
    expected_reason: Option<&'static str>,
    explain_identifiers: &'static [&'static str],
}

/// The deliberately small, independently verified M4a explain set.
///
/// Only indecisive examples are linkable here: a decisive `equivalent` or
/// `not_equivalent` outcome has no registered explain identifier of its own
/// (`DiagCode::EquivalenceVerdict`/`PUR3001` was removed — see issue #287 —
/// because `eq`/`diff` never actually attaches it to a `Diagnostic`; the
/// verdict itself is fully covered by `docs/pure-analyzer.md`'s `eq`
/// reference and the executable M4a corpus). An indecisive result still has
/// a registered, genuinely producer-backed reason identifier to link.
const M4A_EXPLAIN_EXAMPLES: &[M4aExplainExample] = &[M4aExplainExample {
    corpus_id: "different-literal-filters-remain-indecisive",
    expected_outcome: "indecisive",
    expected_reason: Some("IND_MISSING_REWRITE"),
    explain_identifiers: &["IND_MISSING_REWRITE"],
}];

/// A corpus-backed example with its current one-based source line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedM4aExplainExample {
    example: &'static M4aExplainExample,
    line: usize,
}

/// Generate the tracked explain reference from the authoritative catalog.
///
/// This is intentionally explicit rather than an implicit build side effect:
/// the generated Markdown is committed product documentation and the matching
/// [`check`] gate makes a stale checkout fail deterministically.
///
/// # Errors
///
/// Returns an error when Git cannot identify the repository root, a target
/// directory cannot be created, or a generated page cannot be written.
pub fn generate() -> Result<()> {
    let root = repository_root()?;
    for (relative_path, content) in expected_documents(&root)? {
        let path = root.join(relative_path);
        let parent = path
            .parent()
            .context("generated explain page has no parent directory")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Verify every registered explain identifier has exactly one current page.
///
/// # Errors
///
/// Returns an error when Git cannot identify the repository root, a reference
/// page cannot be read, or the checked-in reference differs from the shared
/// explain catalog.
pub fn check() -> Result<()> {
    let root = repository_root()?;
    let expected = expected_documents(&root)?;
    let actual = read_reference_documents(&root)?;
    let problems = document_problems(&expected, &actual);
    if problems.is_empty() {
        return Ok(());
    }

    anyhow::bail!(
        "explain reference drift; run `just generate-explain-docs` and commit the result:\n{}",
        problems
            .iter()
            .map(|problem| format!("  - {problem}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Resolve the repository root so both commands work from any subdirectory.
fn repository_root() -> Result<PathBuf> {
    let output = run_stdout("git", &["rev-parse", "--show-toplevel"])?;
    let root = output.trim();
    if root.is_empty() {
        anyhow::bail!("`git rev-parse --show-toplevel` returned an empty repository root");
    }
    Ok(PathBuf::from(root))
}

/// Render the complete expected set, keyed by repository-relative path.
fn expected_documents(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    let contents = catalog();
    validate_m4a_example_identifiers(&contents, M4A_EXPLAIN_EXAMPLES)?;
    let m4a_examples = resolve_m4a_explain_examples(root)?;
    let mut documents = BTreeMap::new();
    documents.insert(
        Path::new(EXPLAIN_DIRECTORY).join(INDEX_PAGE),
        render_index(&contents),
    );
    for content in contents {
        documents.insert(
            Path::new(EXPLAIN_DIRECTORY).join(format!("{}.md", content.identifier)),
            render_page(content, &m4a_examples),
        );
    }
    Ok(documents)
}

/// Return catalog entries in their stable public registry order.
fn catalog() -> Vec<&'static ExplainContent> {
    ALL_DIAG_CODES
        .iter()
        .map(|code| code.explanation())
        .chain(ALL_REASON_CODES.iter().map(|reason| reason.explanation()))
        .collect()
}

/// Render the Markdown index with one relative link for every reference page.
fn render_index(contents: &[&ExplainContent]) -> String {
    let mut output = String::from(
        "# Diagnostic and reason reference\n\n\
         `pure-analyzer` uses stable diagnostic and conservative-reason identifiers. \
         Each entry below links to its exact user-facing reference.\n\n\
         A `fundamental` reason marks a soundness boundary, while a `recoverable` reason \
         marks an implementation limitation; neither makes a query erroneous.\n",
    );
    for (kind, heading) in [
        (ExplainKind::Diagnostic, "Diagnostics"),
        (ExplainKind::Reason, "Conservative reasons"),
    ] {
        output.push_str(&format!("\n## {heading}\n\n"));
        for content in contents
            .iter()
            .copied()
            .filter(|content| content.kind == kind)
        {
            output.push_str(&format!(
                "- [`{}`]({}.md): `{}`. {}",
                content.identifier,
                content.identifier,
                content.classification.as_str(),
                content.meaning,
            ));
            output.push('\n');
        }
    }
    output
}

/// Render one durable, human-readable reference page from catalog content.
fn render_page(content: &ExplainContent, m4a_examples: &[ResolvedM4aExplainExample]) -> String {
    let mut output = format!(
        "# `{}`\n\n\
         [Back to the explain index]({INDEX_PAGE})\n\n\
         - Kind: `{}`\n\
         - Classification: `{}`\n\n\
         ## Meaning\n\n\
         {}\n\n\
         ## Limit\n\n\
         {}\n\n\
         ## Remedy\n\n\
         {}\n",
        content.identifier,
        content.kind.as_str(),
        content.classification.as_str(),
        content.meaning,
        content.limit,
        content.remedy,
    );

    let examples: Vec<_> = m4a_examples
        .iter()
        .filter(|example| {
            example
                .example
                .explain_identifiers
                .contains(&content.identifier)
        })
        .collect();
    if examples.is_empty() {
        return output;
    }

    output.push_str(
        "\n## Verified M4a examples\n\n\
         These links are generated from the verified comparison corpus; query, model, and \
         oracle details remain in that executable corpus.\n\n",
    );
    for example in examples {
        let reason = example
            .example
            .expected_reason
            .map_or_else(String::new, |reason| format!(" with reason `{reason}`"));
        output.push_str(&format!(
            "- [`{}`]({}): verified `{}` verdict{reason}.\n",
            example.example.corpus_id,
            corpus_link(example.line),
            example.example.expected_outcome,
        ));
    }
    if let Some(sentence) = reason_classification_sentence(content) {
        output.push_str(sentence);
    }
    output
}

/// The classification sentence for a reason page, keyed by catalog data.
///
/// Diagnostics carry no such sentence; a reason page states which side of the
/// soundness boundary it sits on so an indecisive result is never read as an
/// input error.
fn reason_classification_sentence(content: &ExplainContent) -> Option<&'static str> {
    if content.kind != ExplainKind::Reason {
        return None;
    }
    match content.classification {
        ExplainClassification::Recoverable => Some(
            "\nThis `recoverable` reason records engineering backlog: a conservative \
             implementation limitation. The result stays indecisive and the input stays valid.\n",
        ),
        ExplainClassification::Fundamental => Some(
            "\nThis `fundamental` reason records a deliberate soundness boundary rather than \
             engineering backlog. The result stays indecisive and the input stays valid.\n",
        ),
        _ => None,
    }
}

/// Check that each M4a example targets an identifier in the explain catalog.
fn validate_m4a_example_identifiers(
    contents: &[&ExplainContent],
    examples: &[M4aExplainExample],
) -> Result<()> {
    let kinds: BTreeMap<_, _> = contents
        .iter()
        .map(|content| (content.identifier, content.kind))
        .collect();
    for example in examples {
        for identifier in example.explain_identifiers {
            let Some(kind) = kinds.get(identifier) else {
                anyhow::bail!(
                    "M4a explain example {:?} targets unknown explain identifier {identifier:?}",
                    example.corpus_id
                );
            };
            // A reason page may only advertise an example whose verified reason
            // is that same reason; otherwise the page documents a result it did
            // not produce.
            if *kind == ExplainKind::Reason && example.expected_reason != Some(*identifier) {
                anyhow::bail!(
                    "M4a explain example {:?} links reason page {identifier:?} but its verified reason is {:?}",
                    example.corpus_id,
                    example.expected_reason
                );
            }
        }
        if let Some(reason) = example.expected_reason
            && !example.explain_identifiers.contains(&reason)
        {
            anyhow::bail!(
                "M4a explain example {:?} has verified reason {reason:?} but does not link its page",
                example.corpus_id
            );
        }
    }
    Ok(())
}

/// Resolve the configured M4a explain examples against the committed corpus.
fn resolve_m4a_explain_examples(root: &Path) -> Result<Vec<ResolvedM4aExplainExample>> {
    let path = root.join(M4A_COMPARISON_CORPUS);
    let corpus = std::fs::read_to_string(&path)
        .with_context(|| format!("reading M4a comparison corpus {}", path.display()))?;
    resolve_m4a_explain_examples_from_corpus(&corpus)
}

/// Resolve examples from JSON Lines content and reject stale semantic mappings.
fn resolve_m4a_explain_examples_from_corpus(
    corpus: &str,
) -> Result<Vec<ResolvedM4aExplainExample>> {
    let mut entries = BTreeMap::new();
    for (line_index, source) in corpus.lines().enumerate() {
        let source = source.trim();
        if source.is_empty() {
            continue;
        }
        let line = line_index + 1;
        let entry: serde_json::Value = serde_json::from_str(source)
            .with_context(|| format!("parsing M4a comparison corpus line {line}"))?;
        let id = corpus_string(&entry, "id", line)?;
        let outcome = corpus_string(&entry, "outcome", line)?;
        let reason = entry
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        if entries
            .insert(id.to_owned(), (line, outcome.to_owned(), reason))
            .is_some()
        {
            anyhow::bail!("M4a comparison corpus has duplicate example identifier {id:?}");
        }
    }

    let mut resolved = Vec::with_capacity(M4A_EXPLAIN_EXAMPLES.len());
    for example in M4A_EXPLAIN_EXAMPLES {
        let Some((line, outcome, reason)) = entries.get(example.corpus_id) else {
            anyhow::bail!(
                "M4a explain example {:?} is absent from {M4A_COMPARISON_CORPUS}",
                example.corpus_id
            );
        };
        if outcome != example.expected_outcome {
            anyhow::bail!(
                "M4a explain example {:?} has outcome {outcome:?}; expected {:?}",
                example.corpus_id,
                example.expected_outcome
            );
        }
        if reason.as_deref() != example.expected_reason {
            anyhow::bail!(
                "M4a explain example {:?} has reason {:?}; expected {:?}",
                example.corpus_id,
                reason,
                example.expected_reason
            );
        }
        resolved.push(ResolvedM4aExplainExample {
            example,
            line: *line,
        });
    }
    Ok(resolved)
}

/// Read a required string property from one JSON Lines comparison entry.
fn corpus_string<'a>(entry: &'a serde_json::Value, field: &str, line: usize) -> Result<&'a str> {
    entry
        .get(field)
        .and_then(serde_json::Value::as_str)
        .with_context(|| format!("M4a comparison corpus line {line} has no string {field:?}"))
}

/// Render a repository-relative Markdown link from an explain page to a corpus line.
fn corpus_link(line: usize) -> String {
    format!("../../{M4A_COMPARISON_CORPUS}#L{line}")
}

/// Read the flat generated reference directory and reject every unexpected entry.
fn read_reference_documents(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    let directory = root.join(EXPLAIN_DIRECTORY);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", directory.display()));
        }
    };

    let mut documents = BTreeMap::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {} entry", directory.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading type for {}", path.display()))?;
        if !file_type.is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("md")
        {
            documents.insert(
                Path::new(EXPLAIN_DIRECTORY).join(entry.file_name()),
                String::new(),
            );
            continue;
        }
        let relative = Path::new(EXPLAIN_DIRECTORY).join(entry.file_name());
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        documents.insert(relative, content);
    }
    Ok(documents)
}

/// Compare expected catalog pages and actual generated-reference directory.
fn document_problems(
    expected: &BTreeMap<PathBuf, String>,
    actual: &BTreeMap<PathBuf, String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    let expected_paths: BTreeSet<_> = expected.keys().collect();
    let actual_paths: BTreeSet<_> = actual.keys().collect();

    for path in expected_paths.difference(&actual_paths) {
        problems.push(format!("missing reference page {}", path.display()));
    }
    for path in actual_paths.difference(&expected_paths) {
        problems.push(format!("orphaned reference content {}", path.display()));
    }
    for path in expected_paths.intersection(&actual_paths) {
        let (Some(expected_content), Some(actual_content)) =
            (expected.get(*path), actual.get(*path))
        else {
            continue;
        };
        if expected_content != actual_content {
            problems.push(format!(
                "reference page {} differs from the shared explain catalog",
                path.display()
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_reference_covers_every_catalog_identifier_once() {
        let root = repository_root().expect("test runs inside the repository");
        let documents =
            expected_documents(&root).expect("verified corpus resolves explain examples");
        let identifiers: BTreeSet<_> = catalog().iter().map(|content| content.identifier).collect();
        assert_eq!(documents.len(), identifiers.len() + 1);
        for identifier in identifiers {
            let page = Path::new(EXPLAIN_DIRECTORY).join(format!("{identifier}.md"));
            let content = documents.get(&page).expect("catalog entry must have page");
            assert!(content.contains("## Meaning"));
            assert!(content.contains("## Limit"));
            assert!(content.contains("## Remedy"));
        }
        let index = documents
            .get(&Path::new(EXPLAIN_DIRECTORY).join(INDEX_PAGE))
            .expect("reference index must exist");
        for identifier in catalog().iter().map(|content| content.identifier) {
            assert!(index.contains(&format!("]({identifier}.md)")));
        }
    }

    #[test]
    fn verified_m4a_examples_have_current_corpus_links_and_reason_context() {
        let root = repository_root().expect("test runs inside the repository");
        let examples =
            resolve_m4a_explain_examples(&root).expect("verified corpus resolves explain examples");
        let documents = expected_documents(&root).expect("verified corpus renders explain pages");

        assert_eq!(examples.len(), M4A_EXPLAIN_EXAMPLES.len());
        for example in &examples {
            for identifier in example.example.explain_identifiers {
                let page = Path::new(EXPLAIN_DIRECTORY).join(format!("{identifier}.md"));
                let content = documents.get(&page).expect("mapped explain page exists");
                assert!(content.contains(&format!(
                    "[`{}`]({})",
                    example.example.corpus_id,
                    corpus_link(example.line)
                )));
                assert!(content.contains(&format!(
                    "verified `{}` verdict",
                    example.example.expected_outcome
                )));
            }
        }

        let missing_rewrite_page = documents
            .get(&Path::new(EXPLAIN_DIRECTORY).join("IND_MISSING_REWRITE.md"))
            .expect("missing rewrite page exists");
        assert!(missing_rewrite_page.contains("recoverable` reason records engineering backlog"));
        assert!(!missing_rewrite_page.contains("fundamental` reason records"));
    }

    #[test]
    fn verified_m4a_mapping_rejects_a_reason_page_that_did_not_produce_the_result() {
        let contents = catalog();

        let foreign_reason = [M4aExplainExample {
            corpus_id: "different-literal-filters-remain-indecisive",
            expected_outcome: "indecisive",
            expected_reason: Some("IND_MISSING_REWRITE"),
            explain_identifiers: &["PUR2002", "IND_MISSING_REWRITE", "IND_UNMODELED_OP"],
        }];
        let error = validate_m4a_example_identifiers(&contents, &foreign_reason)
            .expect_err("a reason page must not advertise another reason's result");
        assert!(error.to_string().contains("IND_UNMODELED_OP"));
        assert!(error.to_string().contains("IND_MISSING_REWRITE"));

        let unlinked_reason = [M4aExplainExample {
            corpus_id: "different-literal-filters-remain-indecisive",
            expected_outcome: "indecisive",
            expected_reason: Some("IND_MISSING_REWRITE"),
            explain_identifiers: &["PUR2002"],
        }];
        let error = validate_m4a_example_identifiers(&contents, &unlinked_reason)
            .expect_err("a verified reason must link its own page");
        assert!(error.to_string().contains("does not link its page"));

        validate_m4a_example_identifiers(&contents, M4A_EXPLAIN_EXAMPLES)
            .expect("the committed mapping stays consistent");
    }

    #[test]
    fn verified_m4a_mapping_rejects_contradictory_corpus_outcomes_and_reasons() {
        let root = repository_root().expect("test runs inside the repository");
        let corpus = std::fs::read_to_string(root.join(M4A_COMPARISON_CORPUS))
            .expect("read committed M4a comparison corpus");

        let contradictory_outcome = corpus.replacen(
            "\"outcome\":\"indecisive\"",
            "\"outcome\":\"equivalent\"",
            1,
        );
        let outcome_error = resolve_m4a_explain_examples_from_corpus(&contradictory_outcome)
            .expect_err("contradictory indecisive fixture must be rejected");
        assert!(
            outcome_error
                .to_string()
                .contains("different-literal-filters-remain-indecisive")
        );
        assert!(
            outcome_error
                .to_string()
                .contains("expected \"indecisive\"")
        );

        let contradictory_reason = corpus.replacen(
            "\"reason\":\"IND_MISSING_REWRITE\"",
            "\"reason\":\"IND_UNMODELED_OP\"",
            1,
        );
        let reason_error = resolve_m4a_explain_examples_from_corpus(&contradictory_reason)
            .expect_err("contradictory indecisive reason must be rejected");
        assert!(
            reason_error
                .to_string()
                .contains("different-literal-filters-remain-indecisive")
        );
        assert!(
            reason_error
                .to_string()
                .contains("expected Some(\"IND_MISSING_REWRITE\")")
        );
    }

    #[test]
    fn verifier_rejects_a_stale_m4a_verdict_example() {
        let root = repository_root().expect("test runs inside the repository");
        let expected = expected_documents(&root).expect("verified corpus renders explain pages");
        let mut actual = expected.clone();
        let verdict_page = Path::new(EXPLAIN_DIRECTORY).join("IND_MISSING_REWRITE.md");
        let stale = actual
            .get_mut(&verdict_page)
            .expect("missing-rewrite reason page exists");
        *stale = stale.replacen(
            "verified `indecisive` verdict",
            "verified `not_equivalent` verdict",
            1,
        );

        assert_eq!(
            document_problems(&expected, &actual),
            [
                "reference page docs/explain/IND_MISSING_REWRITE.md differs from the shared explain catalog"
            ]
        );
    }

    #[test]
    fn verifier_reports_missing_changed_and_orphaned_pages() {
        let expected = BTreeMap::from([
            (
                PathBuf::from("docs/explain/README.md"),
                "index\n".to_owned(),
            ),
            (
                PathBuf::from("docs/explain/PUR0101.md"),
                "page\n".to_owned(),
            ),
        ]);
        let actual = BTreeMap::from([
            (
                PathBuf::from("docs/explain/README.md"),
                "changed\n".to_owned(),
            ),
            (
                PathBuf::from("docs/explain/ORPHAN.md"),
                "orphan\n".to_owned(),
            ),
        ]);

        assert_eq!(
            document_problems(&expected, &actual),
            [
                "missing reference page docs/explain/PUR0101.md",
                "orphaned reference content docs/explain/ORPHAN.md",
                "reference page docs/explain/README.md differs from the shared explain catalog",
            ]
        );
    }
}
