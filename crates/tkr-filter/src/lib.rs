mod rules;
pub mod v2;
pub use v2::all_filters_v2;

use anyhow::Result;
use rules::{CompiledRule, Rule};
use serde::Deserialize;
use tkr_api::{FilterResult, LegacyPlugin as Plugin};

#[derive(Debug, Deserialize)]
struct FilterDef {
    /// If set, group only applies when proxied command matches.
    /// Example: "cargo" matches `tkr cargo build`. Omit to match any command.
    command: Option<String>,
    /// If non-empty, group only applies when first arg matches one of these.
    /// Example: ["build", "test"] for cargo. Empty/omitted = any subcommand.
    #[serde(default)]
    subcommands: Vec<String>,
    #[serde(default)]
    rules: Vec<Rule>,
}

/// One group of rules scoped to a (command, subcommands) selector.
pub struct FilterGroup {
    command: Option<String>,
    subcommands: Vec<String>,
    rules: Vec<CompiledRule>,
}

impl FilterGroup {
    fn matches(&self, command: &str, subcommand: &str) -> bool {
        if let Some(c) = &self.command {
            if c != command {
                return false;
            }
        }
        if !self.subcommands.is_empty() && !self.subcommands.iter().any(|s| s == subcommand) {
            return false;
        }
        true
    }
}

pub struct FilterPlugin {
    groups: Vec<FilterGroup>,
}

impl Default for FilterPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FilterPlugin {
    pub fn new() -> Self {
        Self { groups: vec![] }
    }

    pub fn from_toml(toml_str: &str) -> Result<Self> {
        let def: FilterDef = toml::from_str(toml_str)?;
        let group = FilterGroup {
            command: def.command,
            subcommands: def.subcommands,
            rules: def
                .rules
                .into_iter()
                .map(|r| r.compile())
                .collect::<Result<Vec<_>>>()?,
        };
        Ok(Self {
            groups: vec![group],
        })
    }

    pub fn from_dir(dir: &std::path::Path) -> Result<Self> {
        let mut plugin = Self { groups: Vec::new() };
        plugin.load_dir(dir)?;
        Ok(plugin)
    }

    /// Append all `.toml` filter groups from `dir` to this plugin.
    pub fn load_dir(&mut self, dir: &std::path::Path) -> Result<()> {
        if dir.is_dir() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "toml") {
                    let text = std::fs::read_to_string(&path)?;
                    let def: FilterDef = toml::from_str(&text)?;
                    let rules = def
                        .rules
                        .into_iter()
                        .map(|r| r.compile())
                        .collect::<Result<Vec<_>>>()?;
                    self.groups.push(FilterGroup {
                        command: def.command,
                        subcommands: def.subcommands,
                        rules,
                    });
                }
            }
        }
        Ok(())
    }

    /// Append filter groups from a single TOML file.
    /// Returns Ok(()) and does nothing if the file doesn't exist.
    pub fn load_file(&mut self, path: &std::path::Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        let text = std::fs::read_to_string(path)?;
        let def: FilterDef = toml::from_str(&text)?;
        let rules = def
            .rules
            .into_iter()
            .map(|r| r.compile())
            .collect::<Result<Vec<_>>>()?;
        self.groups.push(FilterGroup {
            command: def.command,
            subcommands: def.subcommands,
            rules,
        });
        Ok(())
    }

    /// Load only the filter file that matches `cmd_name` from `dir`.
    /// Equivalent to `load_file(dir/<cmd_name>.toml)`.
    pub fn load_for_command(&mut self, dir: &std::path::Path, cmd_name: &str) -> Result<()> {
        let path = dir.join(format!("{cmd_name}.toml"));
        self.load_file(&path)
    }
}

impl Plugin for FilterPlugin {
    fn init(_config: &str) -> Box<dyn Plugin>
    where
        Self: Sized,
    {
        Box::new(FilterPlugin { groups: vec![] })
    }

    fn filter(&mut self, line: &str, command: &str, args: &str, _index: u64) -> FilterResult {
        let subcommand = args.split_whitespace().next().unwrap_or("");
        for group in &mut self.groups {
            if !group.matches(command, subcommand) {
                continue;
            }
            for rule in &mut group.rules {
                if let Some(result) = rule.apply(line) {
                    return result;
                }
            }
        }
        FilterResult::Pass
    }

