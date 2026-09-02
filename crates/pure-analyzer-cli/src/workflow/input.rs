//! Deterministic expansion of CLI source and model inputs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use libpure::{ModelInput, SourceInput};

use super::Failure;

/// Resolve query arguments, expanding globs and snapshotting standard input once.
pub(super) fn query_sources(arguments: &[String]) -> Result<Vec<SourceInput>, Failure> {
    if arguments.is_empty() {
        return Err(Failure::usage(
            "at least one input file, glob, or - is required",
        ));
    }

    let cwd = std::env::current_dir().map_err(|error| {
        Failure::usage(format!("could not determine current directory: {error}"))
    })?;
    let mut seen = BTreeSet::new();
    let mut sources = Vec::new();
    let mut read_stdin = false;
    for argument in arguments {
        if argument == "-" {
            if read_stdin {
                return Err(Failure::usage("standard input may be supplied only once"));
            }
            let text = std::io::read_to_string(std::io::stdin()).map_err(|error| {
                Failure::usage(format!("could not read standard input: {error}"))
            })?;
            sources.push(SourceInput::stdin(text));
            read_stdin = true;
            continue;
        }
        for path in expand_argument(argument, &cwd)? {
            let key = path.canonicalize().map_err(|error| {
                Failure::usage(format!(
                    "could not resolve input {}: {error}",
                    path.display()
                ))
            })?;
            if seen.insert(key) {
                sources.push(SourceInput::file(path));
            }
        }
    }
    if sources.is_empty() {
        return Err(Failure::usage("input patterns matched no files"));
    }
    Ok(sources)
}

/// Resolve exactly two comparison operands without collapsing identical paths.
///
/// Each operand may name one file, one matching glob, or standard input. Unlike
/// [`query_sources`], the two snapshots are positional: comparing a source to
/// itself is valid, so duplicate paths must remain distinct. Standard input is
/// a single stream and therefore cannot supply both operands.
pub(super) fn comparison_sources(left: &str, right: &str) -> Result<[SourceInput; 2], Failure> {
    if left == "-" && right == "-" {
        return Err(Failure::usage(
            "comparison accepts standard input for at most one operand",
        ));
    }
    let cwd = std::env::current_dir().map_err(|error| {
        Failure::usage(format!("could not determine current directory: {error}"))
    })?;
    let left = comparison_source(left, &cwd)?;
    let right = comparison_source(right, &cwd)?;
    Ok([left, right])
}

fn comparison_source(argument: &str, cwd: &Path) -> Result<SourceInput, Failure> {
    if argument == "-" {
        let text = std::io::read_to_string(std::io::stdin())
            .map_err(|error| Failure::usage(format!("could not read standard input: {error}")))?;
        return Ok(SourceInput::stdin(text));
    }

    let paths = expand_argument(argument, cwd)?;
    let [path] = paths.as_slice() else {
        return Err(Failure::usage(format!(
            "comparison input {argument:?} must resolve to exactly one file"
        )));
    };
    Ok(SourceInput::file(path.clone()))
}

/// Convert resolved configuration model paths to typed libpure inputs.
pub(super) fn model_sources(paths: &[PathBuf]) -> Result<Vec<ModelInput>, Failure> {
    let mut models = Vec::with_capacity(paths.len());
    let mut seen = BTreeSet::new();
    for path in paths {
        let key = path.canonicalize().map_err(|error| {
            Failure::model(format!(
                "could not resolve model {}: {error}",
                path.display()
            ))
        })?;
        if !seen.insert(key) {
            continue;
        }
        let source = SourceInput::file(path);
        match path.extension().and_then(|extension| extension.to_str()) {
            Some(extension) if extension.eq_ignore_ascii_case("json") => {
                models.push(ModelInput::pmcd(source));
            }
            Some(extension) if extension.eq_ignore_ascii_case("pure") => {
                models.push(ModelInput::pure(source));
            }
            _ => {
                return Err(Failure::model(format!(
                    "model {} must use a .json or .pure extension",
                    path.display()
                )));
            }
        }
    }
    Ok(models)
}

