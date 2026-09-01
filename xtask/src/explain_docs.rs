//! Generated user-facing documentation for registered explain identifiers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use pure_analyzer_diagnostics::{ALL_DIAG_CODES, ALL_REASON_CODES, ExplainContent, ExplainKind};

use crate::process::run_stdout;

/// Repository-relative directory containing the generated reference pages.
const EXPLAIN_DIRECTORY: &str = "docs/explain";
/// Name of the generated index in [`EXPLAIN_DIRECTORY`].
const INDEX_PAGE: &str = "README.md";

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
    for (relative_path, content) in expected_documents() {
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
    let expected = expected_documents();
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
fn expected_documents() -> BTreeMap<PathBuf, String> {
    let contents = catalog();
    let mut documents = BTreeMap::new();
    documents.insert(
        Path::new(EXPLAIN_DIRECTORY).join(INDEX_PAGE),
        render_index(&contents),
    );
    for content in contents {
        documents.insert(
            Path::new(EXPLAIN_DIRECTORY).join(format!("{}.md", content.identifier)),
            render_page(content),
        );
    }
    documents
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
fn render_page(content: &ExplainContent) -> String {
    format!(
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
    )
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
        let documents = expected_documents();
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
