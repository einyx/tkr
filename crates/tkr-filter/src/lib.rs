mod rules;

use anyhow::Result;
use rules::{CompiledRule, Rule};
use serde::Deserialize;
use tkr_api::{FilterResult, Plugin};

#[derive(Debug, Deserialize)]
struct FilterDef {
    #[allow(dead_code)]
    command: Option<String>,
    #[serde(default)]
    rules: Vec<Rule>,
}

pub struct FilterPlugin {
    rules: Vec<CompiledRule>,
}

impl FilterPlugin {
    pub fn from_toml(toml_str: &str) -> Result<Self> {
        let def: FilterDef = toml::from_str(toml_str)?;
        let rules = def.rules.into_iter().map(|r| r.compile()).collect::<Result<Vec<_>>>()?;
        Ok(Self { rules })
    }

    pub fn from_dir(dir: &std::path::Path) -> Result<Self> {
        let mut all_rules = Vec::new();
        if dir.is_dir() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "toml") {
                    let text = std::fs::read_to_string(&path)?;
                    let def: FilterDef = toml::from_str(&text)?;
                    for r in def.rules { all_rules.push(r.compile()?); }
                }
            }
        }
        Ok(Self { rules: all_rules })
    }
}

impl Plugin for FilterPlugin {
    fn init(_config: &str) -> Box<dyn Plugin> where Self: Sized {
        Box::new(FilterPlugin { rules: vec![] })
    }
    fn filter(&mut self, line: &str, _command: &str, _args: &str, _index: u64) -> FilterResult {
        for rule in &self.rules {
            if let Some(result) = rule.apply(line) { return result; }
        }
        FilterResult::Pass
    }
    fn flush(&mut self) -> String { String::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(toml: &str) -> FilterPlugin { FilterPlugin::from_toml(toml).unwrap() }

    #[test]
    fn suppress_prefix_drops_matching_line() {
        let mut p = make(r#"command = "git"
[[rules]]
type = "suppress_prefix"
prefix = "warning:"
"#);
        assert_eq!(p.filter("warning: something", "git", "", 0), FilterResult::Suppress);
        assert_eq!(p.filter("modified: file.rs", "git", "", 1), FilterResult::Pass);
    }

    #[test]
    fn suppress_regex_drops_blank_lines() {
        let mut p = make(r#"command = "git"
[[rules]]
type = "suppress_regex"
pattern = "^\\s*$"
"#);
        assert_eq!(p.filter("   ", "git", "", 0), FilterResult::Suppress);
        assert_eq!(p.filter("content", "git", "", 1), FilterResult::Pass);
    }

    #[test]
    fn keep_regex_suppresses_non_matching() {
        let mut p = make(r#"command = "cargo"
[[rules]]
type = "keep_regex"
pattern = "^error"
"#);
        assert_eq!(p.filter("error[E0001]: blah", "cargo", "", 0), FilterResult::Pass);
        assert_eq!(p.filter("Compiling foo v0.1.0", "cargo", "", 1), FilterResult::Suppress);
    }
}
