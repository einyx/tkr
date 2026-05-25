use jkr_api::{
    capability,
    host::Host,
    manifest::Manifest,
    plugin::{CommandCtx, FilterDecision, Plugin},
    FilterResult,
};
// ApiResult (jkr_api::Error) — used for Plugin trait method return types.
use jkr_api::Result as ApiResult;
// anyhow::Result — used for constructors that parse TOML.
use anyhow::Result;

use crate::FilterPlugin;

/// V2 wrapper around the existing `FilterPlugin` rules engine.
///
/// Holds a `FilterPlugin` (which owns all the compiled `FilterGroup`s) and
/// implements the new `jkr_api::plugin::Plugin` trait by delegating to it.
/// The legacy impl on `FilterPlugin` itself is untouched.
pub struct FilterPluginV2 {
    inner: FilterPlugin,
}

impl FilterPluginV2 {
    /// Construct from an existing `FilterPlugin` (already loaded with rules).
    pub fn new(inner: FilterPlugin) -> Self {
        Self { inner }
    }

    /// Convenience: parse a TOML string exactly like `FilterPlugin::from_toml`.
    pub fn from_toml(toml_str: &str) -> Result<Self> {
        Ok(Self::new(FilterPlugin::from_toml(toml_str)?))
    }

    /// Convenience: load all `.toml` files from a directory.
    pub fn from_dir(dir: &std::path::Path) -> Result<Self> {
        Ok(Self::new(FilterPlugin::from_dir(dir)?))
    }
}

impl Plugin for FilterPluginV2 {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "jkr-filter".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities_required: vec![capability::STDOUT_FILTER.into()],
            ..Default::default()
        }
    }

    fn on_load(&mut self, _host: std::sync::Arc<dyn Host>) -> ApiResult<()> {
        Ok(())
    }

    // on_command_begin: the rules engine is stateless w.r.t. command identity
    // (it derives command/subcommand from each on_line call), so nothing to do.
    fn on_command_begin(&mut self, _ctx: &CommandCtx) -> ApiResult<()> {
        Ok(())
    }

    fn on_line(&mut self, line: &str, ctx: &CommandCtx) -> ApiResult<FilterDecision> {
        use jkr_api::LegacyPlugin as LegacyPluginTrait;
        let result = self
            .inner
            .filter(line, &ctx.command, &ctx.args, ctx.line_index);
        // The legacy FilterResult's Replace and Annotate variants carry raw
        // C-string pointers (used only in the C-ABI path).  The built-in Rust
        // rules engine (rules.rs) never produces those variants — it returns
        // only Pass or Suppress.  We handle all arms defensively: the pointer
        let decision = match result {
            FilterResult::Pass => FilterDecision::Pass,
            FilterResult::Suppress => FilterDecision::Suppress,
            FilterResult::SuppressWithNote(n) => FilterDecision::SuppressWithNote(n),
            FilterResult::Replace(s) => FilterDecision::Replace(s),
            FilterResult::Annotate(_) => FilterDecision::Pass,
        };
        Ok(decision)
    }

    fn on_command_end(&mut self, _ctx: &CommandCtx) -> ApiResult<String> {
        use jkr_api::LegacyPlugin as LegacyPluginTrait;
        Ok(self.inner.flush())
    }
}

/// Return all built-in filter plugins wrapped in the v2 `Plugin` trait.
///
/// Currently there is a single global plugin (rules branch on command/subcommand
/// internally), so this returns a one-element `Vec`.  Task 6.3 registers these
/// with the host registry.
pub fn all_filters_v2() -> Vec<Box<dyn Plugin>> {
    vec![Box::new(FilterPluginV2::new(FilterPlugin::new()))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use jkr_api::plugin::{CommandCtx, FilterDecision};

    fn ctx(command: &str, args: &str) -> CommandCtx {
        CommandCtx {
            command: command.into(),
            args: args.into(),
            line_index: 0,
        }
    }

    // Mirrors lib.rs legacy test `suppress_prefix_drops_matching_line`
    #[test]
    fn v2_suppress_prefix_parity() {
        let mut p = FilterPluginV2::from_toml(
            r#"command = "git"
[[rules]]
type = "suppress_prefix"
prefix = "warning:"
"#,
        )
        .unwrap();

        let dec = p.on_line("warning: something", &ctx("git", "")).unwrap();
        assert_eq!(dec, FilterDecision::Suppress);

        let dec = p.on_line("modified: file.rs", &ctx("git", "")).unwrap();
        assert_eq!(dec, FilterDecision::Pass);
    }

    // Mirrors lib.rs legacy test `keep_regex_suppresses_non_matching`
    #[test]
    fn v2_keep_regex_parity() {
        let mut p = FilterPluginV2::from_toml(
            r#"command = "cargo"
[[rules]]
type = "keep_regex"
pattern = "^error"
"#,
        )
        .unwrap();

        let dec = p
            .on_line("error[E0001]: blah", &ctx("cargo", "build"))
            .unwrap();
        assert_eq!(dec, FilterDecision::Pass);

        let dec = p
            .on_line("Compiling foo v0.1.0", &ctx("cargo", "build"))
            .unwrap();
        assert_eq!(dec, FilterDecision::Suppress);
    }

    // Mirrors lib.rs legacy test `group_only_applies_to_matching_command`
    #[test]
    fn v2_command_scoping_parity() {
        let mut p = FilterPluginV2::from_toml(
            r#"command = "cargo"
[[rules]]
type = "suppress_prefix"
prefix = "warning:"
"#,
        )
        .unwrap();

        // git command: rule must be skipped
        let dec = p.on_line("warning: foo", &ctx("git", "status")).unwrap();
        assert_eq!(dec, FilterDecision::Pass);

        // cargo command: rule fires
        let dec = p.on_line("warning: foo", &ctx("cargo", "build")).unwrap();
        assert_eq!(dec, FilterDecision::Suppress);
    }

    // Mirrors lib.rs legacy test `group_only_applies_to_matching_subcommand`
    #[test]
    fn v2_subcommand_scoping_parity() {
        let mut p = FilterPluginV2::from_toml(
            r#"command = "cargo"
subcommands = ["test"]
[[rules]]
type = "suppress_regex"
pattern = "^test result: ok"
"#,
        )
        .unwrap();

        let dec = p
            .on_line("test result: ok. 1 passed", &ctx("cargo", "build"))
            .unwrap();
        assert_eq!(dec, FilterDecision::Pass);

        let dec = p
            .on_line("test result: ok. 1 passed", &ctx("cargo", "test"))
            .unwrap();
        assert_eq!(dec, FilterDecision::Suppress);
    }

    // on_command_end delegates to legacy flush (currently returns empty string)
    #[test]
    fn v2_on_command_end_returns_flush_output() {
        let mut p = FilterPluginV2::from_toml(
            r#"command = "git"
[[rules]]
type = "suppress_prefix"
prefix = "warning:"
"#,
        )
        .unwrap();
        let summary = p.on_command_end(&ctx("git", "status")).unwrap();
        assert_eq!(summary, "");
    }

    // manifest reports correct name and capability
    #[test]
    fn v2_manifest() {
        let p = FilterPluginV2::from_toml(
            r#"[[rules]]
type = "suppress_prefix"
prefix = "x"
"#,
        )
        .unwrap();
        let m = p.manifest();
        assert_eq!(m.name, "jkr-filter");
        assert!(m
            .capabilities_required
            .contains(&jkr_api::capability::STDOUT_FILTER.to_string()));
    }
}
