//! Calibration tool: reads stdin, runs through a filter pack, prints stats.
//! Usage: `cat fixture.txt | cargo run --example measure -p tkr-filter -- <command> [subcommand]`

use std::io::{self, BufRead, Write};
use tkr_api::{FilterResult, LegacyPlugin as Plugin};
use tkr_filter::FilterPlugin;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("grep");
    let subcmd = args.get(2).map(String::as_str).unwrap_or("");

    let filter_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../filters");
    let mut plugin = FilterPlugin::new();
    plugin.load_for_command(&filter_dir, cmd)?;

    let stdin = io::stdin();
    let mut in_bytes = 0usize;
    let mut in_lines = 0usize;
    let mut out_bytes = 0usize;
    let mut out_lines = 0usize;
    let mut out = io::BufWriter::new(io::stdout().lock());

    for line in stdin.lock().lines() {
        let line = line?;
        in_bytes += line.len() + 1;
        in_lines += 1;
        match plugin.filter(&line, cmd, subcmd, in_lines as u64) {
            FilterResult::Pass => {
                writeln!(out, "{line}")?;
                out_bytes += line.len() + 1;
                out_lines += 1;
            }
            FilterResult::Replace(s) => {
                writeln!(out, "{s}")?;
                out_bytes += s.len() + 1;
                out_lines += 1;
            }
            FilterResult::Suppress | FilterResult::SuppressWithNote(_) => {}
            FilterResult::Annotate(s) => {
                writeln!(out, "{line} {s}")?;
                out_bytes += line.len() + s.len() + 2;
                out_lines += 1;
            }
        }
    }
    let summary = plugin.flush();
    if !summary.is_empty() {
        writeln!(out, "{summary}")?;
        out_bytes += summary.len() + 1;
        out_lines += summary.lines().count();
    }
    out.flush()?;

    eprintln!(
        "  in:  {in_bytes:>9} bytes / {in_lines:>6} lines\n  out: {out_bytes:>9} bytes / {out_lines:>6} lines\n  reduction: {:.1}% bytes / {:.1}% lines",
        100.0 * (1.0 - out_bytes as f64 / in_bytes.max(1) as f64),
        100.0 * (1.0 - out_lines as f64 / in_lines.max(1) as f64),
    );
    Ok(())
}
