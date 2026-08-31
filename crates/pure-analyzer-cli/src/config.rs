//! Deterministic CLI configuration discovery, layering, and policy compilation.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::{ArgAction, Args, ValueEnum};
use libpure::{ALL_DIAG_CODES, DiagCode, DiagnosticPolicy, Severity};
use serde::Deserialize;
use thiserror::Error;

const CONFIG_VERSION: u32 = 1;
const REPOSITORY_CONFIG_NAME: &str = ".pure-analyzer.toml";
const USER_CONFIG_DIRECTORY: &str = "pure-analyzer";
const USER_CONFIG_NAME: &str = "config.toml";
const ENV_PREFIX: &str = "PURE_ANALYZER_";
const DEFAULT_JOBS: usize = 1;
const DEFAULT_LINE_WIDTH: usize = 100;

const ENV_JOBS: &str = "PURE_ANALYZER_JOBS";
const ENV_FORMAT: &str = "PURE_ANALYZER_FORMAT";
const ENV_COLOR: &str = "PURE_ANALYZER_COLOR";
const ENV_QUIET: &str = "PURE_ANALYZER_QUIET";
const ENV_SELECT: &str = "PURE_ANALYZER_SELECT";
const ENV_IGNORE: &str = "PURE_ANALYZER_IGNORE";
const ENV_DENY: &str = "PURE_ANALYZER_DENY";
const ENV_WARN: &str = "PURE_ANALYZER_WARN";
const ENV_STRICT: &str = "PURE_ANALYZER_STRICT";
const ENV_LINE_WIDTH: &str = "PURE_ANALYZER_FMT_LINE_WIDTH";
const ENV_MODEL_PATHS: &str = "PURE_ANALYZER_MODEL_PATHS";
const KNOWN_ENVIRONMENT: &[&str] = &[
    ENV_JOBS,
    ENV_FORMAT,
    ENV_COLOR,
    ENV_QUIET,
    ENV_SELECT,
    ENV_IGNORE,
    ENV_DENY,
    ENV_WARN,
    ENV_STRICT,
    ENV_LINE_WIDTH,
    ENV_MODEL_PATHS,
];

/// Global configuration and diagnostic-policy flags.
#[derive(Debug, Args)]
pub(crate) struct ConfigFlags {
    /// Read this configuration file instead of repository discovery.
    ///
    /// User configuration still applies below this layer; environment variables
    /// and command-line flags retain higher precedence.
    #[arg(long, global = true, conflicts_with = "no_config")]
    config: Option<PathBuf>,
    /// Disable user and repository configuration files.
    #[arg(long, global = true)]
    no_config: bool,
    /// Print the fully resolved configuration and exit.
    #[arg(long, global = true)]
    print_config: bool,
    /// Maximum number of source files analyzed concurrently.
    #[arg(long, global = true)]
    jobs: Option<usize>,
    /// Diagnostic output format.
    #[arg(long = "format", global = true, value_enum)]
    output_format: Option<OutputFormat>,
    /// Color policy for human output.
    #[arg(long, global = true, value_enum)]
    color: Option<ColorChoice>,
    /// Suppress normal diagnostic output without changing the exit result.
    #[arg(long, global = true, conflicts_with = "no_quiet")]
    quiet: bool,
    /// Override configured quiet mode.
    #[arg(long, global = true)]
    no_quiet: bool,
    /// Retain diagnostics matching these registered-code patterns.
    #[arg(long, global = true, value_delimiter = ',', action = ArgAction::Append)]
    select: Vec<String>,
    /// Suppress diagnostics matching these registered-code patterns.
    #[arg(long, global = true, value_delimiter = ',', action = ArgAction::Append)]
    ignore: Vec<String>,
    /// Promote diagnostics matching these registered-code patterns to errors.
    #[arg(long, global = true, value_delimiter = ',', action = ArgAction::Append)]
    deny: Vec<String>,
    /// Reclassify diagnostics matching these registered-code patterns as warnings.
    #[arg(long, global = true, value_delimiter = ',', action = ArgAction::Append)]
    warn: Vec<String>,
}

impl ConfigFlags {
    /// Return whether the invocation only requests resolved configuration.
    pub(crate) const fn print_requested(&self) -> bool {
        self.print_config
    }

    /// Build the invocation-specific override layer.
    pub(crate) fn overrides(
        &self,
        strict: Option<bool>,
        line_width: Option<usize>,
        model_paths: Vec<PathBuf>,
    ) -> ConfigOverrides {
        ConfigOverrides {
            jobs: self.jobs,
            output_format: self.output_format,
            color: self.color,
            quiet: self
                .quiet
                .then_some(true)
                .or(self.no_quiet.then_some(false)),
            select: nonempty(self.select.clone()),
            ignore: nonempty(self.ignore.clone()),
            deny: nonempty(self.deny.clone()),
            warn: nonempty(self.warn.clone()),
            strict,
            line_width,
            model_paths: nonempty(model_paths),
        }
    }
}

/// Fully resolved output format selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OutputFormat {
    /// Human-readable labeled diagnostics.
    Human,
    /// Versioned machine-readable JSON.
    Json,
    /// SARIF 2.1.0 output.
    Sarif,
}

impl OutputFormat {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Json => "json",
            Self::Sarif => "sarif",
        }
    }
}

/// Fully resolved terminal color policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ColorChoice {
    /// Detect terminal color support at the renderer boundary.
    Auto,
    /// Always emit color escapes.
    Always,
    /// Never emit color escapes.
    Never,
}

