use crate::config::Config;
use crate::stream::{chars_to_tokens, PipelineResult};
use anyhow::{Context, Result};

/// Flags whose argument is the next token (so we skip both).
/// Conservative — for unknown flags we treat the next token as the value
/// only if the flag form has no `=`.
const VALUE_FLAGS: &[&str] = &[
    "-C", // git -C <path>
    "-c", // git -c <key=val>
    "-p", // cargo -p <pkg>, kubectl -p, npm -p
    "--package",
    "--config",
    "--manifest-path",
    "--target",
    "-f",
    "-n", // kubectl -n <ns>
    "--namespace",
    "--context",
    "-o", // kubectl -o <fmt>
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
            if VALUE_FLAGS.contains(&a) {
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
    let safe_prefix = sanitize_prefix(&cfg.core.output_prefix);
    if !safe_prefix.is_empty() {
        std::env::set_var("TKR_OUTPUT_PREFIX", safe_prefix);
    } else {
        std::env::remove_var("TKR_OUTPUT_PREFIX");
    }

    let (cmd, cmd_args) = args.split_first().expect("at least one arg");
    let cmd_args_str = cmd_args.join(" ");

    let sess = crate::session::Session::connect(&cfg.core.socket_path);
    sess.command_start(cmd, &cmd_args_str);

    // Derive the bare command name (e.g. "git" from "/usr/bin/git").
    let cmd_name = std::path::Path::new(cmd.as_str())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd.as_str());

    // Fast boot: filter-only, no keychain/vault/analytics.
    let host = crate::host::boot::ensure()?;

    // RTK-style native handlers: full output capture + structured compression
    // for grep/rg (see `native::try_run`). Disable with `TKR_NATIVE_GREP=0`.
    if let Some(native) = crate::native::try_run(cmd, cmd_args)? {
        let subcmd = first_positional(cmd_args);
        let key = format!("{cmd_name} {subcmd}").trim().to_string();
        record_pipeline_stats(&sess, host, &key, &native.pipeline);
        crate::native::log_session_line(cmd, cmd_args, &native.pipeline, native.exit_code);
        std::process::exit(native.exit_code);
    }

    let filter_arc = crate::host::boot::filter_for_command(cmd_name);
    let mut filter_guard = filter_arc.lock().unwrap();

    let str_args: Vec<&str> = cmd_args.iter().map(String::as_str).collect();
    let lines = crate::runner::stream_command(cmd, &str_args)?;

    let capture = crate::tee::capture_raw_transcript();
    let mut raw_transcript = String::new();
    let (result, code) = crate::stream::run_pipeline_direct(
        lines,
        &mut filter_guard,
        cmd,
        &cmd_args_str,
        capture.then_some(&mut raw_transcript),
    )
    .context("tkr proxy: filtering subprocess output")?;

    let subcmd = first_positional(cmd_args);
    let key = format!("{cmd_name} {subcmd}").trim().to_string();
    let buf = crate::stream::take_signature_buffer();
    let _ = buf;

    record_pipeline_stats(&sess, host, &key, &result);

    match crate::tee::maybe_save_transcript(cmd, &cmd_args_str, code, &raw_transcript) {
        Ok(Some(note)) => println!("{note}"),
        Ok(None) => {}
        Err(e) => eprintln!("tkr: warning: tee save failed: {e}"),
    }

    std::process::exit(code);
}

/// Session summary + vault analytics row for one proxied command (shared by
/// native and TOML filter pipelines).
fn record_pipeline_stats(
    sess: &crate::session::Session,
    host: &crate::host::boot::HostHandle,
    key: &str,
    result: &PipelineResult,
) {
    let tokens_in = chars_to_tokens(result.chars_in);
    let tokens_saved = chars_to_tokens(result.chars_suppressed);
    sess.command_end(tokens_in, tokens_saved);

    let bus = host.bus.clone();
    let key_owned = key.to_string();
    let chars_in = result.chars_in;
    let chars_saved = result.chars_suppressed;
    let handle = std::thread::spawn(move || {
        let vault = crate::host::boot::vault();
        let analytics_host = crate::host::RealHost::new("tkr-analytics", vault, bus);
        if let Err(e) = tkr_analytics::record_command_stat_via_host(
            &analytics_host,
            &key_owned,
            chars_in as u64,
            chars_saved as u64,
        ) {
            eprintln!("tkr: warning: could not save analytics for `{key_owned}`: {e}");
        }
    });
    if let Err(e) = handle.join() {
        eprintln!("tkr: warning: analytics writer thread panicked: {e:?}");
    }
}

fn sanitize_prefix(raw: &str) -> String {
    // `output_prefix = "tkr"` was an old default that makes `tkr ls` render
    // as `tkr <line>`, which is noisy for normal shell use. Treat that legacy
    // value as disabled so existing configs stop prefixing output lines.
    if raw.trim() == "tkr" {
        return String::new();
    }

    let mut out = String::with_capacity(raw.len().min(32));
    for ch in raw.chars() {
        if out.len() >= 32 {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':') {
            out.push(ch);
        }
    }
    out
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

    #[test]
    fn legacy_tkr_prefix_is_disabled() {
        assert_eq!(sanitize_prefix("tkr"), "");
        assert_eq!(sanitize_prefix(" tkr "), "");
    }

    #[test]
    fn custom_prefix_still_allowed() {
        assert_eq!(sanitize_prefix("cmd"), "cmd");
    }
}
