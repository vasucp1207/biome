use crate::configuration::diagnostics::InvalidIgnorePattern;
use crate::configuration::formatter::to_format_settings;
use crate::configuration::linter::to_linter_settings;
use crate::configuration::organize_imports::to_organize_imports_settings;
use crate::configuration::{
    push_to_analyzer_rules, to_override_settings, CssConfiguration, JavascriptConfiguration,
    JsonConfiguration,
};
use crate::{
    configuration::FilesConfiguration, Configuration, ConfigurationDiagnostic, Matcher, Rules,
    WorkspaceError,
};
use biome_analyze::AnalyzerRules;
use biome_css_formatter::context::CssFormatOptions;
use biome_css_parser::CssParserOptions;
use biome_css_syntax::CssLanguage;
use biome_deserialize::{Merge, StringSet};
use biome_diagnostics::Category;
use biome_formatter::{IndentStyle, IndentWidth, LineEnding, LineWidth};
use biome_fs::RomePath;
use biome_js_analyze::metadata;
use biome_js_formatter::context::JsFormatOptions;
use biome_js_parser::JsParserOptions;
use biome_js_syntax::JsLanguage;
use biome_json_formatter::context::JsonFormatOptions;
use biome_json_parser::JsonParserOptions;
use biome_json_syntax::JsonLanguage;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use indexmap::IndexSet;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::{
    num::NonZeroU64,
    sync::{RwLock, RwLockReadGuard},
};

/// Global settings for the entire workspace
#[derive(Debug, Default)]
pub struct WorkspaceSettings {
    /// Formatter settings applied to all files in the workspaces
    pub formatter: FormatSettings,
    /// Linter settings applied to all files in the workspace
    pub linter: LinterSettings,
    /// Language specific settings
    pub languages: LanguageListSettings,
    /// Filesystem settings for the workspace
    pub files: FilesSettings,
    /// Analyzer settings
    pub organize_imports: OrganizeImportsSettings,
    /// overrides
    pub override_settings: OverrideSettings,
}

impl WorkspaceSettings {
    /// Retrieves the settings of the formatter
    pub fn formatter(&self) -> &FormatSettings {
        &self.formatter
    }

    /// Whether the formatter is disabled for JavaScript files
    pub fn javascript_formatter_disabled(&self) -> bool {
        let enabled = self.languages.javascript.formatter.enabled.as_ref();
        enabled == Some(&false)
    }

    /// Whether the formatter is disabled for JSON files
    pub fn json_formatter_disabled(&self) -> bool {
        let enabled = self.languages.json.formatter.enabled.as_ref();
        enabled == Some(&false)
    }

    /// Whether the formatter is disabled for CSS files
    pub fn css_formatter_disabled(&self) -> bool {
        let enabled = self.languages.css.formatter.enabled.as_ref();
        enabled == Some(&false)
    }

    /// Retrieves the settings of the linter
    pub fn linter(&self) -> &LinterSettings {
        &self.linter
    }

    /// Retrieves the settings of the organize imports
    pub fn organize_imports(&self) -> &OrganizeImportsSettings {
        &self.organize_imports
    }

    /// The (configuration)[Configuration] is merged into the workspace
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn merge_with_configuration(
        &mut self,
        configuration: Configuration,
        working_directory: Option<PathBuf>,
        vcs_path: Option<PathBuf>,
        gitignore_matches: &[String],
    ) -> Result<(), WorkspaceError> {
        // formatter part
        if let Some(formatter) = configuration.formatter {
            self.formatter = to_format_settings(working_directory.clone(), formatter)?;
        }

        // linter part
        if let Some(linter) = configuration.linter {
            self.linter = to_linter_settings(working_directory.clone(), linter)?;
        }

        // Filesystem settings
        if let Some(files) = to_file_settings(
            working_directory.clone(),
            configuration.files,
            vcs_path,
            gitignore_matches,
        )? {
            self.files = files;
        }

        if let Some(organize_imports) = configuration.organize_imports {
            self.organize_imports =
                to_organize_imports_settings(working_directory.clone(), organize_imports)?;
        }

        // javascript settings
        if let Some(javascript) = configuration.javascript {
            self.languages.javascript = javascript.into();
        }
        // json settings
        if let Some(json) = configuration.json {
            self.languages.json = json.into();
        }
        // css settings
        if let Some(css) = configuration.css {
            self.languages.css = css.into();
        }

        // NOTE: keep this last. Computing the overrides require reading the settings computed by the parent settings.
        if let Some(overrides) = configuration.overrides {
            self.override_settings =
                to_override_settings(working_directory.clone(), overrides, self)?;
        }

        Ok(())
    }