fn expand_argument(argument: &str, cwd: &Path) -> Result<Vec<PathBuf>, Failure> {
    let path = PathBuf::from(argument);
    if path.is_file() || !contains_glob_meta(argument) {
        let metadata = std::fs::metadata(&path).map_err(|error| {
            Failure::usage(format!(
                "could not access input {}: {error}",
                path.display()
            ))
        })?;
        if !metadata.is_file() {
            return Err(Failure::usage(format!(
                "input {} is not a regular file",
                path.display()
            )));
        }
        return Ok(vec![path]);
    }

    let pattern = rooted_pattern(argument, cwd)?;
    validate_pattern(&pattern, argument)?;
    let root = traversal_root(&pattern, cwd);
    let mut candidates = Vec::new();
    collect_files(&root, argument, &mut candidates)?;
    let mut matches = candidates
        .into_iter()
        .filter_map(|path| {
            let display = display_path(path, cwd);
            let text = display.to_str()?;
            path_matches(&pattern, &normalize_separators(text)).then_some(display)
        })
        .collect::<Vec<_>>();
    matches.sort();
    if matches.is_empty() {
        return Err(Failure::usage(format!(
            "input pattern {argument:?} matched no files"
        )));
    }
    Ok(matches)
}

fn rooted_pattern(argument: &str, cwd: &Path) -> Result<String, Failure> {
    let path = Path::new(argument);
    if path.is_absolute() {
        let relative = path.strip_prefix(cwd).map_err(|_| {
            Failure::usage(format!(
                "absolute input pattern {argument:?} must be inside {}",
                cwd.display()
            ))
        })?;
        let pattern = relative.to_str().ok_or_else(|| {
            Failure::usage(format!("input pattern is not valid UTF-8: {argument:?}"))
        })?;
        Ok(normalize_separators(pattern))
    } else {
        Ok(normalize_separators(argument))
    }
}

fn traversal_root(pattern: &str, cwd: &Path) -> PathBuf {
    let mut root = cwd.to_path_buf();
    for component in pattern.split('/') {
        if contains_glob_meta(component) {
            break;
        }
        root.push(component);
    }
    root
}

fn collect_files(
    directory: &Path,
    argument: &str,
    files: &mut Vec<PathBuf>,
) -> Result<(), Failure> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(Failure::usage(format!(
                "could not expand input pattern {argument:?} below {}: {error}",
                directory.display()
            )));
        }
    };
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| Failure::usage(format!("could not read input candidates: {error}")))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let kind = entry.file_type().map_err(|error| {
            Failure::usage(format!(
                "could not inspect input candidate {}: {error}",
                entry.path().display()
            ))
        })?;
        if kind.is_dir() {
            collect_files(&entry.path(), argument, files)?;
        } else if kind.is_file() || kind.is_symlink() {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn validate_pattern(pattern: &str, argument: &str) -> Result<(), Failure> {
    for component in pattern.split('/') {
        if component == ".." {
            return Err(Failure::usage(format!(
                "input pattern {argument:?} must not traverse above the working directory"
            )));
        }
        let characters = component.chars().collect::<Vec<_>>();
        let mut index = 0;
        while index < characters.len() {
            if characters[index] != '[' {
                index += 1;
                continue;
            }
            let Some(end) = characters[index + 1..]
                .iter()
                .position(|character| *character == ']')
                .map(|offset| index + offset + 1)
            else {
                return Err(Failure::usage(format!(
                    "invalid input pattern {argument:?}: unclosed character class"
                )));
            };
            if end == index + 1 || (end == index + 2 && matches!(characters[index + 1], '!' | '^'))
            {
                return Err(Failure::usage(format!(
                    "invalid input pattern {argument:?}: empty character class"
                )));
            }
            index = end + 1;
        }
    }
    Ok(())
}

fn path_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.split('/').collect::<Vec<_>>();
    let path = path.split('/').collect::<Vec<_>>();
    match_components(&pattern, &path, 0, 0, &mut BTreeMap::new())
}

fn match_components(
    pattern: &[&str],
    path: &[&str],
    pattern_index: usize,
    path_index: usize,
    memo: &mut BTreeMap<(usize, usize), bool>,
) -> bool {
    if let Some(result) = memo.get(&(pattern_index, path_index)) {
        return *result;
    }
    let result = if pattern_index == pattern.len() {
        path_index == path.len()
    } else if pattern[pattern_index] == "**" {
        match_components(pattern, path, pattern_index + 1, path_index, memo)
            || (path_index < path.len()
                && match_components(pattern, path, pattern_index, path_index + 1, memo))
    } else {
        path_index < path.len()
            && component_matches(pattern[pattern_index], path[path_index])
            && match_components(pattern, path, pattern_index + 1, path_index + 1, memo)
    };
    memo.insert((pattern_index, path_index), result);
    result
}

