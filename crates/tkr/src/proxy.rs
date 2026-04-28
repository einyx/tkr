use crate::config::Config;
use crate::stream::chars_to_tokens;
use anyhow::Result;
use tkr_api::plugin::CommandCtx;

/// Flags whose argument is the next token (so we skip both).
/// Conservative — for unknown flags we treat the next token as the value
/// only if the flag form has no `=`.
const VALUE_FLAGS: &[&str] = &[
    "-C",     // git -C <path>
    "-c",     // git -c <key=val>
    "-p",     // cargo -p <pkg>, kubectl -p, npm -p
    "--package",
    "--config",
    "--manifest-path",
    "--target",
    "-f",
    "-n",     // kubectl -n <ns>
    "--namespace",
    "--context",
    "-o",     // kubectl -o <fmt>
    "--output",
    "--format",
    "-l",
    "--label",
    "--profile",
    "--region",
];

/// Pick the first positional argument from `args`, skipping flags and their
/// values. Returns "" if there is no positional arg.
fn first_positional(args: &[String]) -> &str {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a.is_empty() {
            i += 1;
            continue;
        }
        if a.starts_with("--") && a.contains('=') {
            // --foo=bar — single token, skip
            i += 1;
            continue;
        }
        if a.starts_with('-') {
            // Flag. If known to take a value (and no `=`), also skip the next token.
            if VALUE_FLAGS.iter().any(|f| *f == a) {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        return a;
    }
    ""
}

pub fn run(cfg: Config, args: &[String]) -> Result<()> {
    let (cmd, cmd_args) = args.split_first().expect("at least one arg");
    let cmd_args_str = cmd_args.join(" ");

    let sess = crate::session::Session::connect(&cfg.core.socket_path);
    sess.command_start(cmd, &cmd_args_str);

    // Use the v2 registry for filter+analytics.
    let host = crate::host::boot::get_host();
    let registry = &host.registry;

    // Build the legacy semantic plugin chain (semantic dedup remains a legacy plugin).
    let mut semantic_chain = build_semantic_chain(&cfg)?;

    let ctx = CommandCtx {
        command: cmd.clone(),
        args: cmd_args_str.clone(),
        line_index: 0,
    };

    // Signal command begin to all v2 filter plugins.
    registry.filters_command_begin(&ctx);

    let str_args: Vec<&str> = cmd_args.iter().map(String::as_str).collect();
    let lines = crate::runner::stream_command(cmd, &str_args)?;

    let result = crate::stream::run_pipeline_v2(lines, registry, &mut semantic_chain, cmd, &cmd_args_str);

    // Signal command end — collects summaries from v2 filter plugins.
    let summary = registry.filters_command_end(&ctx);
    if !summary.is_empty() {
        for line in summary.lines() {
            println!("{line}");
        }
    }

    let tokens_in = chars_to_tokens(result.chars_in);
    let tokens_saved = chars_to_tokens(result.chars_suppressed);
    sess.command_end(tokens_in, tokens_saved);

    // Flush the per-run signature buffer into the legacy analytics store so
    // `tkr gain` (which reads the legacy DB) continues to work.
    // Analytics savings are also recorded via AnalyticsPluginV2 in the v2 registry
    // (via on_command_end above), but that DB is vault-backed. The legacy
    // analytics.db path is kept alive for `tkr gain` until we migrate that command.
    let db_path = &cfg.plugins.analytics.db_path;
    if let Some(parent) = std::path::Path::new(db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(store) = tkr_analytics::AnalyticsStore::open(db_path) {
        let subcmd = first_positional(cmd_args);
        let cmd_name = std::path::Path::new(cmd.as_str())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(cmd.as_str());
        let _ = store.record(cmd_name, subcmd, result.chars_in, result.chars_suppressed);

        // Flush the per-run signature buffer into the noise table.
        let key = format!("{cmd_name} {subcmd}").trim().to_string();
        for (_buf_cmd, sig, entry) in crate::stream::take_signature_buffer() {
            let _ = store.record_signature(
                &key,
                &sig,
                &entry.sample,
                entry.occurrences,
                entry.total_chars,
            );
        }
    }

    Ok(())
}

/// Build the semantic-only legacy plugin chain (no filter, no analytics — those
/// are handled by the v2 registry now).
fn build_semantic_chain(cfg: &Config) -> Result<Vec<Box<dyn tkr_api::LegacyPlugin>>> {
    use tkr_api::LegacyPlugin as Plugin;
    let mut chain: Vec<Box<dyn Plugin>> = Vec::new();

    if cfg.plugins.chain.iter().any(|p| p == "tkr-semantic") {
        let sc = tkr_semantic::SemanticConfig {
            dedup_threshold: cfg.plugins.semantic.dedup_threshold,
            relevance_threshold: cfg.plugins.semantic.relevance_threshold,
            window_size: cfg.plugins.semantic.window_size,
            emit_summaries: cfg.plugins.semantic.emit_summaries,
            ollama_url: cfg.plugins.semantic.ollama_url.clone(),
            ollama_model: cfg.plugins.semantic.ollama_model.clone(),
        };
        chain.push(Box::new(tkr_semantic::SemanticPlugin::new(&sc)));
    }

    Ok(chain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn skips_short_value_flag() {
        assert_eq!(first_positional(&s(&["-C", "/path", "status"])), "status");
    }

    #[test]
    fn skips_long_eq_flag() {
        assert_eq!(first_positional(&s(&["--target=foo", "build"])), "build");
    }

    #[test]
    fn skips_unknown_short_flag_alone() {
        assert_eq!(first_positional(&s(&["-x", "test"])), "test");
    }

    #[test]
    fn empty_when_only_flags() {
        assert_eq!(first_positional(&s(&["--version"])), "");
    }

    #[test]
    fn returns_first_positional() {
        assert_eq!(first_positional(&s(&["status", "--short"])), "status");
    }
}