    /// It retrieves the severity based on the `code` of the rule and the current configuration.
    ///
    /// The code of the has the following pattern: `{group}/{rule_name}`.
    ///
    /// It returns [None] if the `code` doesn't match any rule.
    pub fn get_severity_from_rule_code(
        &self,
        code: &Category,
    ) -> Option<biome_diagnostics::Severity> {
        let rules = self.linter.rules.as_ref();
        if let Some(rules) = rules {
            rules.get_severity_from_code(code)
        } else {
            None
        }
    }

    /// Returns rules taking overrides into account.
    pub fn as_rules(&self, path: &Path) -> Option<Cow<Rules>> {
        let mut result = self.linter.rules.as_ref().map(Cow::Borrowed);
        let overrides = &self.override_settings;
        for pattern in overrides.patterns.iter() {
            let excluded = pattern.exclude.matches_path(path);
            if !excluded && !pattern.include.is_empty() && pattern.include.matches_path(path) {
                let pattern_rules = pattern.linter.rules.as_ref();
                if let Some(pattern_rules) = pattern_rules {
                    result = if let Some(mut result) = result.take() {
                        // Override rules
                        result.to_mut().merge_with(pattern_rules.clone());
                        Some(result)
                    } else {
                        Some(Cow::Borrowed(pattern_rules))
                    };
                }
            }
        }
        result
    }
}

/// Formatter settings for the entire workspace
#[derive(Debug)]
pub struct FormatSettings {
    /// Enabled by default
    pub enabled: bool,
    /// Stores whether formatting should be allowed to proceed if a given file
    /// has syntax errors
    pub format_with_errors: bool,
    pub indent_style: Option<IndentStyle>,
    pub indent_width: Option<IndentWidth>,
    pub line_ending: Option<LineEnding>,
    pub line_width: Option<LineWidth>,
    /// List of ignore paths/files
    pub ignored_files: Matcher,
    /// List of included paths/files
    pub included_files: Matcher,
}

impl Default for FormatSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            format_with_errors: false,
            indent_style: Some(IndentStyle::default()),
            indent_width: Some(IndentWidth::default()),
            line_ending: Some(LineEnding::default()),
            line_width: Some(LineWidth::default()),
            ignored_files: Matcher::empty(),
            included_files: Matcher::empty(),
        }
    }
}

/// Formatter settings for the entire workspace
#[derive(Debug)]
pub struct OverrideFormatSettings {
    /// Enabled by default
    pub enabled: Option<bool>,
    /// Stores whether formatting should be allowed to proceed if a given file
    /// has syntax errors
    pub format_with_errors: bool,
    pub indent_style: Option<IndentStyle>,
    pub indent_width: Option<IndentWidth>,
    pub line_ending: Option<LineEnding>,
    pub line_width: Option<LineWidth>,
}

/// Linter settings for the entire workspace
#[derive(Debug)]
pub struct LinterSettings {
    /// Enabled by default
    pub enabled: bool,

    /// List of rules
    pub rules: Option<Rules>,

    /// List of ignored paths/files to match
    pub ignored_files: Matcher,

    /// List of included paths/files to match
    pub included_files: Matcher,
}

impl Default for LinterSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            rules: Some(Rules::default()),
            ignored_files: Matcher::empty(),
            included_files: Matcher::empty(),
        }
    }
}

