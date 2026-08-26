//! Repository-local Markdown link validation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

use crate::process::run_stdout;

/// A link destination and its one-based source line.
#[derive(Debug, Clone, Eq, PartialEq)]
struct LinkTarget {
    line: usize,
    target: String,
}

/// Parsed link and heading data for one Markdown document.
#[derive(Debug, Clone, Eq, PartialEq)]
struct ParsedMarkdown {
    links: Vec<LinkTarget>,
    heading_ids: BTreeSet<String>,
}

/// A normalized repository-local link destination.
#[derive(Debug, Clone, Eq, PartialEq)]
struct ResolvedTarget {
    path: PathBuf,
    fragment: Option<String>,
}

/// Check every tracked Markdown file for missing relative files and anchors.
///
/// External URLs are outside this gate. Local destinations are resolved
/// lexically from their source document, must remain inside the repository,
/// and must name a tracked file or directory. Fragments on Markdown targets
/// must match a GitHub-style heading identifier.
///
/// # Errors
///
/// Returns an error when Git cannot identify the repository or enumerate
/// tracked files, a tracked Markdown document cannot be read, or one or more
/// local links are invalid.
pub fn check_doc_links() -> Result<()> {
    let root = repository_root()?;
    let tracked = tracked_paths(&root)?;
    let documents = markdown_documents(&root, &tracked)?;
    let problems = link_problems(&documents, &tracked);
    if !problems.is_empty() {
        anyhow::bail!(
            "broken repository-local Markdown links:\n{}",
            problems.join("\n")
        );
    }
    Ok(())
}

/// Resolve the Git repository root so the command works from any subdirectory.
fn repository_root() -> Result<PathBuf> {
    let output = run_stdout("git", &["rev-parse", "--show-toplevel"])?;
    let root = output.trim();
    if root.is_empty() {
        anyhow::bail!("`git rev-parse --show-toplevel` returned an empty path");
    }
    Ok(PathBuf::from(root))
}

/// Enumerate repository paths from the index rather than filesystem noise.
fn tracked_paths(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let root = root
        .to_str()
        .context("Git repository root is not valid UTF-8")?;
    let output = run_stdout("git", &["-C", root, "ls-files", "-z"])?;
    Ok(output
        .split_terminator('\0')
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Read every tracked Markdown document relative to the repository root.
fn markdown_documents(
    root: &Path,
    tracked: &BTreeSet<PathBuf>,
) -> Result<BTreeMap<PathBuf, String>> {
    let mut documents = BTreeMap::new();
    for path in tracked.iter().filter(|path| is_markdown_path(path)) {
        let text = std::fs::read_to_string(root.join(path))
            .with_context(|| format!("reading tracked Markdown file {}", path.display()))?;
        documents.insert(path.clone(), text);
    }
    Ok(documents)
}

/// Parse CommonMark links, reference definitions, and rendered heading text.
fn parse_markdown(markdown: &str) -> ParsedMarkdown {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let parser = Parser::new_ext(markdown, options);

    // Reference links are checked at their definitions below. This covers
    // unused definitions and multiline CommonMark definitions without
    // duplicating an error at each use site.
    let mut links: Vec<LinkTarget> = parser
        .reference_definitions()
        .iter()
        .map(|(_, definition)| LinkTarget {
            line: source_line(markdown, definition.span.start),
            target: definition.dest.to_string(),
        })
        .collect();

    let mut heading_ids = BTreeSet::new();
    let mut heading_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut heading_text: Option<String> = None;

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { .. }) => heading_text = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => {
                if let Some(text) = heading_text.take() {
                    reserve_heading_id(&mut heading_ids, &mut heading_counts, &text);
                }
            }
            Event::Start(
                Tag::Link {
                    link_type,
                    dest_url,
                    ..
                }
                | Tag::Image {
                    link_type,
                    dest_url,
                    ..
                },
            ) if should_check_event_target(link_type) => links.push(LinkTarget {
                line: source_line(markdown, range.start),
                target: dest_url.to_string(),
            }),
            Event::Text(text)
            | Event::Code(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text) => {
                if let Some(heading) = &mut heading_text {
                    heading.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(heading) = &mut heading_text {
                    heading.push(' ');
                }
            }
            _ => {}
        }
    }

    ParsedMarkdown { links, heading_ids }
}

