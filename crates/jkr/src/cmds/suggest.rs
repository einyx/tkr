//! `tkr suggest` — examine vault-backed analytics and suggest concrete filter
//! improvements for commands with low savings ratios. Surfaces:
//!   - high-volume / low-savings commands → which filter file to edit
//!   - commands tkr is recording but has no filter file for
//!   - rough estimate of additional tokens we could save

use crate::signature::signature_to_regex;
use crate::util::fmt_num;
use anyhow::Result;
use std::path::PathBuf;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

pub fn run() -> Result<()> {
    let vault = crate::host::boot::vault();
    let host_handle = crate::host::boot::get_host();
    let analytics_host =
        crate::host::RealHost::new("tkr-analytics", vault, host_handle.bus.clone());
    // Lazy: persist embeddings for any noise signatures missing them, so the
    // vault accumulates a vector index across sessions. No-op without the
    // `embeddings` cargo feature.
    let new_embeds = crate::embedding_ranker::embed_pending_signatures(&analytics_host, 256);
    if new_embeds > 0 {
        eprintln!("tkr suggest: embedded {new_embeds} new noise signatures into the vault");
    }

    let mut rows = tkr_analytics::total_savings_via_host(&analytics_host).unwrap_or_default();
    if rows.is_empty() {
        println!("No analytics rows yet.");
        return Ok(());
    }

    rows.sort_by(|a, b| {
        let unsaved_a = a.tokens_in.saturating_sub(a.tokens_saved);
        let unsaved_b = b.tokens_in.saturating_sub(b.tokens_saved);
        unsaved_b.cmp(&unsaved_a)
    });

    let on = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let p = |c: &'static str| if on { c } else { "" };

    println!();
    println!(
        "{}{}tkr suggest{} — filter improvement opportunities based on your usage",
        p(BOLD),
        p(CYAN),
        p(RESET)
    );
    println!();

    let bundled_filters = bundled_filter_set();
    let mut suggestions: Vec<String> = Vec::new();

    for row in &rows {
        if row.tokens_in < 100 {
            continue;
        }
        let saved = row.tokens_saved;
        let unsaved = row.tokens_in.saturating_sub(saved);
        let pct = if row.tokens_in > 0 {
            (saved as f64 / row.tokens_in as f64) * 100.0
        } else {
            0.0
        };
        let cmd_first = row.command.split_whitespace().next().unwrap_or("");

        let has_filter = bundled_filters.iter().any(|f| f == cmd_first);
        if !has_filter {
            suggestions.push(format!(
                "  {}{}{} not yet filtered ({} tokens in across {} runs).\n     → create {}filters/{}.toml{} with rules for this tool",
                p(BOLD),
                row.command,
                p(RESET),
                fmt_num(row.tokens_in),
                row.runs,
                p(YELLOW),
                cmd_first,
                p(RESET),
            ));
            continue;
        }

        if pct < 25.0 {
            let mut line = format!(
                "  {}{}{}: {}{:.0}%{} cut, {}{}{} unsaved over {} runs.\n     → tighten {}filters/{}*.toml{} — likely candidates: header lines, boilerplate, repetitive prefixes",
                p(BOLD),
                row.command,
                p(RESET),
                p(RED),
                pct,
                p(RESET),
                p(YELLOW),
                fmt_num(unsaved),
                p(RESET),
                row.runs,
                p(YELLOW),
                cmd_first,
                p(RESET),
            );
            if let Some(h) = extra_command_hint(&row.command, cmd_first) {
                line.push_str(&format!("\n     → {h}"));
            }
            suggestions.push(line);
        }
    }

    print_native_ecosystem_notes(on)?;

    if suggestions.is_empty() {
        println!(
            "  {}No glaring opportunities. Coverage looks healthy.{}",
            p(DIM),
            p(RESET)
        );
        return Ok(());
    }

    for (i, s) in suggestions.iter().enumerate().take(5) {
        if i > 0 {
            println!();
        }
        println!("{s}");
    }
    if suggestions.len() > 5 {
        println!();
        println!(
            "  {}… {} more suggestions hidden{}",
            p(DIM),
            suggestions.len() - 5,
            p(RESET)
        );
    }

    print_noise_section(&analytics_host, on)?;

    println!();
    println!(
        "  {}docs: https://github.com/einyx/tkr#custom-filters · {}native-handlers.md{} (repo {}docs/native-handlers.md{})",
        p(DIM),
        p(YELLOW),
        p(RESET),
        p(DIM),
        p(RESET),
    );
    println!();
    Ok(())
}