/// Linter settings for the entire workspace
#[derive(Debug)]
pub struct OverrideLinterSettings {
    /// Enabled by default
    pub enabled: Option<bool>,

    /// List of rules
    pub rules: Option<Rules>,
}

/// Linter settings for the entire workspace
#[derive(Debug)]
pub struct OrganizeImportsSettings {
    /// Enabled by default
    pub enabled: bool,

    /// List of ignored paths/files to match
    pub ignored_files: Matcher,

    /// List of ignored paths/files to match
    pub included_files: Matcher,
}

impl Default for OrganizeImportsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            ignored_files: Matcher::empty(),
            included_files: Matcher::empty(),
        }
    }
}

/// Linter settings for the entire workspace
#[derive(Debug)]
pub struct OverrideOrganizeImportsSettings {
    /// Enabled by default
    pub enabled: Option<bool>,
}

/// Static map of language names to language-specific settings
#[derive(Debug, Default)]
pub struct LanguageListSettings {
    pub javascript: LanguageSettings<JsLanguage>,
    pub json: LanguageSettings<JsonLanguage>,
    pub css: LanguageSettings<CssLanguage>,
}

impl From<JavascriptConfiguration> for LanguageSettings<JsLanguage> {
    fn from(javascript: JavascriptConfiguration) -> Self {
        let mut language_setting: LanguageSettings<JsLanguage> = LanguageSettings::default();
        let formatter = javascript.formatter;
        if let Some(formatter) = formatter {
            language_setting.formatter.quote_style = formatter.quote_style;
            language_setting.formatter.jsx_quote_style = formatter.jsx_quote_style;
            language_setting.formatter.quote_properties = formatter.quote_properties;
            language_setting.formatter.trailing_comma = formatter.trailing_comma;
            language_setting.formatter.semicolons = formatter.semicolons;
            language_setting.formatter.arrow_parentheses = formatter.arrow_parentheses;
            language_setting.formatter.bracket_spacing = formatter.bracket_spacing.map(Into::into);
            language_setting.formatter.bracket_same_line =
                formatter.bracket_same_line.map(Into::into);
            language_setting.formatter.enabled = formatter.enabled;
            language_setting.formatter.line_width = formatter.line_width;
            language_setting.formatter.indent_width = formatter
                .indent_width
                .map(Into::into)
                .or(formatter.indent_size.map(Into::into));
            language_setting.formatter.indent_style = formatter.indent_style.map(Into::into);
        }

        if let Some(parser) = javascript.parser {
            language_setting.parser.parse_class_parameter_decorators = parser
                .unsafe_parameter_decorators_enabled
                .unwrap_or_default();
        }

        let organize_imports = javascript.organize_imports;
        if let Some(_organize_imports) = organize_imports {}

        language_setting.globals = javascript.globals.map(|global| global.into_index_set());

        language_setting
    }
}

impl From<JsonConfiguration> for LanguageSettings<JsonLanguage> {
    fn from(json: JsonConfiguration) -> Self {
        let mut language_setting: LanguageSettings<JsonLanguage> = LanguageSettings::default();
        if let Some(parser) = json.parser {
            language_setting.parser.allow_comments = parser.allow_comments.unwrap_or_default();
            language_setting.parser.allow_trailing_commas =
                parser.allow_trailing_commas.unwrap_or_default();
        }
        if let Some(formatter) = json.formatter {
            language_setting.formatter.enabled = formatter.enabled;
            language_setting.formatter.line_width = formatter.line_width;
            language_setting.formatter.indent_width = formatter
                .indent_width
                .map(Into::into)
                .or(formatter.indent_size.map(Into::into));
            language_setting.formatter.indent_style = formatter.indent_style.map(Into::into);
        }
        language_setting
    }
}