/// Whether a parser link event carries a destination this gate should check.
fn should_check_event_target(link_type: LinkType) -> bool {
    !matches!(
        link_type,
        LinkType::Reference | LinkType::Collapsed | LinkType::Shortcut | LinkType::Email
    )
}

/// Convert a byte offset into a one-based source line.
fn source_line(markdown: &str, offset: usize) -> usize {
    markdown
        .as_bytes()
        .iter()
        .take(offset.min(markdown.len()))
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

/// Resolve every parsed local link and aggregate deterministic failures.
fn link_problems(
    documents: &BTreeMap<PathBuf, String>,
    tracked: &BTreeSet<PathBuf>,
) -> Vec<String> {
    let directories = tracked_directories(tracked);
    let parsed: BTreeMap<PathBuf, ParsedMarkdown> = documents
        .iter()
        .map(|(path, text)| (path.clone(), parse_markdown(text)))
        .collect();
    let mut problems: Vec<(PathBuf, usize, String)> = Vec::new();

    for (source, document) in &parsed {
        for link in &document.links {
            let resolved = match normalize_relative_target(source, &link.target) {
                Ok(Some(resolved)) => resolved,
                Ok(None) => continue,
                Err(reason) => {
                    problems.push((
                        source.clone(),
                        link.line,
                        format!("invalid link `{}`: {reason}", link.target),
                    ));
                    continue;
                }
            };

            if !tracked.contains(&resolved.path) && !directories.contains(&resolved.path) {
                problems.push((
                    source.clone(),
                    link.line,
                    format!(
                        "link `{}` resolves to missing `{}`",
                        link.target,
                        resolved.path.display()
                    ),
                ));
                continue;
            }

            let Some(fragment) = resolved
                .fragment
                .as_deref()
                .filter(|fragment| !fragment.is_empty())
            else {
                continue;
            };
            if !is_markdown_path(&resolved.path) {
                continue;
            }
            let present = parsed
                .get(&resolved.path)
                .is_some_and(|target| target.heading_ids.contains(fragment));
            if !present {
                problems.push((
                    source.clone(),
                    link.line,
                    format!(
                        "link `{}` has missing anchor `#{fragment}` in `{}`",
                        link.target,
                        resolved.path.display()
                    ),
                ));
            }
        }
    }

    problems.sort();
    problems
        .into_iter()
        .map(|(source, line, problem)| format!("{}:{line}: {problem}", source.display()))
        .collect()
}

/// Derive every tracked directory from the tracked file set.
fn tracked_directories(tracked: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
    let mut directories = BTreeSet::from([PathBuf::new()]);
    for path in tracked {
        let mut parent = path.parent();
        while let Some(directory) = parent {
            directories.insert(directory.to_path_buf());
            if directory.as_os_str().is_empty() {
                break;
            }
            parent = directory.parent();
        }
    }
    directories
}

/// Normalize one local destination or classify it as external/ignored.
fn normalize_relative_target(
    source: &Path,
    raw_target: &str,
) -> std::result::Result<Option<ResolvedTarget>, String> {
    let unescaped = markdown_unescape(raw_target);
    if unescaped.starts_with('/') || has_uri_scheme(&unescaped) {
        return Ok(None);
    }

    let (path_and_query, fragment) = unescaped
        .split_once('#')
        .map_or((unescaped.as_str(), None), |(path, fragment)| {
            (path, Some(fragment))
        });
    let path = path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path);
    let decoded_path = percent_decode(path)?;
    if decoded_path.starts_with('/') {
        return Ok(None);
    }
    let decoded_fragment = fragment.map(percent_decode).transpose()?;

    let resolved = if decoded_path.is_empty() {
        source.to_path_buf()
    } else {
        let base = source.parent().unwrap_or_else(|| Path::new(""));
        lexical_join(base, Path::new(&decoded_path))?
    };
    Ok(Some(ResolvedTarget {
        path: resolved,
        fragment: decoded_fragment,
    }))
}

/// Join and normalize `.`/`..` without filesystem canonicalization.
fn lexical_join(base: &Path, relative: &Path) -> std::result::Result<PathBuf, String> {
    let mut normalized = PathBuf::new();
    for component in base.components().chain(relative.components()) {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("destination escapes the repository".to_string());
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute local destinations are not checked".to_string());
            }
        }
    }
    Ok(normalized)
}

