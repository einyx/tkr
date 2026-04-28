use crate::config::Config;
use crate::stream::chars_to_tokens;
use anyhow::Result;
use tkr_analytics::AnalyticsStore;

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

    let mut chain = crate::dispatch::build_chain(&cfg, cmd)?;
    let str_args: Vec<&str> = cmd_args.iter().map(String::as_str).collect();
    let lines = crate::runner::stream_command(cmd, &str_args)?;
    let result = crate::stream::run_pipeline(lines, &mut chain, cmd, &cmd_args_str);

    let tokens_in = chars_to_tokens(result.chars_in);
    let tokens_saved = chars_to_tokens(result.chars_suppressed);
    sess.command_end(tokens_in, tokens_saved);

    // Record savings to the analytics store. The analytics plugin's flush() can't
    // see suppressed lines (they short-circuit at the filter/semantic plugins),
    // so the proxy records the canonical totals here using PipelineResult.
    let db_path = &cfg.plugins.analytics.db_path;
    if let Some(parent) = std::path::Path::new(db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(store) = AnalyticsStore::open(db_path) {
        // Subcommand is the first positional arg, skipping flags and their
        // values (`git -C path status` → "status", `cargo --version` → "").
        let subcmd = first_positional(cmd_args);
        let _ = store.record(cmd, subcmd, result.chars_in, result.chars_suppressed);
    }

    Ok(())
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