fn component_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let text = text.chars().collect::<Vec<_>>();
    match_characters(&pattern, &text, 0, 0, &mut BTreeMap::new())
}

fn match_characters(
    pattern: &[char],
    text: &[char],
    pattern_index: usize,
    text_index: usize,
    memo: &mut BTreeMap<(usize, usize), bool>,
) -> bool {
    if let Some(result) = memo.get(&(pattern_index, text_index)) {
        return *result;
    }
    let result = match pattern.get(pattern_index) {
        None => text_index == text.len(),
        Some('*') => {
            match_characters(pattern, text, pattern_index + 1, text_index, memo)
                || (text_index < text.len()
                    && match_characters(pattern, text, pattern_index, text_index + 1, memo))
        }
        Some('?') => {
            text_index < text.len()
                && match_characters(pattern, text, pattern_index + 1, text_index + 1, memo)
        }
        Some('[') => text.get(text_index).is_some_and(|character| {
            let (class_match, next_pattern) = character_class(pattern, pattern_index, *character);
            class_match && match_characters(pattern, text, next_pattern, text_index + 1, memo)
        }),
        Some(expected) => {
            text.get(text_index) == Some(expected)
                && match_characters(pattern, text, pattern_index + 1, text_index + 1, memo)
        }
    };
    memo.insert((pattern_index, text_index), result);
    result
}

fn character_class(pattern: &[char], start: usize, character: char) -> (bool, usize) {
    let end = pattern[start + 1..]
        .iter()
        .position(|candidate| *candidate == ']')
        .map_or(pattern.len(), |offset| start + offset + 1);
    let mut index = start + 1;
    let negated = pattern
        .get(index)
        .is_some_and(|candidate| matches!(candidate, '!' | '^'));
    if negated {
        index += 1;
    }
    let mut matched = false;
    while index < end {
        if index + 2 < end && pattern[index + 1] == '-' {
            matched |= pattern[index] <= character && character <= pattern[index + 2];
            index += 3;
        } else {
            matched |= pattern[index] == character;
            index += 1;
        }
    }
    (matched != negated, end.saturating_add(1))
}

fn display_path(path: PathBuf, cwd: &Path) -> PathBuf {
    path.strip_prefix(cwd).map_or_else(
        |_| path.clone(),
        |relative| {
            if relative.as_os_str().is_empty() {
                path.clone()
            } else {
                relative.to_path_buf()
            }
        },
    )
}

fn contains_glob_meta(argument: &str) -> bool {
    argument
        .bytes()
        .any(|byte| matches!(byte, b'*' | b'?' | b'['))
}

fn normalize_separators(pattern: &str) -> String {
    pattern.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_supported_glob_metacharacters() {
        assert!(contains_glob_meta("queries/*.pure"));
        assert!(contains_glob_meta("queries/q?.pure"));
        assert!(contains_glob_meta("queries/[ab].pure"));
        assert!(!contains_glob_meta("queries/query.pure"));
    }

    #[test]
    fn matches_component_classes_and_recursive_directories() {
        assert!(path_matches("queries/*.pure", "queries/a.pure"));
        assert!(!path_matches("queries/*.pure", "queries/nested/a.pure"));
        assert!(path_matches("queries/**/*.pure", "queries/nested/a.pure"));
        assert!(path_matches("queries/[ab]?.pure", "queries/a1.pure"));
        assert!(!path_matches("queries/[!a]*.pure", "queries/a1.pure"));
        assert!(path_matches("queries/[!a]*.pure", "queries/b1.pure"));
    }

    #[test]
    fn classifies_model_extensions_case_insensitively() {
        let root = std::env::temp_dir().join(format!(
            "pure-analyzer-model-inputs-{}-{}",
            std::process::id(),
            super::super::test_nonce()
        ));
        std::fs::create_dir_all(&root).expect("create fixture directory");
        let json = root.join("model.JSON");
        let pure = root.join("model.PURE");
        std::fs::write(&json, "{}").expect("write JSON fixture");
        std::fs::write(&pure, "Class model::A {}").expect("write Pure fixture");

        let models = model_sources(&[json, pure]).expect("classify model inputs");
        assert!(matches!(models[0], ModelInput::Pmcd { .. }));
        assert!(matches!(models[1], ModelInput::Pure { .. }));
        std::fs::remove_dir_all(root).expect("remove fixture directory");
    }
}
