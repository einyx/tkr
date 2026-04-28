use anyhow::Result;
use tkr_analytics::AnalyticsStore;

pub fn run(breakdown: bool) -> Result<()> {
    let home = dirs::home_dir().unwrap_or_default();
    let db_path = home.join(".tkr/analytics.db");

    if !db_path.exists() {
        println!("No analytics data yet. Run some commands with tkr first.");
        return Ok(());
    }

    let store = AnalyticsStore::open(db_path.to_str().unwrap_or(":memory:"))?;
    let rows = store.total_savings()?;

    if rows.is_empty() {
        println!("No analytics data yet.");
        return Ok(());
    }

    let total_in: u64 = rows.iter().map(|r| r.tokens_in).sum();
    let total_saved: u64 = rows.iter().map(|r| r.tokens_saved).sum();
    let pct = if total_in > 0 { (total_saved as f64 / total_in as f64 * 100.0) as u64 } else { 0 };

    println!("Token savings summary");
    println!("─────────────────────────────────");
    println!("  Tokens in:    {:>10}", total_in);
    println!("  Tokens saved: {:>10}  ({pct}%)", total_saved);
    println!();

    if breakdown {
        println!("{:<30} {:>10} {:>10} {:>6}", "Command", "Tokens in", "Saved", "Runs");
        println!("{}", "─".repeat(60));
        for row in &rows {
            println!("{:<30} {:>10} {:>10} {:>6}", row.command, row.tokens_in, row.tokens_saved, row.runs);
        }
    }

    Ok(())
}
