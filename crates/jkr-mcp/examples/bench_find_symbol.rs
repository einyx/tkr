//! Compare token cost of `find_symbol` with and without a built index.
//!
//! Strategy: same set of queries, same repo root, two code paths.
//! - "scan"  → search::find_symbol — walks .rs files, parses each one
//! - "index" → index_backed::try_find_symbol — single SQL lookup
//!
//! We report bytes-of-output as a token proxy. The Claude tokenizer averages
//! ~3.5 chars/token on code-shaped text, but for *comparative* numbers byte
//! count is honest and avoids pulling a tokenizer dependency.
//!
//! Run from repo root:
//!   cargo run -p jkr-mcp --example bench_find_symbol -- <repo_root> sym1 sym2 ...

use std::path::PathBuf;
use std::time::Instant;
use jkr_mcp::{index_backed, search};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("usage: bench <root> <symbol>..."));
    let symbols: Vec<String> = args.collect();
    if symbols.is_empty() {
        eprintln!("no symbols provided; pass one or more after the root");
        std::process::exit(2);
    }

    // Build the index first so the index path is fair-game.
    println!("== building index for {} ==", root.display());
    let t = Instant::now();
    let built = index_backed::build(&root)?;
    println!("{}elapsed: {:?}\n", built, t.elapsed());

    let mut scan_bytes = 0usize;
    let mut idx_bytes = 0usize;
    let mut scan_time = std::time::Duration::ZERO;
    let mut idx_time = std::time::Duration::ZERO;

    println!(
        "{:<32} {:>10} {:>10} {:>10} {:>10}",
        "symbol", "scan B", "idx B", "scan ms", "idx ms"
    );
    for sym in &symbols {
        let t = Instant::now();
        let scan_out = search::find_symbol(sym, &root)?;
        let dt_scan = t.elapsed();

        let t = Instant::now();
        let idx_out = index_backed::try_find_symbol(sym, &root)?.unwrap_or_default();
        let dt_idx = t.elapsed();

        scan_bytes += scan_out.len();
        idx_bytes += idx_out.len();
        scan_time += dt_scan;
        idx_time += dt_idx;

        println!(
            "{:<32} {:>10} {:>10} {:>10.1} {:>10.1}",
            sym,
            scan_out.len(),
            idx_out.len(),
            dt_scan.as_secs_f64() * 1000.0,
            dt_idx.as_secs_f64() * 1000.0,
        );
    }
    println!(
        "\nTOTAL: scan={}B in {:?}, index={}B in {:?}  ({}x bytes, {:.1}x faster)",
        scan_bytes,
        scan_time,
        idx_bytes,
        idx_time,
        if idx_bytes == 0 {
            f64::INFINITY
        } else {
            scan_bytes as f64 / idx_bytes as f64
        },
        if idx_time.as_nanos() == 0 {
            f64::INFINITY
        } else {
            scan_time.as_nanos() as f64 / idx_time.as_nanos() as f64
        },
    );
    Ok(())
}
