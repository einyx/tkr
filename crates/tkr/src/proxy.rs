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

    let ctx = CommandCtx {
        command: cmd.clone(),
        args: cmd_args_str.clone(),
        line_index: 0,
    };

    // Signal command begin to all v2 filter plugins.
    registry.filters_command_begin(&ctx);

    let str_args: Vec<&str> = cmd_args.iter().map(String::as_str).collect();
    let lines = crate::runner::stream_command(cmd, &str_args)?;

    let result = crate::stream::run_pipeline_v2(lines, registry, cmd, &cmd_args_str);

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

    // Flush the per-run signature buffer into the vault-backed analytics store.
    let subcmd = first_positional(cmd_args);
    let cmd_name = std::path::Path::new(cmd.as_str())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd.as_str());
    let key = format!("{cmd_name} {subcmd}").trim().to_string();
    let buf = crate::stream::take_signature_buffer();

    // Vault-backed analytics writes are age-encrypted per row — doing them
    // synchronously here adds ~1s per signature in the user-facing path.
    // Hand the buffer to a detached thread so the command exits immediately.
    // Analytics are best-effort, not critical to correctness.
    if !buf.is_empty() {
        let host_handle = crate::host::boot::get_host();
        let vault = host_handle.vault.clone();
        let bus = host_handle.bus.clone();
        let key_owned = key.clone();
        std::thread::spawn(move || {
            let analytics_host = crate::host::RealHost::new("tkr-analytics", vault, bus);
            for (_buf_cmd, sig, entry) in &buf {
                let _ = tkr_analytics::record_noise_signature_via_host(
                    &analytics_host,
                    &key_owned,
                    sig,
                    &entry.sample,
                    entry.occurrences,
                    entry.total_chars,
                );
            }
        });
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