impl From<CssConfiguration> for LanguageSettings<CssLanguage> {
    fn from(css: CssConfiguration) -> Self {
        let mut language_setting: LanguageSettings<CssLanguage> = LanguageSettings::default();
        if let Some(formatter) = css.formatter {
            language_setting.formatter.enabled = formatter.enabled;
            language_setting.formatter.line_width = formatter.line_width;
            language_setting.formatter.indent_width = formatter
                .indent_width
                .map(Into::into)
                .or(formatter.indent_size.map(Into::into));
            language_setting.formatter.indent_style = formatter.indent_style.map(Into::into);
            language_setting.formatter.quote_style = formatter.quote_style;
        }
        language_setting
    }
}

pub trait Language: biome_rowan::Language {
    /// Formatter settings type for this language
    type FormatterSettings: Default;

    type LinterSettings: Default;

    /// Organize imports settings type for this language
    type OrganizeImportsSettings: Default;

    /// Fully resolved formatter options type for this language
    type FormatOptions: biome_formatter::FormatOptions;

    /// Settings that belong to the parser
    type ParserSettings: Default;

    /// Read the settings type for this language from the [LanguageListSettings] map
    fn lookup_settings(languages: &LanguageListSettings) -> &LanguageSettings<Self>;

    /// Resolve the formatter options from the global (workspace level),
    /// per-language and editor provided formatter settings
    fn resolve_format_options(
        global: &FormatSettings,
        overrides: &OverrideSettings,
        language: &Self::FormatterSettings,
        path: &RomePath,
    ) -> Self::FormatOptions;
}

#[derive(Debug, Default)]
pub struct LanguageSettings<L: Language> {
    /// Formatter settings for this language
    pub formatter: L::FormatterSettings,

    /// Linter settings for this language
    pub linter: L::LinterSettings,

    /// Globals variables/bindings that can be found in a file
    pub globals: Option<IndexSet<String>>,

    /// Organize imports settings for this language
    pub organize_imports: L::OrganizeImportsSettings,

    /// Parser settings for this language
    pub parser: L::ParserSettings,
}

/// Filesystem settings for the entire workspace
#[derive(Debug)]
pub struct FilesSettings {
    /// File size limit in bytes
    pub max_size: NonZeroU64,

    /// gitignore file patterns
    pub git_ignore: Option<Gitignore>,

    /// List of paths/files to matcher
    pub ignored_files: Matcher,

    /// List of paths/files to matcher
    pub included_files: Matcher,

    /// Files not recognized by Biome should not emit a diagnostic
    pub ignore_unknown: bool,
}

/// Limit the size of files to 1.0 MiB by default
const DEFAULT_FILE_SIZE_LIMIT: NonZeroU64 =
    // SAFETY: This constant is initialized with a non-zero value
    unsafe { NonZeroU64::new_unchecked(1024 * 1024) };

impl Default for FilesSettings {
    fn default() -> Self {
        Self {
            max_size: DEFAULT_FILE_SIZE_LIMIT,
            git_ignore: None,
            ignored_files: Matcher::empty(),
            included_files: Matcher::empty(),
            ignore_unknown: false,
        }
    }
}

fn to_file_settings(
    working_directory: Option<PathBuf>,
    config: Option<FilesConfiguration>,
    vcs_config_path: Option<PathBuf>,
    gitignore_matches: &[String],
) -> Result<Option<FilesSettings>, WorkspaceError> {
    let config = if let Some(config) = config {
        Some(config)
    } else if vcs_config_path.is_some() {
        Some(FilesConfiguration::default())
    } else {
        None
    };
    let git_ignore = if let Some(vcs_config_path) = vcs_config_path {
        Some(to_git_ignore(vcs_config_path, gitignore_matches)?)
    } else {
        None
    };
    Ok(if let Some(config) = config {
        Some(FilesSettings {
            max_size: config.max_size.unwrap_or(DEFAULT_FILE_SIZE_LIMIT),
            git_ignore,
            ignored_files: to_matcher(working_directory.clone(), config.ignore.as_ref())?,
            included_files: to_matcher(working_directory, config.include.as_ref())?,
            ignore_unknown: config.ignore_unknown.unwrap_or_default(),
        })
    } else {
        None
    })
}

