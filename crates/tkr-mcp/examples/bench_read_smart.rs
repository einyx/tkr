//! Compare `tkr_read_smart` output size vs the naive baseline of reading
//! every file that contains the question's keywords.
//!
//! Run from repo root:
//!   cargo run -p tkr-mcp --example bench_read_smart -- <root> "question one" "another question"

use std::fs;
use std::path::PathBuf;
use tkr_mcp::index_backed;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("usage: bench <root> <question>..."));
    let questions: Vec<String> = args.collect();
    if questions.is_empty() {
        eprintln!("no questions provided");
        std::process::exit(2);
    }

    index_backed::build(&root)?;

    println!(
        "{:<40} {:>12} {:>12} {:>8}",
        "question", "naive B", "smart B", "ratio"
    );
    let mut total_naive = 0usize;
    let mut total_smart = 0usize;
    for q in &questions {
        let smart = index_backed::try_read_smart(q, &root, 8)?.unwrap_or_default();
        // Naive baseline: every file under root that contains any keyword,
        // read in full. (Mirrors "Grep then Read" without the Grep filter.)
        let tokens: Vec<String> = q
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|s| s.len() >= 2)
            .map(|s| s.to_lowercase())
            .collect();
        let mut naive_bytes = 0usize;
        for entry in ignore::WalkBuilder::new(&root)
            .standard_filters(true)
            .build()
            .filter_map(Result::ok)
        {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let bytes = match fs::read(p) {
                Ok(b) => b,
                Err(_) => continue,
            };
            // Treat as text only.
            if bytes.iter().take(8000).any(|b| *b == 0) {
                continue;
            }
            let text = match std::str::from_utf8(&bytes) {
                Ok(s) => s.to_lowercase(),
                Err(_) => continue,
            };
            if tokens.iter().any(|t| text.contains(t)) {
                naive_bytes += bytes.len();
            }
        }

        total_naive += naive_bytes;
        total_smart += smart.len();
        let ratio = if smart.is_empty() {
            f64::INFINITY
        } else {
            naive_bytes as f64 / smart.len() as f64
        };
        println!(
            "{:<40} {:>12} {:>12} {:>7.1}x",
            truncate(q, 40),
            naive_bytes,
            smart.len(),
            ratio
        );
    }
    let ratio = if total_smart == 0 {
        f64::INFINITY
    } else {
        total_naive as f64 / total_smart as f64
    };
    println!(
        "\nTOTAL: naive={}B, smart={}B, {:.1}x reduction",
        total_naive, total_smart, ratio
    );
    Ok(())
}

fn truncate(s: &str, n: usize) -> &str {
    if s.len() <= n {
        s
    } else {
        &s[..n]
    }
}
