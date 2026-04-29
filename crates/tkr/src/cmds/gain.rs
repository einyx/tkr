use crate::util::fmt_num;
use anyhow::Result;
use std::io::IsTerminal;
use tkr_analytics::SavingsRow;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const MAGENTA: &str = "\x1b[35m";
const GREY: &str = "\x1b[90m";

/// Anthropic Sonnet input price approximation: $3 per 1M tokens.
/// Lets us put a $ figure on saved tokens. Adjust if pricing changes.
const USD_PER_M_INPUT_TOKENS: f64 = 3.0;

/// Width budget for the table — chosen to fit a typical 80-col terminal.
const TABLE_WIDTH: usize = 64;

struct Style {
    on: bool,
}

impl Style {
    fn new(plain: bool) -> Self {
        Self {
            on: !plain && std::io::stdout().is_terminal(),
        }
    }
    fn p<'a>(&self, code: &'a str) -> &'a str {
        if self.on {
            code
        } else {
            ""
        }
    }
}

pub fn run(breakdown: bool, sort: &str, plain: bool) -> Result<()> {
    let vault = crate::host::boot::vault();
    let host_handle = crate::host::boot::get_host();
    let analytics_host = crate::host::RealHost::new(
        "tkr-analytics",
        vault,
        host_handle.bus.clone(),
    );
    let mut rows = match tkr_analytics::total_savings_via_host(&analytics_host) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("tkr gain: could not read analytics from vault: {e}");
            Vec::new()
        }
    };
    if rows.is_empty() {
        println!("No analytics data yet. Run some commands with tkr first.");
        return Ok(());
    }

    sort_rows(&mut rows, sort);

    let total_in: u64 = rows.iter().map(|r| r.tokens_in).sum();
    let total_saved: u64 = rows.iter().map(|r| r.tokens_saved).sum();
    let total_kept: u64 = total_in.saturating_sub(total_saved);
    let total_runs: u64 = rows.iter().map(|r| r.runs).sum();
    let pct = ratio_pct(total_saved, total_in);
    let usd_saved = (total_saved as f64 / 1_000_000.0) * USD_PER_M_INPUT_TOKENS;

    let s = Style::new(plain);

    print_banner(&s);
    print_aggregate(&s, total_in, total_saved, total_kept, pct, usd_saved, total_runs, rows.len());
    print_aggregate_bar(&s, total_in, total_saved);
    println!();
    print_table(&s, &rows, breakdown);
    print_improvement_opportunities(&s, &rows);
    print_footer(&s);

    Ok(())
}