    fn flush(&mut self) -> String {
        let mut out = String::new();
        for group in &mut self.groups {
            for rule in &mut group.rules {
                if let Some(s) = rule.flush_summary() {
                    if !s.is_empty() {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(&s);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(toml: &str) -> FilterPlugin {
        FilterPlugin::from_toml(toml).unwrap()
    }

    #[test]
    fn suppress_prefix_drops_matching_line() {
        let mut p = make(
            r#"command = "git"
[[rules]]
type = "suppress_prefix"
prefix = "warning:"
"#,
        );
        assert_eq!(
            p.filter("warning: something", "git", "", 0),
            FilterResult::Suppress
        );
        assert_eq!(
            p.filter("modified: file.rs", "git", "", 1),
            FilterResult::Pass
        );
    }

    #[test]
    fn suppress_regex_drops_blank_lines() {
        let mut p = make(
            r#"command = "git"
[[rules]]
type = "suppress_regex"
pattern = "^\\s*$"
"#,
        );
        assert_eq!(p.filter("   ", "git", "", 0), FilterResult::Suppress);
        assert_eq!(p.filter("content", "git", "", 1), FilterResult::Pass);
    }

    #[test]
    fn keep_regex_suppresses_non_matching() {
        let mut p = make(
            r#"command = "cargo"
[[rules]]
type = "keep_regex"
pattern = "^error"
"#,
        );
        assert_eq!(
            p.filter("error[E0001]: blah", "cargo", "build", 0),
            FilterResult::Pass
        );
        assert_eq!(
            p.filter("Compiling foo v0.1.0", "cargo", "build", 1),
            FilterResult::Suppress
        );
    }

    #[test]
    fn group_only_applies_to_matching_command() {
        // Group scoped to cargo — must NOT fire for git
        let mut p = make(
            r#"command = "cargo"
[[rules]]
type = "suppress_prefix"
prefix = "warning:"
"#,
        );
        // git command: rule should be skipped
        assert_eq!(
            p.filter("warning: foo", "git", "status", 0),
            FilterResult::Pass
        );
        // cargo command: rule fires
        assert_eq!(
            p.filter("warning: foo", "cargo", "build", 0),
            FilterResult::Suppress
        );
    }

    #[test]
    fn group_only_applies_to_matching_subcommand() {
        let mut p = make(
            r#"command = "cargo"
subcommands = ["test"]
[[rules]]
type = "suppress_regex"
pattern = "^test result: ok"
"#,
        );
        // cargo build: rule should NOT fire
        assert_eq!(
            p.filter("test result: ok. 1 passed", "cargo", "build", 0),
            FilterResult::Pass
        );
        // cargo test: rule fires
        assert_eq!(
            p.filter("test result: ok. 1 passed", "cargo", "test", 0),
            FilterResult::Suppress
        );
    }

    #[test]
    fn empty_subcommands_matches_any_subcommand() {
        let mut p = make(
            r#"command = "cargo"
[[rules]]
type = "suppress_regex"
pattern = "^Compiling"
"#,
        );
        assert_eq!(
            p.filter("Compiling x", "cargo", "anything", 0),
            FilterResult::Suppress
        );
    }

    #[test]
    fn flush_returns_empty_string_when_no_rules_emit_summary() {
        let mut p = make(
            r#"command = "git"
[[rules]]
type = "suppress_prefix"
prefix = "warning:"
"#,
        );
        p.filter("warning: x", "git", "", 0);
        assert_eq!(p.flush(), "");
    }

    #[test]
    fn empty_result_substitute_integrates_with_plugin_flush() {
        let mut p = make(
            r#"command = "grep"
[[rules]]
type = "suppress_regex"
pattern = "noisy"

[[rules]]
type = "empty_result_substitute"
message = "0 matches"
"#,
        );
        p.filter("noisy line 1", "grep", "", 0);
        p.filter("noisy line 2", "grep", "", 1);
        assert_eq!(p.flush(), "0 matches");
    }

    #[test]
    fn grep_filter_pack_loads_and_groups() {
        let toml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../filters/grep.toml"),
        )
        .expect("read grep.toml");
        let mut p = FilterPlugin::from_toml(&toml).expect("parse");
        p.filter("src/lib.rs:42:fn foo()", "grep", "", 0);
        p.filter("src/lib.rs:99:fn bar()", "grep", "", 1);
        let flushed = p.flush();
        assert!(flushed.contains("grep matches by file:"), "got {flushed:?}");
        assert!(flushed.contains("src/lib.rs"), "got {flushed:?}");
    }

    #[test]
    fn grep_filter_pack_emits_empty_marker() {
        let toml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../filters/grep.toml"),
        )
        .expect("read grep.toml");
        let mut p = FilterPlugin::from_toml(&toml).expect("parse");
        let flushed = p.flush();
        assert!(flushed.contains("0 grep matches"), "got {flushed:?}");
    }

    #[test]
    fn find_filter_pack_groups_by_directory() {
        let toml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../filters/find.toml"),
        )
        .expect("read find.toml");
        let mut p = FilterPlugin::from_toml(&toml).expect("parse");
        p.filter("./src/foo.rs", "find", "", 0);
        p.filter("./src/bar.rs", "find", "", 1);
        let flushed = p.flush();
        assert!(flushed.contains("find results by directory:"), "got {flushed:?}");
        assert!(flushed.contains("./src"), "got {flushed:?}");
    }
}
