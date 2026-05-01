use crate::config::Config;
use crate::stream::chars_to_tokens;
use anyhow::Result;

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

    // Load only the per-command filter TOML (lazy, cached).
    // This is the key optimization: instead of parsing all ~90 bundled TOML files
    // at boot, we load only the one relevant to the current command.
    let filter_arc = crate::host::boot::filter_for_command(cmd_name);
    let mut filter_guard = filter_arc.lock().unwrap();

    let str_args: Vec<&str> = cmd_args.iter().map(String::as_str).collect();
    let lines = crate::runner::stream_command(cmd, &str_args)?;

    // Run the fast pipeline: uses the per-command FilterPlugin directly,
    // bypassing the registry's (empty) filter plugin.
    let result = crate::stream::run_pipeline_direct(lines, &mut filter_guard, cmd, &cmd_args_str);

    let tokens_in = chars_to_tokens(result.chars_in);
    let tokens_saved = chars_to_tokens(result.chars_suppressed);
    sess.command_end(tokens_in, tokens_saved);

    // Flush the per-run signature buffer into the vault-backed analytics store.
    let subcmd = first_positional(cmd_args);
    let key = format!("{cmd_name} {subcmd}").trim().to_string();
    let buf = crate::stream::take_signature_buffer();

    // Vault-backed analytics writes are age-encrypted per row — doing them
    // synchronously here adds ~1s per signature in the user-facing path.
    // Hand the buffer to a detached thread so the command exits immediately.
    // Analytics are best-effort, not critical to correctness.
    // The detached thread calls ensure_full() (lazy) to obtain a real vault.
    // Always upsert command_stats so `tkr gain` shows real numbers without
    // requiring `tkr watch` to be running. Spawn a detached thread so the
    // user-facing command exits immediately; analytics are best-effort.
    {
        // Detached analytics write. One vault decrypt+re-encrypt costs ~5s,
        // so doing it inline would regress every command from 15ms to 7s.
        // Detached, the writer races with process exit — short-lived commands
        // (`tkr true`, `tkr cargo --version`) may lose one analytics row.
        // Long-running commands (the ones that move tokens-saved) land
        let _ = buf; // unused until noise-batching lands
        let bus = host.bus.clone();
        let key_owned = key.clone();
        let chars_in = result.chars_in;
        let chars_saved = result.chars_suppressed;
        // Join instead of detach: XChaCha20-Poly1305 vault writes are ~1ms,
        // so this is safe. Detached threads are killed on process exit before
        // the write completes, causing 'tkr gain' to always show no data.
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

    Ok(())
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