fn sort_rows(rows: &mut [SavingsRow], sort: &str) {
    match sort {
        "in" => rows.sort_by(|a, b| b.tokens_in.cmp(&a.tokens_in)),
        "runs" => rows.sort_by(|a, b| b.runs.cmp(&a.runs)),
        "ratio" => rows.sort_by(|a, b| {
            let ra = ratio_pct(a.tokens_saved, a.tokens_in);
            let rb = ratio_pct(b.tokens_saved, b.tokens_in);
            rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => rows.sort_by(|a, b| b.tokens_saved.cmp(&a.tokens_saved)),
    }
}

fn print_banner(s: &Style) {
    let title = "tkr · token savings report";
    let inner = TABLE_WIDTH - 2;
    let pad = inner.saturating_sub(title.chars().count() + 2);

    println!();
    println!(
        "{}{}╭{}╮{}",
        s.p(BOLD),
        s.p(CYAN),
        "─".repeat(inner),
        s.p(RESET)
    );
    println!(
        "{}{}│{} {}{}{}{} {}{}│{}",
        s.p(BOLD),
        s.p(CYAN),
        s.p(RESET),
        s.p(BOLD),
        title,
        s.p(RESET),
        " ".repeat(pad),
        s.p(BOLD),
        s.p(CYAN),
        s.p(RESET),
    );
    println!(
        "{}{}╰{}╯{}",
        s.p(BOLD),
        s.p(CYAN),
        "─".repeat(inner),
        s.p(RESET)
    );
    println!();
}

fn print_aggregate(
    s: &Style,
    total_in: u64,
    total_saved: u64,
    total_kept: u64,
    pct: f64,
    usd: f64,
    runs: u64,
    cmds: usize,
) {
    let label = |k: &str| format!("{}{:<14}{}", s.p(DIM), k, s.p(RESET));

    println!(
        "  {}  {}{:>12}{}  {}{}",
        label("tokens in"),
        s.p(BOLD),
        fmt_num(total_in),
        s.p(RESET),
        s.p(DIM),
        s.p(RESET),
    );
    println!(
        "  {}  {}{}{:>12}{}  {}sent to LLM{}",
        label("tokens kept"),
        s.p(BOLD),
        s.p(YELLOW),
        fmt_num(total_kept),
        s.p(RESET),
        s.p(DIM),
        s.p(RESET),
    );
    println!(
        "  {}  {}{}{:>12}{}  {}filtered out{}",
        label("tokens saved"),
        s.p(BOLD),
        s.p(GREEN),
        fmt_num(total_saved),
        s.p(RESET),
        s.p(DIM),
        s.p(RESET),
    );
    println!(
        "  {}  {}{}{:>11.1}%{}  {}of total{}",
        label("reduction"),
        s.p(BOLD),
        s.p(tier_color(s, pct)),
        pct,
        s.p(RESET),
        s.p(DIM),
        s.p(RESET),
    );
    println!(
        "  {}  {}{}${:>10.4}{}  {}@ ${:.0}/M input tokens{}",
        label("cost saved"),
        s.p(BOLD),
        s.p(GREEN),
        usd,
        s.p(RESET),
        s.p(DIM),
        USD_PER_M_INPUT_TOKENS,
        s.p(RESET),
    );
    println!(
        "  {}  {}{:>12}{}  {}across {} commands{}",
        label("runs"),
        s.p(BOLD),
        fmt_num(runs),
        s.p(RESET),
        s.p(DIM),
        cmds,
        s.p(RESET),
    );
}

fn print_aggregate_bar(s: &Style, total_in: u64, total_saved: u64) {
    let width = TABLE_WIDTH - 4;
    let saved_w = if total_in > 0 {
        ((total_saved as f64 / total_in as f64) * width as f64).round() as usize
    } else {
        0
    };
    let saved_w = saved_w.min(width);
    let kept_w = width - saved_w;

    println!();
    println!(
        "  {}{}{}{}{}{}",
        s.p(GREEN),
        "█".repeat(saved_w),
        s.p(RESET),
        s.p(YELLOW),
        "▒".repeat(kept_w),
        s.p(RESET),
    );
    println!(
        "  {}█ saved   ▒ kept (sent to LLM){}",
        s.p(DIM),
        s.p(RESET)
    );
}

fn print_table(s: &Style, rows: &[SavingsRow], breakdown: bool) {
    // Default view: hide rows with 0% savings (filter has nothing to do for
    // them anyway). --breakdown shows everything including the no-ops.
    let visible: Vec<&SavingsRow> = if breakdown {
        rows.iter().collect()
    } else {
        rows.iter().filter(|r| r.tokens_saved > 0).collect()
    };
    let hidden = rows.len() - visible.len();
    let limit = if breakdown { visible.len() } else { 10.min(visible.len()) };
    let max_saved = visible.iter().map(|r| r.tokens_saved).max().unwrap_or(1).max(1);

    println!();
    println!(
        "  {}{}per-command savings{}",
        s.p(BOLD),
        s.p(MAGENTA),
        s.p(RESET)
    );
    println!("  {}{}{}", s.p(GREY), "─".repeat(TABLE_WIDTH - 2), s.p(RESET));
    println!(
        "  {}{:<22} {:>9} {:>9} {:>4} {:>5}  {}{}",
        s.p(DIM),
        "command",
        "in",
        "saved",
        "runs",
        "ratio",
        "",
        s.p(RESET),
    );

    if visible.is_empty() {
        println!(
            "  {}(no commands have measurable savings yet){}",
            s.p(DIM),
            s.p(RESET)
        );
    }

    for row in visible.iter().take(limit) {
        let row_pct = ratio_pct(row.tokens_saved, row.tokens_in);
        let bar_len = ((row.tokens_saved as f64 / max_saved as f64) * 14.0).round() as usize;
        let bar_str: String = "█".repeat(bar_len);
        let cmd = truncate(&row.command, 22);
        println!(
            "  {:<22} {:>9} {}{:>9}{} {:>4} {}{:>4.0}%{}  {}{}{}",
            cmd,
            fmt_num(row.tokens_in),
            s.p(GREEN),
            fmt_num(row.tokens_saved),
            s.p(RESET),
            row.runs,
            s.p(tier_color(s, row_pct)),
            row_pct,
            s.p(RESET),
            s.p(tier_color(s, row_pct)),
            bar_str,
            s.p(RESET),
        );
    }

    if !breakdown && (visible.len() > limit || hidden > 0) {
        println!();
        let extra = visible.len().saturating_sub(limit);
        let mut bits = Vec::new();
        if extra > 0 {
            bits.push(format!("{} more", extra));
        }
        if hidden > 0 {
            bits.push(format!("{} with 0% savings", hidden));
        }
        println!(
            "  {}… {} (use {}--breakdown{} to see all){}",
            s.p(DIM),
            bits.join(", "),
            s.p(YELLOW),
            s.p(DIM),
            s.p(RESET),
        );
    }
}

/// Surface commands where lots of tokens went in but little came out — these
/// are the highest-leverage filter improvements.
fn print_improvement_opportunities(s: &Style, rows: &[SavingsRow]) {
    let mut candidates: Vec<&SavingsRow> = rows
        .iter()
        .filter(|r| r.tokens_in >= 200 && ratio_pct(r.tokens_saved, r.tokens_in) < 25.0)
        .collect();
    if candidates.is_empty() {
        return;
    }
    candidates.sort_by(|a, b| {
        let unsaved_a = a.tokens_in.saturating_sub(a.tokens_saved);
        let unsaved_b = b.tokens_in.saturating_sub(b.tokens_saved);
        unsaved_b.cmp(&unsaved_a)
    });

    println!();
    println!(
        "  {}{}improvement opportunities{}  {}(low savings, lots of in){}",
        s.p(BOLD),
        s.p(YELLOW),
        s.p(RESET),
        s.p(DIM),
        s.p(RESET),
    );
    println!("  {}{}{}", s.p(GREY), "─".repeat(TABLE_WIDTH - 2), s.p(RESET));
    for row in candidates.iter().take(3) {
        let row_pct = ratio_pct(row.tokens_saved, row.tokens_in);
        let unsaved = row.tokens_in.saturating_sub(row.tokens_saved);
        println!(
            "  {:<22} {:>9} in  {}{:>4.0}%{} cut, {}{:>9}{} unsaved",
            truncate(&row.command, 22),
            fmt_num(row.tokens_in),
            s.p(RED),
            row_pct,
            s.p(RESET),
            s.p(YELLOW),
            fmt_num(unsaved),
            s.p(RESET),
        );
    }
}

fn print_footer(s: &Style) {
    println!();
    println!(
        "  {}sort: --sort=savings|in|runs|ratio · {}--breakdown{}{} for all{}",
        s.p(DIM),
        s.p(YELLOW),
        s.p(DIM),
        "",
        s.p(RESET),
    );
    println!();
}

fn ratio_pct(saved: u64, total: u64) -> f64 {
    if total > 0 {
        (saved as f64 / total as f64) * 100.0
    } else {
        0.0
    }
}

fn tier_color(s: &Style, pct: f64) -> &'static str {
    if !s.on {
        return "";
    }
    if pct >= 60.0 {
        GREEN
    } else if pct >= 30.0 {
        YELLOW
    } else if pct >= 10.0 {
        MAGENTA
    } else {
        RED
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_safe_with_zero() {
        assert_eq!(ratio_pct(0, 0), 0.0);
        assert_eq!(ratio_pct(50, 100), 50.0);
    }

    #[test]
    fn fmt_thousands() {
        assert_eq!(fmt_num(0), "0");
        assert_eq!(fmt_num(999), "999");
        assert_eq!(fmt_num(1000), "1,000");
        assert_eq!(fmt_num(71275), "71,275");
        assert_eq!(fmt_num(1_234_567_890), "1,234,567,890");
    }

    #[test]
    fn truncate_handles_unicode() {
        assert_eq!(truncate("hi", 10), "hi");
        assert_eq!(truncate("├── anyhow v1.0.102", 10), "├── anyho…");
    }

    #[test]
    fn sort_default_is_savings() {
        let mut r = vec![
            mk("a", 100, 10, 1),
            mk("b", 100, 50, 1),
            mk("c", 100, 30, 1),
        ];
        sort_rows(&mut r, "savings");
        assert_eq!(r[0].command, "b");
        assert_eq!(r[2].command, "a");
    }

    #[test]
    fn sort_by_runs() {
        let mut r = vec![
            mk("a", 100, 10, 5),
            mk("b", 100, 50, 1),
            mk("c", 100, 30, 9),
        ];
        sort_rows(&mut r, "runs");
        assert_eq!(r[0].command, "c");
        assert_eq!(r[2].command, "b");
    }

    #[test]
    fn sort_by_ratio() {
        let mut r = vec![
            mk("low", 1000, 100, 1),  // 10%
            mk("hi", 100, 90, 1),     // 90%
            mk("mid", 100, 50, 1),    // 50%
        ];
        sort_rows(&mut r, "ratio");
        assert_eq!(r[0].command, "hi");
        assert_eq!(r[1].command, "mid");
        assert_eq!(r[2].command, "low");
    }

    fn mk(cmd: &str, tin: u64, saved: u64, runs: u64) -> SavingsRow {
        SavingsRow {
            command: cmd.to_string(),
            tokens_in: tin,
            tokens_saved: saved,
            runs,
        }
    }
}