/// Handle object holding a temporary lock on the workspace settings until
/// the deferred language-specific options resolution is called
#[derive(Debug)]
pub struct SettingsHandle<'a> {
    inner: RwLockReadGuard<'a, WorkspaceSettings>,
}

impl<'a> SettingsHandle<'a> {
    pub(crate) fn new(settings: &'a RwLock<WorkspaceSettings>) -> Self {
        Self {
            inner: settings.read().unwrap(),
        }
    }
}

impl<'a> AsRef<WorkspaceSettings> for SettingsHandle<'a> {
    fn as_ref(&self) -> &WorkspaceSettings {
        &self.inner
    }
}

impl<'a> SettingsHandle<'a> {
    /// Resolve the formatting context for the given language
    pub(crate) fn format_options<L>(self, path: &RomePath) -> L::FormatOptions
    where
        L: Language,
    {
        L::resolve_format_options(
            &self.inner.formatter,
            &self.inner.override_settings,
            &L::lookup_settings(&self.inner.languages).formatter,
            path,
        )
    }
}

#[derive(Debug, Default)]
pub struct OverrideSettings {
    pub patterns: Vec<OverrideSettingPattern>,
}

impl OverrideSettings {
    /// Checks whether at least one override excludes the provided `path`
    pub fn is_path_excluded(&self, path: &Path) -> Option<bool> {
        for pattern in &self.patterns {
            if !pattern.exclude.is_empty() && pattern.exclude.matches_path(path) {
                return Some(true);
            }
        }
        None
    }
    /// Checks whether at least one override include the provided `path`
    pub fn is_path_included(&self, path: &Path) -> Option<bool> {
        for pattern in &self.patterns {
            if !pattern.include.is_empty() && pattern.include.matches_path(path) {
                return Some(true);
            }
        }
        None
    }

    /// It scans the current override rules and return the formatting options that of the first override is matched
    pub fn override_js_format_options(
        &self,
        path: &Path,
        options: JsFormatOptions,
    ) -> JsFormatOptions {
        self.patterns.iter().fold(options, |mut options, pattern| {
            let included = !pattern.include.is_empty() && pattern.include.matches_path(path);
            let excluded = !pattern.exclude.is_empty() && pattern.exclude.matches_path(path);

            if excluded {
                return options;
            }

            if included {
                let js_formatter = &pattern.languages.javascript.formatter;
                let formatter = &pattern.formatter;
                if let Some(indent_style) = js_formatter.indent_style.or(formatter.indent_style) {
                    options.set_indent_style(indent_style);
                }

                if let Some(indent_width) = js_formatter.indent_width.or(formatter.indent_width) {
                    options.set_indent_width(indent_width)
                }
                if let Some(line_width) = js_formatter.line_width.or(formatter.line_width) {
                    options.set_line_width(line_width);
                }
                if let Some(quote_style) = js_formatter.quote_style {
                    options.set_quote_style(quote_style);
                }
                if let Some(trailing_comma) = js_formatter.trailing_comma {
                    options.set_trailing_comma(trailing_comma);
                }
                if let Some(quote_properties) = js_formatter.quote_properties {
                    options.set_quote_properties(quote_properties);
                }
                if let Some(jsx_quote_style) = js_formatter.jsx_quote_style {
                    options.set_jsx_quote_style(jsx_quote_style);
                }
                if let Some(semicolons) = js_formatter.semicolons {
                    options.set_semicolons(semicolons);
                }
                if let Some(arrow_parentheses) = js_formatter.arrow_parentheses {
                    options.set_arrow_parentheses(arrow_parentheses);
                }
                if let Some(bracket_spacing) = js_formatter.bracket_spacing {
                    options.set_bracket_spacing(bracket_spacing);
                }
                if let Some(bracket_same_line) = js_formatter.bracket_same_line {
                    options.set_bracket_same_line(bracket_same_line);
                }
            }

            options
        })
    }