fn print_noise_section(host: &dyn tkr_api::host::Host, on: bool) -> Result<()> {
    let p = |c: &'static str| if on { c } else { "" };
    let rows = tkr_analytics::top_noise_signatures_via_host(host, 5, 100).unwrap_or_default();
    if rows.is_empty() {
        return Ok(());
    }
    println!();
    println!(
        "  {}{}Noise patterns to filter{}",
        p(BOLD),
        p(CYAN),
        p(RESET)
    );
    println!(
        "  {}repeated line shapes that survived filtering — candidate suppress_regex rules{}",
        p(DIM),
        p(RESET)
    );
    println!();
    let vault2 = crate::host::boot::vault();
    let host_handle2 = crate::host::boot::get_host();
    let analytics_host =
        crate::host::RealHost::new("tkr-analytics", vault2, host_handle2.bus.clone());

    for row in rows.iter().take(8) {
        let regex = signature_to_regex(&row.signature);
        println!(
            "  {}{}{}: shape {}{}{} (×{}, {} chars)",
            p(BOLD),
            row.command,
            p(RESET),
            p(YELLOW),
            row.signature,
            p(RESET),
            fmt_num(row.occurrences),
            fmt_num(row.total_chars),
        );
        println!("     sample: {}{}{}", p(DIM), row.sample, p(RESET));
        println!("     suppress_regex = '{regex}'");

        // If this signature has an embedding in the vault, surface its k-NN
        // family — patterns that probably want the same suppress rule.
        if let Ok(Some(sig_id)) = tkr_analytics::noise_signature_id_via_host(
            &analytics_host,
            &row.command,
            &row.signature,
        ) {
            if let Ok(neighbors) =
                tkr_analytics::nearest_to_signature_via_host(&analytics_host, sig_id, 3)
            {
                let close: Vec<_> = neighbors
                    .into_iter()
                    .filter(|(_, _, _, d)| *d < 0.6)
                    .collect();
                if !close.is_empty() {
                    println!(
                        "     {}near-duplicate family ({} signatures):{}",
                        p(DIM),
                        close.len(),
                        p(RESET)
                    );
                    for (_id, cmd, sig, dist) in close {
                        println!(
                            "       {}└─{} {} {}{}{} ({}d={:.2}{})",
                            p(DIM),
                            p(RESET),
                            cmd,
                            p(YELLOW),
                            sig,
                            p(RESET),
                            p(DIM),
                            dist,
                            p(RESET),
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Set of `command =` values declared by bundled filter files. Used to detect
/// commands that we record but don't have a filter for.
fn bundled_filter_set() -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    let dirs = [
        crate::config::bundled_filters_dir(),
        Some(
            dirs::home_dir()
                .unwrap_or(PathBuf::from("."))
                .join(".tkr/filters"),
        ),
    ];
    for d in dirs.into_iter().flatten() {
        if !d.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&d) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().is_some_and(|e| e == "toml") {
                    if let Ok(text) = std::fs::read_to_string(&p) {
                        if let Ok(v) = toml::from_str::<toml::Value>(&text) {
                            if let Some(cmd) = v.get("command").and_then(|c| c.as_str()) {
                                commands.push(cmd.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    commands.sort();
    commands.dedup();
    commands
}

fn print_native_ecosystem_notes(on: bool) -> Result<()> {
    let p = |c: &'static str| if on { c } else { "" };
    println!(
        "  {}{}Native handlers & tooling{}",
        p(BOLD),
        p(CYAN),
        p(RESET)
    );
    println!(
        "  {}grep / rg{} — structured compression is default ({}TKR_NATIVE_GREP=0{} falls back to TOML only).",
        p(DIM),
        p(RESET),
        p(YELLOW),
        p(RESET),
    );
    println!(
        "  {}git status{} — eligible runs use {}-sb{} ({}TKR_NATIVE_GIT=0{} disables all git natives).",
        p(DIM),
        p(RESET),
        p(DIM),
        p(RESET),
        p(YELLOW),
        p(RESET),
    );
    println!(
        "  {}git diff{} — condenses unified output; {}TKR_NATIVE_GIT_DIFF=0{} for stream+filters; >8MB diffs fall back.",
        p(DIM),
        p(RESET),
        p(YELLOW),
        p(RESET),
    );
    println!(
        "  {}ls{} — line-capped ({}TKR_NATIVE_LS=0{} for TOML-only).",
        p(DIM),
        p(RESET),
        p(YELLOW),
        p(RESET),
    );
    println!(
        "  {}cargo test{} — elides `{}test … ok{}` spam ({}TKR_NATIVE_CARGO_TEST=0{}).",
        p(DIM),
        p(RESET),
        p(YELLOW),
        p(RESET),
        p(YELLOW),
        p(RESET),
    );
    println!(
        "  {}session log{} — {}TKR_NATIVE_SESSION_LOG=1{} → {}~/.tkr/native-handlers.jsonl{}",
        p(DIM),
        p(RESET),
        p(YELLOW),
        p(RESET),
        p(DIM),
        p(RESET),
    );
    println!(
        "  {}Editor Grep / Read tools{} often {}bypass{} shell hooks — run {}",
        p(DIM),
        p(RESET),
        p(RED),
        p(RESET),
        p(YELLOW),
    );
    println!(
        "  {}`tkr rg`{}, {}`tkr grep`{}, {}`tkr cat`{} so traffic goes through tkr.",
        p(YELLOW),
        p(RESET),
        p(YELLOW),
        p(RESET),
        p(YELLOW),
        p(RESET),
    );
    println!(
        "  {}Heavy test runners{} — {}TKR_MAX_TOKENS{}, tweak {}{}~/.tkr/filters/*.toml{}, or narrower runs.",
        p(DIM),
        p(RESET),
        p(YELLOW),
        p(RESET),
        p(DIM),
        p(YELLOW),
        p(RESET),
    );
    println!();
    Ok(())
}

/// Extra one-liners for analytics rows that look like known noisy commands.
fn extra_command_hint(full_command: &str, cmd_first: &str) -> Option<String> {
    let f = full_command.to_lowercase();
    match cmd_first {
        "grep" | "rg" | "egrep" | "fgrep" => Some(
            "ensure you invoke search via the shell (`tkr rg …`) so the native handler runs; with `rg --json`, matches are summarized like plain ripgrep output.".into(),
        ),
        "cargo" if f.contains("test") => Some(
            "native `cargo test` elides passing `ok` lines (`TKR_NATIVE_CARGO_TEST=0` for full stream + filters); raise `TKR_NATIVE_CARGO_COMPILE_LINES` if compile noise is still heavy.".into(),
        ),
        "go" if f.contains(" test") && !f.contains("help test") => Some(
            "native `go test` elides verbose `=== RUN` / `--- PASS` lines (`TKR_NATIVE_GO_TEST=0` for full stream); skipped automatically for `-json`/`-bench`/`-fuzz`.".into(),
        ),
        "jest" | "vitest" | "mocha" | "playwright" | "cypress" => Some(
            "standalone test-runner binaries are covered by the same native shrinking as npm-style runs (`TKR_NATIVE_JS_TEST=0` for full output); `playwright` uses `playwright test`, `cypress` uses `cypress run`.".into(),
        ),
        "npm" | "pnpm" | "yarn" | "npx" | "bunx" | "corepack"
            if f.contains("test")
                || f.contains("vitest")
                || f.contains("jest")
                || f.contains("playwright") =>
        {
            Some(
                "native JS/Deno/Bun test runs elide vitest ✓, jest PASS, deno `… ok`, bun `(pass)` (`TKR_NATIVE_JS_TEST=0` for full stream); optionally tighten package-manager filters or cap with `TKR_MAX_TOKENS`.".into(),
            )
        }
        "deno" | "bun" if f.contains("test") => Some(
            "native test output shrinking (`TKR_NATIVE_JS_TEST=0` for full stream); run via shell so `tkr` sees the command.".into(),
        ),
        "git" => {
            if f.contains("diff") {
                Some(
                    "native path condenses unified diff output (`TKR_NATIVE_GIT_DIFF=0` uses stream+filters only); huge diffs fall back automatically.".into(),
                )
            } else if f.contains("status") {
                Some(
                    "native path rewrites to `git status -sb` unless porcelain/verbose (`TKR_NATIVE_GIT=0` disables all git natives).".into(),
                )
            } else {
                None
            }
        }
        "ls" => Some(
            "native caps line count (`TKR_NATIVE_LS_MAX_LINES`) — set `TKR_NATIVE_LS=0` to use filter TOML only.".into(),
        ),
        "pytest" | "py.test" => Some(
            "native pytest elides verbose `PASSED` and dot-progress rows (`TKR_NATIVE_PYTEST=0` for the full stream).".into(),
        ),
        "uv" | "poetry" | "pipenv" | "pdm"
            if f.contains("pytest") || f.contains(" -m pytest") =>
        {
            Some(
                "native pytest wrapper (`TKR_NATIVE_PYTEST=0` to disable the native path for poetry/uv-style runs).".into(),
            )
        }
        c if (c.starts_with("python") || c == "py") && f.contains("-m pytest") => {
            Some(
                "native pytest elides verbose `PASSED` / dot rows when run as `-m pytest` (`TKR_NATIVE_PYTEST=0` for full output).".into(),
            )
        }
        _ => None,
    }
}
