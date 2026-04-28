use regex::Regex;
use serde::Deserialize;
use tkr_api::FilterResult;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Rule {
    SuppressPrefix { prefix: String },
    SuppressRegex { pattern: String },
    KeepRegex { pattern: String },
}

pub struct CompiledRule(pub RuleKind);

pub enum RuleKind {
    SuppressPrefix(String),
    SuppressRegex(Regex),
    KeepRegex(Regex),
}

impl Rule {
    pub fn compile(self) -> anyhow::Result<CompiledRule> {
        match self {
            Rule::SuppressPrefix { prefix } => Ok(CompiledRule(RuleKind::SuppressPrefix(prefix))),
            Rule::SuppressRegex { pattern } => Ok(CompiledRule(RuleKind::SuppressRegex(Regex::new(&pattern)?))),
            Rule::KeepRegex { pattern } => Ok(CompiledRule(RuleKind::KeepRegex(Regex::new(&pattern)?))),
        }
    }
}

impl CompiledRule {
    pub fn apply(&self, line: &str) -> Option<FilterResult> {
        match &self.0 {
            RuleKind::SuppressPrefix(p) => if line.starts_with(p.as_str()) { Some(FilterResult::Suppress) } else { None },
            RuleKind::SuppressRegex(re) => if re.is_match(line) { Some(FilterResult::Suppress) } else { None },
            RuleKind::KeepRegex(re) => if re.is_match(line) { None } else { Some(FilterResult::Suppress) },
        }
    }
}