    /// It scans the current override rules and return the formatting options that of the first override is matched
    pub fn override_json_format_options(
        &self,
        path: &Path,
        options: JsonFormatOptions,
    ) -> JsonFormatOptions {
        self.patterns.iter().fold(options, |mut options, pattern| {
            let included = !pattern.include.is_empty() && pattern.include.matches_path(path);
            let excluded = !pattern.exclude.is_empty() && pattern.exclude.matches_path(path);
            if excluded {
                return options;
            }
            if included {
                let json_formatter = &pattern.languages.json.formatter;

                if let Some(indent_style) = json_formatter
                    .indent_style
                    .or(pattern.formatter.indent_style)
                {
                    options.set_indent_style(indent_style);
                }

                if let Some(indent_width) = json_formatter
                    .indent_width
                    .or(pattern.formatter.indent_width)
                {
                    options.set_indent_width(indent_width)
                }
                if let Some(line_width) = json_formatter.line_width.or(pattern.formatter.line_width)
                {
                    options.set_line_width(line_width);
                }
            }

            options
        })
    }

    /// It scans the current override rules and return the formatting options that of the first override is matched
    pub fn override_css_format_options(
        &self,
        path: &Path,
        options: CssFormatOptions,
    ) -> CssFormatOptions {
        self.patterns.iter().fold(options, |mut options, pattern| {
            let included = !pattern.include.is_empty() && pattern.include.matches_path(path);
            let excluded = !pattern.exclude.is_empty() && pattern.exclude.matches_path(path);
            if excluded {
                return options;
            }
            if included {
                let css_formatter = &pattern.languages.css.formatter;
                let formatter = &pattern.formatter;

                if let Some(indent_style) = css_formatter.indent_style.or(formatter.indent_style) {
                    options.set_indent_style(indent_style);
                }
                if let Some(indent_width) = css_formatter.indent_width.or(formatter.indent_width) {
                    options.set_indent_width(indent_width)
                }
                if let Some(line_width) = css_formatter.line_width.or(formatter.line_width) {
                    options.set_line_width(line_width);
                }
                if let Some(quote_style) = css_formatter.quote_style {
                    options.set_quote_style(quote_style);
                }
            }

            options
        })
    }

    pub fn override_js_parser_options(
        &self,
        path: &Path,
        options: JsParserOptions,
    ) -> JsParserOptions {
        self.patterns.iter().fold(options, |mut options, pattern| {
            let included = !pattern.include.is_empty() && pattern.include.matches_path(path);
            let excluded = !pattern.exclude.is_empty() && pattern.exclude.matches_path(path);
            if excluded {
                return options;
            }
            if included {
                let js_parser = &pattern.languages.javascript.parser;

                options.parse_class_parameter_decorators =
                    js_parser.parse_class_parameter_decorators;
            }
            options
        })
    }

    pub fn as_json_parser_options(&self, path: &Path) -> Option<JsonParserOptions> {
        for pattern in &self.patterns {
            let included = !pattern.include.is_empty() && pattern.include.matches_path(path);
            let excluded = !pattern.exclude.is_empty() && pattern.exclude.matches_path(path);

            if included || !excluded {
                let json_parser = &pattern.languages.json.parser;

                return Some(JsonParserOptions {
                    allow_comments: json_parser.allow_comments,
                    allow_trailing_commas: json_parser.allow_trailing_commas,
                });
            }
        }

        None
    }

    pub fn as_css_parser_options(&self, path: &Path) -> Option<CssParserOptions> {
        for pattern in &self.patterns {
            let included = !pattern.include.is_empty() && pattern.include.matches_path(path);
            let excluded = !pattern.exclude.is_empty() && pattern.exclude.matches_path(path);

            if included || !excluded {
                let css_parser = &pattern.languages.css.parser;

                return Some(CssParserOptions {
                    allow_wrong_line_comments: css_parser.allow_wrong_line_comments,
                });
            }
        }

        None
    }