impl ColorChoice {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

/// One CLI-only override layer applied after files and environment.
#[derive(Debug, Clone, Default)]
pub(crate) struct ConfigOverrides {
    jobs: Option<usize>,
    output_format: Option<OutputFormat>,
    color: Option<ColorChoice>,
    quiet: Option<bool>,
    select: Option<Vec<String>>,
    ignore: Option<Vec<String>>,
    deny: Option<Vec<String>>,
    warn: Option<Vec<String>>,
    strict: Option<bool>,
    line_width: Option<usize>,
    model_paths: Option<Vec<PathBuf>>,
}

impl ConfigOverrides {
    fn into_layer(self, base: &Path) -> ConfigLayer {
        ConfigLayer {
            jobs: self.jobs,
            output_format: self.output_format,
            color: self.color,
            quiet: self.quiet,
            select: self.select,
            ignore: self.ignore,
            deny: self.deny,
            warn: self.warn,
            strict: self.strict,
            line_width: self.line_width,
            model_paths: self.model_paths.map(|paths| resolve_paths(paths, base)),
        }
    }
}

/// Deterministic resolver whose filesystem and environment roots are explicit.
#[derive(Debug, Clone)]
pub(crate) struct ConfigResolver {
    cwd: PathBuf,
    user_config: Option<PathBuf>,
    environment: BTreeMap<String, String>,
}

impl ConfigResolver {
    /// Capture the process configuration inputs once at the CLI boundary.
    pub(crate) fn from_process() -> Result<Self, ConfigError> {
        let cwd = std::env::current_dir().map_err(ConfigError::CurrentDirectory)?;
        let user_config = user_config_path();
        let mut environment = BTreeMap::new();
        for (name, value) in std::env::vars_os() {
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(ENV_PREFIX) {
                continue;
            }
            let value = value
                .into_string()
                .map_err(|value| ConfigError::NonUtf8Environment {
                    name: name.to_owned(),
                    value,
                })?;
            environment.insert(name.to_owned(), value);
        }
        Ok(Self {
            cwd,
            user_config,
            environment,
        })
    }

    /// Resolve defaults, files, environment, and flags in documented order.
    pub(crate) fn resolve(
        &self,
        flags: &ConfigFlags,
        overrides: ConfigOverrides,
    ) -> Result<ResolvedConfig, ConfigError> {
        let mut resolved = ResolvedConfig::default();
        if !flags.no_config {
            if let Some(path) = &self.user_config
                && path.is_file()
            {
                resolved.apply(read_config(path)?)?;
            }
            let project_path = if let Some(path) = &flags.config {
                Some(resolve_path(path, &self.cwd))
            } else {
                discover_repository_config(&self.cwd)
            };
            if let Some(path) = project_path {
                resolved.apply(read_config(&path)?)?;
            }
        }
        resolved.apply(environment_layer(&self.environment, &self.cwd)?)?;
        resolved.apply(overrides.into_layer(&self.cwd))?;
        resolved.validate_policy()?;
        Ok(resolved)
    }

    #[cfg(test)]
    fn new(
        cwd: impl Into<PathBuf>,
        user_config: Option<PathBuf>,
        environment: BTreeMap<String, String>,
    ) -> Self {
        Self {
            cwd: cwd.into(),
            user_config,
            environment,
        }
    }
}

/// One completely resolved configuration suitable for printing or execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedConfig {
    version: u32,
    jobs: usize,
    output: ResolvedOutput,
    lint: ResolvedLint,
    validate: ResolvedValidate,
    fmt: ResolvedFormat,
    model: ResolvedModel,
}

impl Default for ResolvedConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            jobs: DEFAULT_JOBS,
            output: ResolvedOutput {
                format: OutputFormat::Human,
                color: ColorChoice::Auto,
                quiet: false,
            },
            lint: ResolvedLint::default(),
            validate: ResolvedValidate { strict: false },
            fmt: ResolvedFormat {
                line_width: DEFAULT_LINE_WIDTH,
            },
            model: ResolvedModel::default(),
        }
    }
}

impl ResolvedConfig {
    /// Return the maximum configured analysis concurrency.
    pub(crate) const fn jobs(&self) -> usize {
        self.jobs
    }

    /// Return the renderer selected for diagnostic output.
    pub(crate) const fn output_format(&self) -> OutputFormat {
        self.output.format
    }

    /// Return the human-renderer color policy.
    pub(crate) const fn color(&self) -> ColorChoice {
        self.output.color
    }

    /// Return whether normal diagnostic output is suppressed.
    pub(crate) const fn quiet(&self) -> bool {
        self.output.quiet
    }

    /// Return whether validation promotes otherwise-warning findings.
    pub(crate) const fn validate_strict(&self) -> bool {
        self.validate.strict
    }

    /// Return the preferred layout-formatting line width.
    pub(crate) const fn line_width(&self) -> usize {
        self.fmt.line_width
    }

    /// Return resolved model paths in deterministic loading order.
    pub(crate) fn model_paths(&self) -> &[PathBuf] {
        &self.model.paths
    }