/// Remove CommonMark backslash escapes from a destination.
fn markdown_unescape(target: &str) -> String {
    let mut output = String::with_capacity(target.len());
    let mut characters = target.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            if let Some(escaped) = characters.next() {
                output.push(escaped);
            }
        } else {
            output.push(character);
        }
    }
    output
}

/// Decode percent-encoded UTF-8 without treating `+` as a space.
fn percent_decode(value: &str) -> std::result::Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(format!("incomplete percent escape in `{value}`"));
            }
            let high = hex_value(bytes[index + 1]);
            let low = hex_value(bytes[index + 2]);
            let (Some(high), Some(low)) = (high, low) else {
                return Err(format!("invalid percent escape in `{value}`"));
            };
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| format!("percent escape in `{value}` is not UTF-8"))
}

/// Convert one ASCII hex digit to its value.
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Detect RFC-style URI schemes so external destinations stay out of scope.
fn has_uri_scheme(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

/// Reserve a globally unique GitHub-style identifier for one rendered heading.
fn reserve_heading_id(
    ids: &mut BTreeSet<String>,
    counts: &mut BTreeMap<String, usize>,
    heading: &str,
) {
    let base = github_slug(heading);
    if base.is_empty() {
        return;
    }

    let count = counts.entry(base.clone()).or_insert(0);
    let mut id = if *count == 0 {
        base.clone()
    } else {
        format!("{base}-{count}")
    };
    while ids.contains(&id) {
        *count += 1;
        id = format!("{base}-{count}");
    }
    *count += 1;
    ids.insert(id);
}

/// Approximate GitHub's heading slugger over CommonMark-rendered text.
fn github_slug(heading: &str) -> String {
    let mut slug = String::new();
    for character in heading.chars() {
        match character {
            character if character.is_alphanumeric() || matches!(character, '_' | '-') => {
                slug.extend(character.to_lowercase());
            }
            character if character.is_whitespace() => slug.push('-'),
            _ => {}
        }
    }
    slug
}

/// Whether a repository path is a Markdown document checked for anchors.
fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(
        documents: &[(&str, &str)],
        other_paths: &[&str],
    ) -> (BTreeMap<PathBuf, String>, BTreeSet<PathBuf>) {
        let documents: BTreeMap<PathBuf, String> = documents
            .iter()
            .map(|(path, text)| (PathBuf::from(path), (*text).to_string()))
            .collect();
        let mut tracked: BTreeSet<PathBuf> = documents.keys().cloned().collect();
        tracked.extend(other_paths.iter().map(PathBuf::from));
        (documents, tracked)
    }

    #[test]
    fn valid_sibling_parent_and_directory_links_pass() {
        let (documents, tracked) = fixture(
            &[
                (
                    "docs/guide/start.md",
                    "[sibling](next.md) [parent](../README.md) [dir](../../assets/)",
                ),
                ("docs/guide/next.md", "# Next"),
                ("docs/README.md", "# Docs"),
            ],
            &["assets/logo.svg"],
        );
        assert!(link_problems(&documents, &tracked).is_empty());
    }

    #[test]
    fn missing_file_and_repository_escape_are_reported() {
        let (documents, tracked) = fixture(
            &[(
                "docs/guide.md",
                "[missing](gone.md)\n[escape](../../outside.md)",
            )],
            &[],
        );
        let problems = link_problems(&documents, &tracked);
        assert_eq!(problems.len(), 2);
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("missing `docs/gone.md`"))
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("escapes the repository"))
        );
    }

    #[test]
    fn same_file_cross_file_and_duplicate_anchors_pass() {
        let (documents, tracked) = fixture(
            &[
                (
                    "README.md",
                    "# Intro\n## Repeat\n## Repeat\n[same](#intro) [duplicate](#repeat-1) [other](docs/a.md#target)",
                ),
                ("docs/a.md", "# Target"),
            ],
            &[],
        );
        assert!(link_problems(&documents, &tracked).is_empty());
    }

    #[test]
    fn duplicate_anchor_suffixes_skip_all_global_collisions() {
        let first = parse_markdown("# Foo\n# Foo-1\n# Foo");
        assert_eq!(
            first.heading_ids,
            BTreeSet::from(["foo".to_string(), "foo-1".to_string(), "foo-2".to_string(),])
        );

        let second = parse_markdown("# Foo\n# Foo\n# Foo-1");
        assert_eq!(
            second.heading_ids,
            BTreeSet::from([
                "foo".to_string(),
                "foo-1".to_string(),
                "foo-1-1".to_string(),
            ])
        );
    }

    #[test]
    fn missing_anchor_is_reported() {
        let (documents, tracked) = fixture(&[("README.md", "# Present\n[bad](#absent)")], &[]);
        let problems = link_problems(&documents, &tracked);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("missing anchor `#absent`"));
    }

    #[test]
    fn images_angle_paths_titles_and_reference_definitions_are_checked() {
        let (documents, tracked) = fixture(
            &[(
                "docs/a.md",
                "![logo](../assets/logo.svg)\n[space](<folder/a file.md>)\n[title](b.md \"B\")\n[ref]: c.md 'C'",
            )],
            &[
                "assets/logo.svg",
                "docs/folder/a file.md",
                "docs/b.md",
                "docs/c.md",
            ],
        );
        assert!(link_problems(&documents, &tracked).is_empty());
    }

    #[test]
    fn multiline_reference_definitions_are_checked() {
        let (documents, tracked) =
            fixture(&[("README.md", "[used][ref]\n\n[ref]:\n  missing.md")], &[]);
        let problems = link_problems(&documents, &tracked);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("missing `missing.md`"));
    }

    #[test]
    fn percent_encoded_paths_and_fragments_are_decoded() {
        let (documents, tracked) = fixture(
            &[
                ("README.md", "[encoded](docs/a%20file.md#hello%2Dworld)"),
                ("docs/a file.md", "# Hello world"),
            ],
            &[],
        );
        assert!(link_problems(&documents, &tracked).is_empty());
    }

    #[test]
    fn external_urls_are_ignored() {
        let (documents, tracked) = fixture(
            &[(
                "README.md",
                "[web](https://example.com/missing) [mail](mailto:a@example.com) bare@example.com [data](data:text/plain,x) [root](/site/path)",
            )],
            &[],
        );
        assert!(link_problems(&documents, &tracked).is_empty());
    }

    #[test]
    fn only_commonmark_links_are_checked() {
        let markdown = "literal](missing.md)\n\\[escaped](missing.md)\n[unterminated](missing.md\n\n    [code](missing.md)\n`unmatched [real](real.md)";
        assert_eq!(
            parse_markdown(markdown).links,
            [LinkTarget {
                line: 6,
                target: "real.md".to_string(),
            }]
        );
    }

    #[test]
    fn commonmark_heading_events_drive_anchor_text() {
        let (documents, tracked) = fixture(
            &[(
                "README.md",
                "    # Not a heading\n# [Guide](https://example.com/a_(b)) suffix\n[good](#guide-suffix)\n[bad](#not-a-heading)",
            )],
            &[],
        );
        let problems = link_problems(&documents, &tracked);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("missing anchor `#not-a-heading`"));
    }

    #[test]
    fn code_spans_and_blocks_do_not_create_links_or_headings() {
        let markdown = "`[inline](missing.md)`\n```md\n[fenced](missing.md)\n# Hidden\n```\n# Real";
        let parsed = parse_markdown(markdown);
        assert!(parsed.links.is_empty());
        assert_eq!(parsed.heading_ids, BTreeSet::from(["real".to_string()]));
    }

    #[test]
    fn standard_markdown_extension_is_supported() {
        assert!(is_markdown_path(Path::new("guide.md")));
        assert!(is_markdown_path(Path::new("guide.markdown")));
        assert!(!is_markdown_path(Path::new("guide.txt")));
    }

    #[test]
    fn percent_decoder_rejects_bad_sequences() {
        assert!(percent_decode("%").is_err());
        assert!(percent_decode("%GG").is_err());
        assert!(percent_decode("%FF").is_err());
    }

    #[test]
    fn failures_are_sorted_by_source_then_line() {
        let (documents, tracked) = fixture(
            &[
                ("z.md", "[later](b.md)\n[first](c.md)"),
                ("a.md", "[only](missing.md)"),
            ],
            &[],
        );
        let problems = link_problems(&documents, &tracked);
        assert_eq!(
            problems,
            [
                "a.md:1: link `missing.md` resolves to missing `missing.md`",
                "z.md:1: link `b.md` resolves to missing `b.md`",
                "z.md:2: link `c.md` resolves to missing `c.md`",
            ]
        );
    }
}