    /// Retrieves the options of lint rules that have been overridden
    pub fn override_analyzer_rules(
        &self,
        path: &Path,
        analyzer_rules: AnalyzerRules,
    ) -> AnalyzerRules {
        self.patterns
            .iter()
            .fold(analyzer_rules, |mut analyzer_rules, pattern| {
                let excluded = !pattern.exclude.is_empty() && pattern.exclude.matches_path(path);
                if !excluded && !pattern.include.is_empty() && pattern.include.matches_path(path) {
                    if let Some(rules) = pattern.linter.rules.as_ref() {
                        push_to_analyzer_rules(rules, metadata(), &mut analyzer_rules);
                    }
                }

                analyzer_rules
            })
    }

    /// Scans the overrides and checks if there's an override that disable the formatter for `path`
    pub fn formatter_disabled(&self, path: &Path) -> Option<bool> {
        for pattern in &self.patterns {
            if pattern.exclude.matches_path(path) {
                continue;
            }
            if !pattern.include.is_empty() && pattern.include.matches_path(path) {
                if let Some(enabled) = pattern.formatter.enabled {
                    return Some(!enabled);
                }
                continue;
            }
        }
        None
    }

    /// Scans the overrides and checks if there's an override that disable the linter for `path`
    pub fn linter_disabled(&self, path: &Path) -> Option<bool> {
        for pattern in &self.patterns {
            if pattern.exclude.matches_path(path) {
                continue;
            }
            if !pattern.include.is_empty() && pattern.include.matches_path(path) {
                if let Some(enabled) = pattern.linter.enabled {
                    return Some(!enabled);
                }
                continue;
            }
        }
        None
    }

    /// Scans the overrides and checks if there's an override that disable the organize imports for `path`
    pub fn organize_imports_disabled(&self, path: &Path) -> Option<bool> {
        for pattern in &self.patterns {
            if pattern.exclude.matches_path(path) {
                continue;
            }
            if !pattern.include.is_empty() && pattern.include.matches_path(path) {
                if let Some(enabled) = pattern.organize_imports.enabled {
                    return Some(!enabled);
                }
                continue;
            }
        }
        None
    }
}
#[derive(Debug)]
pub struct OverrideSettingPattern {
    pub exclude: Matcher,
    pub include: Matcher,
    /// Formatter settings applied to all files in the workspaces
    pub formatter: OverrideFormatSettings,
    /// Linter settings applied to all files in the workspace
    pub linter: OverrideLinterSettings,
    /// Linter settings applied to all files in the workspace
    pub organize_imports: OverrideOrganizeImportsSettings,
    /// Language specific settings
    pub languages: LanguageListSettings,
}

/// Creates a [Matcher] from a [StringSet]
///
/// ## Errors
///
/// It can raise an error if the patterns aren't valid
pub fn to_matcher(
    working_directory: Option<PathBuf>,
    string_set: Option<&StringSet>,
) -> Result<Matcher, WorkspaceError> {
    let mut matcher = Matcher::empty();
    if let Some(working_directory) = working_directory {
        matcher.set_root(working_directory)
    }
    if let Some(string_set) = string_set {
        for pattern in string_set.iter() {
            matcher.add_pattern(pattern).map_err(|err| {
                WorkspaceError::Configuration(ConfigurationDiagnostic::new_invalid_ignore_pattern(
                    pattern.to_string(),
                    err.msg.to_string(),
                ))
            })?;
        }
    }
    Ok(matcher)
}

fn to_git_ignore(path: PathBuf, matches: &[String]) -> Result<Gitignore, WorkspaceError> {
    let mut gitignore_builder = GitignoreBuilder::new(path.clone());

    for the_match in matches {
        gitignore_builder
            .add_line(Some(path.clone()), the_match)
            .map_err(|err| {
                WorkspaceError::Configuration(ConfigurationDiagnostic::InvalidIgnorePattern(
                    InvalidIgnorePattern {
                        message: err.to_string(),
                    },
                ))
            })?;
    }
    let gitignore = gitignore_builder.build().map_err(|err| {
        WorkspaceError::Configuration(ConfigurationDiagnostic::InvalidIgnorePattern(
            InvalidIgnorePattern {
                message: err.to_string(),
            },
        ))
    })?;
    Ok(gitignore)
}
