//! `tkr suggest` — examine the analytics DB and suggest concrete filter
//! improvements for commands with low savings ratios. Surfaces:
//!   - high-volume / low-savings commands → which filter file to edit
//!   - commands tkr is recording but has no filter file for
//!   - rough estimate of additional tokens we could save

use crate::signature::signature_to_regex;
use crate::util::fmt_num;
use anyhow::Result;
use std::path::PathBuf;
use tkr_analytics::AnalyticsStore;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

pub fn run() -> Result<()> {
    let home = dirs::home_dir().unwrap_or_default();
    let db_path = home.join(".tkr/analytics.db");
    if !db_path.exists() {
        println!("No analytics yet. Run some commands first to generate data.");
        return Ok(());
    }
    let store = AnalyticsStore::open(db_path.to_str().unwrap_or(":memory:"))?;
    let mut rows = store.total_savings()?;
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
            suggestions.push(format!(
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
            ));
        }
    }

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
        println!("{}", s);
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

    print_noise_section(&store, on)?;

    println!();
    println!(
        "  {}docs: https://github.com/einyx/tkr#custom-filters{}",
        p(DIM),
        p(RESET)
    );
    println!();
    Ok(())
}

fn print_noise_section(store: &AnalyticsStore, on: bool) -> Result<()> {
    let p = |c: &'static str| if on { c } else { "" };
    let rows = store.top_noise_signatures(5, 100)?;
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
        println!("     suppress_regex = '{}'", regex);
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
                if p.extension().map_or(false, |e| e == "toml") {
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