    /// Serialize the resolved configuration with stable field and set ordering.
    pub(crate) fn to_toml(&self) -> Result<String, ConfigError> {
        let select = toml_array(self.lint.select.iter().map(String::as_str))?;
        let ignore = toml_array(self.lint.ignore.iter().map(String::as_str))?;
        let deny = toml_array(self.lint.deny.iter().map(String::as_str))?;
        let warn = toml_array(self.lint.warn.iter().map(String::as_str))?;
        let model_paths = self
            .model
            .paths
            .iter()
            .map(|path| {
                path.to_str()
                    .ok_or_else(|| ConfigError::NonUtf8Path { path: path.clone() })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let models = toml_array(model_paths)?;
        Ok(format!(
            "version = {}\njobs = {}\n\n[output]\nformat = {}\ncolor = {}\nquiet = {}\n\n[lint]\nselect = {select}\nignore = {ignore}\ndeny = {deny}\nwarn = {warn}\n\n[validate]\nstrict = {}\n\n[fmt]\nline-width = {}\n\n[model]\npaths = {models}\n",
            self.version,
            self.jobs,
            toml_string(self.output.format.as_str())?,
            toml_string(self.output.color.as_str())?,
            self.output.quiet,
            self.validate.strict,
            self.fmt.line_width,
        ))
    }

    /// Compile the validation finding policy, including strict warning handling.
    pub(crate) fn validation_policy(&self) -> Result<DiagnosticPolicy, ConfigError> {
        self.policy(self.validate.strict)
    }

    /// Compile the lint finding policy without validation-only strictness.
    pub(crate) fn lint_policy(&self) -> Result<DiagnosticPolicy, ConfigError> {
        self.policy(false)
    }

    fn policy(&self, warnings_as_errors: bool) -> Result<DiagnosticPolicy, ConfigError> {
        let selected = expand_patterns(&self.lint.select)?;
        let ignored = expand_patterns(&self.lint.ignore)?;
        let denied = expand_patterns(&self.lint.deny)?;
        let warned = expand_patterns(&self.lint.warn)?;
        if let Some(code) = denied.intersection(&warned).next() {
            return Err(ConfigError::ConflictingSeverity { code: *code });
        }
        let mut policy = DiagnosticPolicy::new().with_warnings_as_errors(warnings_as_errors);
        if !self.lint.select.is_empty() {
            for code in selected {
                policy = policy.select(code);
            }
        }
        for code in ignored {
            policy = policy.ignore(code);
        }
        for code in denied {
            policy = policy.with_severity(code, Severity::Error);
        }
        for code in warned {
            policy = policy.with_severity(code, Severity::Warning);
        }
        Ok(policy)
    }

    fn apply(&mut self, layer: ConfigLayer) -> Result<(), ConfigError> {
        if let Some(jobs) = layer.jobs {
            if jobs == 0 {
                return Err(ConfigError::ZeroJobs);
            }
            self.jobs = jobs;
        }
        if let Some(format) = layer.output_format {
            self.output.format = format;
        }
        if let Some(color) = layer.color {
            self.output.color = color;
        }
        if let Some(quiet) = layer.quiet {
            self.output.quiet = quiet;
        }
        replace_set(&mut self.lint.select, layer.select);
        replace_set(&mut self.lint.ignore, layer.ignore);
        replace_set(&mut self.lint.deny, layer.deny);
        replace_set(&mut self.lint.warn, layer.warn);
        if let Some(strict) = layer.strict {
            self.validate.strict = strict;
        }
        if let Some(line_width) = layer.line_width {
            if line_width == 0 {
                return Err(ConfigError::ZeroLineWidth);
            }
            self.fmt.line_width = line_width;
        }
        if let Some(model_paths) = layer.model_paths {
            if model_paths.iter().any(|path| path.as_os_str().is_empty()) {
                return Err(ConfigError::EmptyModelPath);
            }
            self.model.paths = model_paths;
        }
        Ok(())
    }

    fn validate_policy(&self) -> Result<(), ConfigError> {
        self.validation_policy().map(|_| ())?;
        self.lint_policy().map(|_| ())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedOutput {
    format: OutputFormat,
    color: ColorChoice,
    quiet: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ResolvedLint {
    select: BTreeSet<String>,
    ignore: BTreeSet<String>,
    deny: BTreeSet<String>,
    warn: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedValidate {
    strict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedFormat {
    line_width: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ResolvedModel {
    paths: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    version: u32,
    jobs: Option<usize>,
    #[serde(default)]
    output: FileOutput,
    #[serde(default)]
    lint: FileLint,
    #[serde(default)]
    validate: FileValidate,
    #[serde(default)]
    fmt: FileFormat,
    #[serde(default)]
    model: FileModel,
}

impl FileConfig {
    fn into_layer(self, path: &Path) -> Result<ConfigLayer, ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                path: path.to_owned(),
                version: self.version,
            });
        }
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        Ok(ConfigLayer {
            jobs: self.jobs,
            output_format: self.output.format,
            color: self.output.color,
            quiet: self.output.quiet,
            select: self.lint.select,
            ignore: self.lint.ignore,
            deny: self.lint.deny,
            warn: self.lint.warn,
            strict: self.validate.strict,
            line_width: self.fmt.line_width,
            model_paths: self.model.paths.map(|paths| resolve_paths(paths, base)),
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileOutput {
    format: Option<OutputFormat>,
    color: Option<ColorChoice>,
    quiet: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileLint {
    select: Option<Vec<String>>,
    ignore: Option<Vec<String>>,
    deny: Option<Vec<String>>,
    warn: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileValidate {
    strict: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileFormat {
    #[serde(rename = "line-width")]
    line_width: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileModel {
    paths: Option<Vec<PathBuf>>,
}

#[derive(Debug, Default)]
struct ConfigLayer {
    jobs: Option<usize>,
    output_format: Option<OutputFormat>,
    color: Option<ColorChoice>,
    quiet: Option<bool>,
    select: Option<Vec<String>>,
    ignore: Option<Vec<String>>,
    deny: Option<Vec<String>>,
    warn: Option<Vec<String>>,
    strict: Option<bool>,
    line_width: Option<usize>,
    model_paths: Option<Vec<PathBuf>>,
}

/// A deterministic configuration usage or loading failure.
#[derive(Debug, Error)]
pub(crate) enum ConfigError {
    /// The process working directory cannot be read.
    #[error("could not determine the current directory: {0}")]
    CurrentDirectory(std::io::Error),
    /// A configuration file cannot be read.
    #[error("could not read configuration {path}: {source}")]
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying filesystem failure.
        source: std::io::Error,
    },
    /// A configuration file is not valid TOML for the closed schema.
    #[error("could not parse configuration {path}: {source}")]
    Parse {
        /// The rejected file.
        path: PathBuf,
        /// The TOML schema failure.
        source: toml::de::Error,
    },
    /// A configuration file declares an unsupported schema version.
    #[error("configuration {path} uses unsupported version {version}; expected {CONFIG_VERSION}")]
    UnsupportedVersion {
        /// The rejected file.
        path: PathBuf,
        /// The unsupported version.
        version: u32,
    },
    /// A reserved environment variable contains non-UTF-8 data.
    #[error("environment variable {name} is not valid UTF-8: {value:?}")]
    NonUtf8Environment {
        /// The variable name.
        name: String,
        /// The rejected operating-system value.
        value: OsString,
    },
    /// An unknown variable uses the reserved analyzer prefix.
    #[error("unknown pure-analyzer environment variable {name}")]
    UnknownEnvironment {
        /// The rejected variable name.
        name: String,
    },
    /// A configuration or environment value is malformed.
    #[error("invalid {field} value {value:?}: {reason}")]
    InvalidValue {
        /// The schema or environment field.
        field: &'static str,
        /// The rejected value.
        value: String,
        /// A concise expected-shape description.
        reason: &'static str,
    },
    /// A registered-code pattern is malformed or matches no code.
    #[error("invalid diagnostic code pattern {pattern:?}: {reason}")]
    InvalidPattern {
        /// The rejected pattern.
        pattern: String,
        /// Why it cannot select from the closed registry.
        reason: &'static str,
    },
    /// The same code is present in both final deny and warn sets.
    #[error("diagnostic code {code} is selected by both deny and warn policy")]
    ConflictingSeverity {
        /// The conflicting registered code.
        code: DiagCode,
    },
    /// Zero workers cannot execute an analysis request.
    #[error("jobs must be at least one")]
    ZeroJobs,
    /// A zero-width formatter setting is invalid.
    #[error("fmt.line-width must be at least one")]
    ZeroLineWidth,
    /// An empty model path is ambiguous.
    #[error("model paths must not be empty")]
    EmptyModelPath,
    /// A resolved path cannot be represented in the UTF-8 TOML format.
    #[error("resolved path is not valid UTF-8: {path:?}")]
    NonUtf8Path {
        /// The path that cannot be printed losslessly.
        path: PathBuf,
    },
    /// Resolved configuration cannot be serialized.
    #[error("could not serialize resolved configuration: {0}")]
    Serialize(serde_json::Error),
}

fn read_config(path: &Path) -> Result<ConfigLayer, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_owned(),
        source,
    })?;
    let config = toml::from_str::<FileConfig>(&text).map_err(|source| ConfigError::Parse {
        path: path.to_owned(),
        source,
    })?;
    config.into_layer(path)
}

fn discover_repository_config(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .map(|directory| directory.join(REPOSITORY_CONFIG_NAME))
        .find(|candidate| candidate.is_file())
}

fn user_config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        user_config_path_from_roots(std::env::var_os("APPDATA"))
    }
    #[cfg(not(windows))]
    {
        user_config_path_from_roots(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
    }
}

#[cfg(windows)]
fn user_config_path_from_roots(appdata: Option<OsString>) -> Option<PathBuf> {
    absolute_path(appdata).map(|root| root.join(USER_CONFIG_DIRECTORY).join(USER_CONFIG_NAME))
}

#[cfg(not(windows))]
fn user_config_path_from_roots(
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    absolute_path(xdg_config_home)
        .or_else(|| absolute_path(home).map(|home| home.join(".config")))
        .map(|root| root.join(USER_CONFIG_DIRECTORY).join(USER_CONFIG_NAME))
}

fn absolute_path(value: Option<OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|path| path.is_absolute())
}

fn environment_layer(
    environment: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ConfigLayer, ConfigError> {
    for name in environment.keys() {
        if name.starts_with(ENV_PREFIX) && !KNOWN_ENVIRONMENT.contains(&name.as_str()) {
            return Err(ConfigError::UnknownEnvironment { name: name.clone() });
        }
    }
    Ok(ConfigLayer {
        jobs: parse_optional(environment, ENV_JOBS, parse_usize)?,
        output_format: parse_optional(environment, ENV_FORMAT, parse_output_format)?,
        color: parse_optional(environment, ENV_COLOR, parse_color)?,
        quiet: parse_optional(environment, ENV_QUIET, parse_bool)?,
        select: environment.get(ENV_SELECT).map(|value| split_list(value)),
        ignore: environment.get(ENV_IGNORE).map(|value| split_list(value)),
        deny: environment.get(ENV_DENY).map(|value| split_list(value)),
        warn: environment.get(ENV_WARN).map(|value| split_list(value)),
        strict: parse_optional(environment, ENV_STRICT, parse_bool)?,
        line_width: parse_optional(environment, ENV_LINE_WIDTH, parse_usize)?,
        model_paths: environment.get(ENV_MODEL_PATHS).map(|value| {
            resolve_paths(
                std::env::split_paths(&OsString::from(value)).collect::<Vec<_>>(),
                cwd,
            )
        }),
    })
}

fn parse_optional<T>(
    environment: &BTreeMap<String, String>,
    name: &'static str,
    parse: fn(&str) -> Result<T, &'static str>,
) -> Result<Option<T>, ConfigError> {
    environment
        .get(name)
        .map(|value| {
            parse(value).map_err(|reason| ConfigError::InvalidValue {
                field: name,
                value: value.clone(),
                reason,
            })
        })
        .transpose()
}

fn parse_usize(value: &str) -> Result<usize, &'static str> {
    value.parse().map_err(|_| "expected a positive integer")
}

fn parse_bool(value: &str) -> Result<bool, &'static str> {
    value.parse().map_err(|_| "expected true or false")
}

fn parse_output_format(value: &str) -> Result<OutputFormat, &'static str> {
    match value {
        "human" => Ok(OutputFormat::Human),
        "json" => Ok(OutputFormat::Json),
        "sarif" => Ok(OutputFormat::Sarif),
        _ => Err("expected human, json, or sarif"),
    }
}

fn parse_color(value: &str) -> Result<ColorChoice, &'static str> {
    match value {
        "auto" => Ok(ColorChoice::Auto),
        "always" => Ok(ColorChoice::Always),
        "never" => Ok(ColorChoice::Never),
        _ => Err("expected auto, always, or never"),
    }
}

fn split_list(value: &str) -> Vec<String> {
    value.split(',').map(str::trim).map(str::to_owned).collect()
}

fn toml_array<'a>(values: impl IntoIterator<Item = &'a str>) -> Result<String, ConfigError> {
    values
        .into_iter()
        .map(toml_string)
        .collect::<Result<Vec<_>, _>>()
        .map(|values| format!("[{}]", values.join(", ")))
}

fn toml_string(value: &str) -> Result<String, ConfigError> {
    serde_json::to_string(value).map_err(ConfigError::Serialize)
}

fn expand_patterns(patterns: &BTreeSet<String>) -> Result<BTreeSet<DiagCode>, ConfigError> {
    let mut expanded = BTreeSet::new();
    for pattern in patterns {
        if pattern.is_empty() {
            return Err(ConfigError::InvalidPattern {
                pattern: pattern.clone(),
                reason: "patterns must not be empty",
            });
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            if prefix.len() < 3
                || !prefix.starts_with("PUR")
                || !prefix[3..].bytes().all(|byte| byte.is_ascii_digit())
                || prefix.len() > 7
            {
                return Err(ConfigError::InvalidPattern {
                    pattern: pattern.clone(),
                    reason: "expected PUR followed by zero to four digits and one trailing *",
                });
            }
            let matches = ALL_DIAG_CODES
                .iter()
                .copied()
                .filter(|code| code.as_str().starts_with(prefix))
                .collect::<Vec<_>>();
            if matches.is_empty() {
                return Err(ConfigError::InvalidPattern {
                    pattern: pattern.clone(),
                    reason: "pattern matches no registered diagnostic code",
                });
            }
            expanded.extend(matches);
        } else {
            let code = pattern
                .parse::<DiagCode>()
                .map_err(|_| ConfigError::InvalidPattern {
                    pattern: pattern.clone(),
                    reason: "expected an exact registered code or a trailing-* prefix",
                })?;
            expanded.insert(code);
        }
    }
    Ok(expanded)
}

fn replace_set(target: &mut BTreeSet<String>, replacement: Option<Vec<String>>) {
    if let Some(values) = replacement {
        *target = values.into_iter().collect();
    }
}

fn resolve_paths(paths: Vec<PathBuf>, base: &Path) -> Vec<PathBuf> {
    paths
        .into_iter()
        .map(|path| {
            if path.as_os_str().is_empty() {
                path
            } else {
                resolve_path(&path, base)
            }
        })
        .collect()
}

fn resolve_path(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

fn nonempty<T>(values: Vec<T>) -> Option<Vec<T>> {
    (!values.is_empty()).then_some(values)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use clap::Parser;

    use super::*;

    static FIXTURE_COUNTER: AtomicUsize = AtomicUsize::new(0);

    const TWO_FINDING_MODEL: &str = r#"{
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

    fn two_finding_request(policy: libpure::DiagnosticPolicy) -> libpure::LintRequest {
        libpure::LintRequest::new(
            libpure::SourceRequest::new([libpure::SourceInput::in_memory(
                "query.pure",
                "model::Person.all()->filter(x| $x.missing)",
            )])
            .with_diagnostic_policy(policy),
            [
                libpure::ModelInput::pmcd(libpure::SourceInput::in_memory(
                    "first.json",
                    TWO_FINDING_MODEL,
                )),
                libpure::ModelInput::pmcd(libpure::SourceInput::in_memory(
                    "second.json",
                    TWO_FINDING_MODEL,
                )),
            ],
        )
    }

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        config: ConfigFlags,
    }

    struct DirectoryFixture {
        path: PathBuf,
    }

    impl DirectoryFixture {
        fn new(name: &str) -> Self {
            let counter = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pure-analyzer-config-{}-{counter}-{name}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create config fixture");
            Self { path }
        }

        fn write(&self, relative: &str, text: &str) -> PathBuf {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create fixture parent");
            }
            std::fs::write(&path, text).expect("write config fixture");
            path
        }
    }

    impl Drop for DirectoryFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn flags(arguments: &[&str]) -> ConfigFlags {
        TestCli::try_parse_from(arguments)
            .expect("parse config flags")
            .config
    }

    fn resolve(
        cwd: &Path,
        user_config: Option<PathBuf>,
        environment: BTreeMap<String, String>,
        arguments: &[&str],
        overrides: ConfigOverrides,
    ) -> Result<ResolvedConfig, ConfigError> {
        ConfigResolver::new(cwd, user_config, environment).resolve(&flags(arguments), overrides)
    }

    #[test]
    fn defaults_serialize_deterministically() {
        let fixture = DirectoryFixture::new("defaults");
        let first = resolve(
            &fixture.path,
            None,
            BTreeMap::new(),
            &["test", "--no-config"],
            ConfigOverrides::default(),
        )
        .expect("resolve defaults");
        let second = resolve(
            &fixture.path,
            None,
            BTreeMap::new(),
            &["test", "--no-config"],
            ConfigOverrides::default(),
        )
        .expect("resolve defaults again");

        assert_eq!(first, second);
        assert_eq!(
            first.to_toml().expect("serialize"),
            second.to_toml().expect("serialize")
        );
        let serialized = first.to_toml().expect("serialize");
        assert!(serialized.contains("version = 1"));
        let reparsed = toml::from_str::<FileConfig>(&serialized).expect("reparse resolved config");
        assert_eq!(reparsed.version, CONFIG_VERSION);
        let round_trip_path = fixture.write("round-trip.toml", &serialized);
        let arguments = [
            "test",
            "--config",
            round_trip_path.to_str().expect("utf8 fixture"),
        ];
        let round_trip = resolve(
            &fixture.path,
            None,
            BTreeMap::new(),
            &arguments,
            ConfigOverrides::default(),
        )
        .expect("resolve serialized config");
        assert_eq!(first, round_trip);
    }

    #[test]
    fn json_string_escaping_is_valid_for_toml_basic_strings() {
        for value in [
            "plain",
            "quote\"and\\backslash",
            "line\nfeed\ttab",
            "control\u{0001}",
            "non-ascii-λ",
        ] {
            let document = format!("value = {}\n", toml_string(value).expect("escape string"));
            let parsed = document.parse::<toml::Table>().expect("valid TOML string");
            assert_eq!(parsed["value"].as_str(), Some(value));
        }
    }

    #[test]
    fn serialization_canonicalizes_policy_set_order() {
        let fixture = DirectoryFixture::new("serialization-order");
        let first_path = fixture.write(
            "first.toml",
            "version = 1\n[lint]\nselect = [\"PUR2002\", \"PUR2001\"]\nignore = [\"PUR2101\", \"PUR2100\"]\ndeny = [\"PUR9000\", \"PUR2003\"]\nwarn = [\"PUR1202\", \"PUR1201\"]\n[model]\npaths = [\"first.pure\", \"second.pure\"]\n",
        );
        let second_path = fixture.write(
            "second.toml",
            "version = 1\n[lint]\nselect = [\"PUR2001\", \"PUR2002\"]\nignore = [\"PUR2100\", \"PUR2101\"]\ndeny = [\"PUR2003\", \"PUR9000\"]\nwarn = [\"PUR1201\", \"PUR1202\"]\n[model]\npaths = [\"first.pure\", \"second.pure\"]\n",
        );
        let first_arguments = [
            "test",
            "--config",
            first_path.to_str().expect("utf8 fixture"),
        ];
        let second_arguments = [
            "test",
            "--config",
            second_path.to_str().expect("utf8 fixture"),
        ];
        let first = resolve(
            &fixture.path,
            None,
            BTreeMap::new(),
            &first_arguments,
            ConfigOverrides::default(),
        )
        .expect("resolve first ordering");
        let second = resolve(
            &fixture.path,
            None,
            BTreeMap::new(),
            &second_arguments,
            ConfigOverrides::default(),
        )
        .expect("resolve second ordering");

        assert_eq!(first, second);
        let serialized = first.to_toml().expect("serialize first ordering");
        assert_eq!(
            serialized,
            second.to_toml().expect("serialize second ordering")
        );
        assert!(serialized.contains("select = [\"PUR2001\", \"PUR2002\"]"));
        assert!(serialized.contains("ignore = [\"PUR2100\", \"PUR2101\"]"));
        assert!(serialized.contains("deny = [\"PUR2003\", \"PUR9000\"]"));
        assert!(serialized.contains("warn = [\"PUR1201\", \"PUR1202\"]"));
    }

    #[test]
    fn closed_schema_rejects_missing_version_unknown_fields_and_wrong_types() {
        let fixture = DirectoryFixture::new("schema");
        for (name, text) in [
            ("missing.toml", "jobs = 2\n"),
            ("unknown.toml", "version = 1\nmystery = true\n"),
            ("type.toml", "version = 1\njobs = \"many\"\n"),
        ] {
            let path = fixture.write(name, text);
            let arguments = ["test", "--config", path.to_str().expect("utf8 fixture")];
            assert!(
                resolve(
                    &fixture.path,
                    None,
                    BTreeMap::new(),
                    &arguments,
                    ConfigOverrides::default(),
                )
                .is_err(),
                "{name} must fail"
            );
        }
        let unsupported = fixture.write("version.toml", "version = 2\n");
        let arguments = [
            "test",
            "--config",
            unsupported.to_str().expect("utf8 fixture"),
        ];
        assert!(matches!(
            resolve(
                &fixture.path,
                None,
                BTreeMap::new(),
                &arguments,
                ConfigOverrides::default(),
            ),
            Err(ConfigError::UnsupportedVersion { version: 2, .. })
        ));
    }

    #[test]
    fn nearest_repository_config_overrides_user_and_resolves_its_paths() {
        let fixture = DirectoryFixture::new("discovery");
        let user = fixture.write(
            "user.toml",
            "version = 1\njobs = 2\n[output]\ncolor = \"always\"\n",
        );
        fixture.write(
            REPOSITORY_CONFIG_NAME,
            "version = 1\njobs = 3\n[model]\npaths = [\"root.pure\"]\n",
        );
        let nested = fixture.path.join("nested/deeper");
        std::fs::create_dir_all(&nested).expect("create nested directory");
        let nearest = fixture.write(
            "nested/.pure-analyzer.toml",
            "version = 1\njobs = 4\n[model]\npaths = [\"model.pure\"]\n",
        );

        let resolved = resolve(
            &nested,
            Some(user),
            BTreeMap::new(),
            &["test"],
            ConfigOverrides::default(),
        )
        .expect("resolve discovered files");

        assert_eq!(resolved.jobs, 4);
        assert_eq!(resolved.output.color, ColorChoice::Always);
        assert_eq!(
            resolved.model.paths,
            vec![nearest.parent().expect("config parent").join("model.pure")]
        );
    }

    #[test]
    fn explicit_config_and_no_config_control_file_discovery() {
        let fixture = DirectoryFixture::new("explicit");
        let discovered = fixture.write(
            REPOSITORY_CONFIG_NAME,
            "version = 1\njobs = 2\n[output]\ncolor = \"always\"\n",
        );
        let explicit = fixture.write("chosen.toml", "version = 1\njobs = 5\n");
        let explicit_args = ["test", "--config", explicit.to_str().expect("utf8 fixture")];
        let chosen = resolve(
            &fixture.path,
            Some(discovered),
            BTreeMap::new(),
            &explicit_args,
            ConfigOverrides::default(),
        )
        .expect("resolve explicit config");
        let disabled = resolve(
            &fixture.path,
            Some(explicit),
            BTreeMap::new(),
            &["test", "--no-config"],
            ConfigOverrides::default(),
        )
        .expect("disable config files");

        assert_eq!(chosen.jobs, 5);
        assert_eq!(chosen.output.color, ColorChoice::Always);
        assert_eq!(disabled.jobs, DEFAULT_JOBS);
        assert!(TestCli::try_parse_from(["test", "--config", "a", "--no-config"]).is_err());
        let missing = fixture.path.join("missing.toml");
        let missing_args = ["test", "--config", missing.to_str().expect("utf8 fixture")];
        assert!(matches!(
            resolve(
                &fixture.path,
                None,
                BTreeMap::new(),
                &missing_args,
                ConfigOverrides::default(),
            ),
            Err(ConfigError::Read { .. })
        ));
    }

    #[test]
    fn environment_and_cli_override_file_layers_field_by_field() {
        let fixture = DirectoryFixture::new("precedence");
        fixture.write(
            REPOSITORY_CONFIG_NAME,
            "version = 1\njobs = 2\n[output]\nformat = \"human\"\nquiet = true\n[lint]\ndeny = [\"PUR2*\"]\n[validate]\nstrict = true\n[fmt]\nline-width = 80\n",
        );
        let environment = BTreeMap::from([
            (ENV_JOBS.to_owned(), "3".to_owned()),
            (ENV_FORMAT.to_owned(), "json".to_owned()),
            (ENV_DENY.to_owned(), "PUR2001".to_owned()),
            (ENV_STRICT.to_owned(), "false".to_owned()),
        ]);
        let cli = flags(&[
            "test",
            "--jobs",
            "4",
            "--format",
            "sarif",
            "--no-quiet",
            "--deny",
            "PUR2002",
        ]);
        let overrides = cli.overrides(Some(true), Some(120), vec![PathBuf::from("model.pure")]);
        let resolved = ConfigResolver::new(&fixture.path, None, environment)
            .resolve(&cli, overrides)
            .expect("resolve all layers");

        assert_eq!(resolved.jobs(), 4);
        assert_eq!(resolved.output_format(), OutputFormat::Sarif);
        assert!(!resolved.quiet());
        assert_eq!(resolved.lint.deny, BTreeSet::from(["PUR2002".to_owned()]));
        assert!(resolved.validate_strict());
        assert_eq!(resolved.line_width(), 120);
        assert_eq!(resolved.model_paths(), &[fixture.path.join("model.pure")]);
    }

    #[test]
    fn environment_validation_is_fail_closed() {
        let fixture = DirectoryFixture::new("environment");
        for environment in [
            BTreeMap::from([(ENV_JOBS.to_owned(), "zero".to_owned())]),
            BTreeMap::from([("PURE_ANALYZER_JBOS".to_owned(), "2".to_owned())]),
            BTreeMap::from([(ENV_COLOR.to_owned(), "sometimes".to_owned())]),
        ] {
            assert!(
                resolve(
                    &fixture.path,
                    None,
                    environment,
                    &["test", "--no-config"],
                    ConfigOverrides::default(),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn every_supported_environment_value_is_applied() {
        let fixture = DirectoryFixture::new("environment-complete");
        let model_paths = std::env::join_paths(["first.pure", "second.pure"])
            .expect("join model paths")
            .into_string()
            .expect("utf8 model paths");
        let environment = BTreeMap::from([
            (ENV_JOBS.to_owned(), "6".to_owned()),
            (ENV_FORMAT.to_owned(), "json".to_owned()),
            (ENV_COLOR.to_owned(), "never".to_owned()),
            (ENV_QUIET.to_owned(), "true".to_owned()),
            (ENV_SELECT.to_owned(), "PUR2*".to_owned()),
            (ENV_IGNORE.to_owned(), "PUR2100".to_owned()),
            (ENV_DENY.to_owned(), "PUR2001".to_owned()),
            (ENV_WARN.to_owned(), "PUR2002".to_owned()),
            (ENV_STRICT.to_owned(), "true".to_owned()),
            (ENV_LINE_WIDTH.to_owned(), "88".to_owned()),
            (ENV_MODEL_PATHS.to_owned(), model_paths),
        ]);

        let resolved = resolve(
            &fixture.path,
            None,
            environment,
            &["test", "--no-config"],
            ConfigOverrides::default(),
        )
        .expect("resolve every environment value");

        assert_eq!(resolved.jobs(), 6);
        assert_eq!(resolved.output_format(), OutputFormat::Json);
        assert_eq!(resolved.color(), ColorChoice::Never);
        assert!(resolved.quiet());
        assert_eq!(resolved.lint.select, BTreeSet::from(["PUR2*".to_owned()]));
        assert_eq!(resolved.lint.ignore, BTreeSet::from(["PUR2100".to_owned()]));
        assert_eq!(resolved.lint.deny, BTreeSet::from(["PUR2001".to_owned()]));
        assert_eq!(resolved.lint.warn, BTreeSet::from(["PUR2002".to_owned()]));
        assert!(resolved.validate_strict());
        assert_eq!(resolved.line_width(), 88);
        assert_eq!(
            resolved.model_paths(),
            &[
                fixture.path.join("first.pure"),
                fixture.path.join("second.pure")
            ]
        );
    }

    #[test]
    fn exact_and_prefix_patterns_compile_against_the_closed_registry() {
        let fixture = DirectoryFixture::new("patterns");
        let arguments = [
            "test",
            "--no-config",
            "--select",
            "PUR2*,PUR2001",
            "--ignore",
            "PUR2100",
            "--deny",
            "PUR2001",
        ];
        let parsed = flags(&arguments);
        let resolved = ConfigResolver::new(&fixture.path, None, BTreeMap::new())
            .resolve(&parsed, parsed.overrides(None, None, Vec::new()))
            .expect("compile patterns");
        let policy = resolved.lint_policy().expect("lint policy");

        let request = libpure::SourceRequest::new([libpure::SourceInput::in_memory(
            "query.pure",
            "(value, other)",
        )])
        .with_diagnostic_policy(policy);
        let output = libpure::AnalysisDriver
            .validate(&request)
            .expect("validate with selected lint codes");
        assert!(output.diagnostics().is_empty());
    }

    #[test]
    fn selection_and_ignore_filters_apply_to_model_findings() {
        let fixture = DirectoryFixture::new("selection-and-ignore");
        let selected_flags = flags(&["test", "--no-config", "--select", "PUR9000"]);
        let selected = ConfigResolver::new(&fixture.path, None, BTreeMap::new())
            .resolve(
                &selected_flags,
                selected_flags.overrides(None, None, Vec::new()),
            )
            .expect("resolve selected policy");
        let selected_output = libpure::AnalysisDriver
            .lint(&two_finding_request(
                selected.lint_policy().expect("compile selected policy"),
            ))
            .expect("lint selected model finding");
        assert_eq!(
            selected_output
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            vec![DiagCode::ModelMergeConflict]
        );

        let ignored_flags = flags(&[
            "test",
            "--no-config",
            "--select",
            "PUR9000",
            "--ignore",
            "PUR9000",
        ]);
        let ignored = ConfigResolver::new(&fixture.path, None, BTreeMap::new())
            .resolve(
                &ignored_flags,
                ignored_flags.overrides(None, None, Vec::new()),
            )
            .expect("resolve ignored policy");
        let ignored_output = libpure::AnalysisDriver
            .lint(&two_finding_request(
                ignored.lint_policy().expect("compile ignored policy"),
            ))
            .expect("lint ignored model finding");
        assert!(ignored_output.diagnostics().is_empty());
    }

    #[test]
    fn invalid_empty_unknown_and_no_match_patterns_are_usage_errors() {
        let fixture = DirectoryFixture::new("bad-patterns");
        for pattern in ["", "pur2*", "PUR9999", "PUR8*"] {
            let parsed = flags(&["test", "--no-config", "--select", pattern]);
            assert!(matches!(
                ConfigResolver::new(&fixture.path, None, BTreeMap::new())
                    .resolve(&parsed, parsed.overrides(None, None, Vec::new()),),
                Err(ConfigError::InvalidPattern { .. })
            ));
        }
    }

    #[test]
    fn conflicting_deny_and_warn_patterns_are_rejected_after_expansion() {
        let fixture = DirectoryFixture::new("conflict");
        let parsed = flags(&[
            "test",
            "--no-config",
            "--deny",
            "PUR2*",
            "--warn",
            "PUR2001",
        ]);
        assert!(matches!(
            ConfigResolver::new(&fixture.path, None, BTreeMap::new())
                .resolve(&parsed, parsed.overrides(None, None, Vec::new()),),
            Err(ConfigError::ConflictingSeverity {
                code: DiagCode::WrongMilestoningArity
            })
        ));
    }

    #[test]
    fn severity_patterns_reclassify_model_and_source_findings_without_changing_identity() {
        let fixture = DirectoryFixture::new("severity-policy");
        let config = fixture.write(
            "policy.toml",
            "version = 1\n[lint]\ndeny = [\"PUR9000\"]\nwarn = [\"PUR2002\"]\n",
        );
        let arguments = ["test", "--config", config.to_str().expect("utf8 fixture")];
        let resolved = resolve(
            &fixture.path,
            None,
            BTreeMap::new(),
            &arguments,
            ConfigOverrides::default(),
        )
        .expect("resolve severity policy");
        let driver = libpure::AnalysisDriver;
        let baseline = driver
            .lint(&two_finding_request(libpure::DiagnosticPolicy::new()))
            .expect("lint without severity overrides");
        let transformed = driver
            .lint(&two_finding_request(
                resolved.lint_policy().expect("compile severity policy"),
            ))
            .expect("lint with severity overrides");

        for (code, baseline_severity, transformed_severity) in [
            (
                DiagCode::ModelMergeConflict,
                Severity::Warning,
                Severity::Error,
            ),
            (
                DiagCode::UnknownProperty,
                Severity::Error,
                Severity::Warning,
            ),
        ] {
            let original = baseline
                .diagnostics()
                .iter()
                .find(|diagnostic| diagnostic.code == code)
                .expect("baseline finding");
            let changed = transformed
                .diagnostics()
                .iter()
                .find(|diagnostic| diagnostic.code == code)
                .expect("reclassified finding");
            assert_eq!(original.severity, baseline_severity);
            assert_eq!(changed.severity, transformed_severity);
            assert_eq!(changed.code, original.code);
            assert_eq!(changed.message, original.message);
            assert_eq!(changed.primary, original.primary);
            assert_eq!(changed.secondary, original.secondary);
        }
    }

    #[test]
    fn strict_validation_policy_promotes_model_merge_without_affecting_lint_policy() {
        let fixture = DirectoryFixture::new("strict-policy");
        let config = fixture.write("strict.toml", "version = 1\n[validate]\nstrict = true\n");
        let arguments = ["test", "--config", config.to_str().expect("utf8 fixture")];
        let resolved = resolve(
            &fixture.path,
            None,
            BTreeMap::new(),
            &arguments,
            ConfigOverrides::default(),
        )
        .expect("resolve strict validation policy");

        let driver = libpure::AnalysisDriver;
        let validation_output = driver
            .lint(&two_finding_request(
                resolved
                    .validation_policy()
                    .expect("compile strict validation policy"),
            ))
            .expect("lint with validation policy");
        let lint_output = driver
            .lint(&two_finding_request(
                resolved.lint_policy().expect("compile lint policy"),
            ))
            .expect("lint with lint policy");

        let validation_merge = validation_output
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == DiagCode::ModelMergeConflict)
            .expect("validation model merge finding");
        let lint_merge = lint_output
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code == DiagCode::ModelMergeConflict)
            .expect("lint model merge finding");
        assert_eq!(validation_merge.severity, Severity::Error);
        assert_eq!(lint_merge.severity, Severity::Warning);
    }

    #[test]
    fn zero_jobs_line_width_and_empty_model_paths_are_rejected() {
        let fixture = DirectoryFixture::new("numeric");
        for text in [
            "version = 1\njobs = 0\n",
            "version = 1\n[fmt]\nline-width = 0\n",
            "version = 1\n[model]\npaths = [\"\"]\n",
        ] {
            let path = fixture.write("invalid.toml", text);
            let arguments = ["test", "--config", path.to_str().expect("utf8 fixture")];
            assert!(
                resolve(
                    &fixture.path,
                    None,
                    BTreeMap::new(),
                    &arguments,
                    ConfigOverrides::default(),
                )
                .is_err()
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn empty_or_relative_xdg_config_home_falls_back_to_absolute_home() {
        let home = PathBuf::from("/home/config-user");
        assert_eq!(
            user_config_path_from_roots(Some(OsString::new()), Some(home.clone().into_os_string())),
            Some(
                home.join(".config")
                    .join(USER_CONFIG_DIRECTORY)
                    .join(USER_CONFIG_NAME)
            )
        );
        assert_eq!(
            user_config_path_from_roots(
                Some(OsString::from("relative-config")),
                Some(home.clone().into_os_string()),
            ),
            Some(
                home.join(".config")
                    .join(USER_CONFIG_DIRECTORY)
                    .join(USER_CONFIG_NAME)
            )
        );
        assert_eq!(
            user_config_path_from_roots(Some(OsString::new()), Some(OsString::new())),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn empty_or_relative_appdata_does_not_create_a_relative_user_config_path() {
        for appdata in [OsString::new(), OsString::from("relative-appdata")] {
            assert_eq!(user_config_path_from_roots(Some(appdata)), None);
        }
    }
}
