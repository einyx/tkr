//! Micro-bench for the hot path in `jkr-filter`.
//!
//! Builds a synthetic 100k-line corpus that exercises the patterns each rule
//! type was designed for, then times each rule variant in isolation plus a
//! representative "real" pack (cargo). Prints ns/line so we can compare runs.
//!
//! Run with: `cargo run --release -p jkr-filter --example perf`

use std::time::Instant;
use jkr_api::{FilterResult, LegacyPlugin as Plugin};
use jkr_filter::FilterPlugin;

const LINES: usize = 100_000;

fn build_corpus() -> Vec<String> {
    // Mix lines that exercise: prefixes, regex matches, timestamped log
    // repetition, long lines, capture groups for dedup.
    let mut out = Vec::with_capacity(LINES);
    let templates: &[&str] = &[
        "warning: unused variable `x`",
        "Compiling jkr-filter v0.1.0 (/home/alessio/jkr/crates/jkr-filter)",
        "error[E0001]: mismatched types",
        "2026-05-23T14:00:01Z host pod-7c8 ready",
        "    Finished release [optimized] target(s) in 12.34s",
        "test result: ok. 42 passed; 0 failed; 0 ignored",
        "  --> src/lib.rs:42:1",
        "abcdef1234567890abcdef1234567890abcdef1234567890",
        "noise noise noise noise noise noise noise noise noise noise noise",
        "[INFO ] container web restarted unexpectedly with exit code 137",
    ];
    for i in 0..LINES {
        out.push(templates[i % templates.len()].to_string());
    }
    out
}

fn run(label: &str, toml: &str, corpus: &[String], cmd: &str, subcmd: &str) {
    let mut plugin = FilterPlugin::from_toml(toml).expect("parse");
    // Warm: ensure regex DFAs are fully constructed before timing.
    // RegexSet in particular lazy-builds its automaton on first match.
    for line in corpus.iter().take(2048) {
        let _ = plugin.filter(line, cmd, subcmd, 0);
    }
    let start = Instant::now();
    let mut passed = 0usize;
    for (i, line) in corpus.iter().enumerate() {
        if matches!(plugin.filter(line, cmd, subcmd, i as u64), FilterResult::Pass) {
            passed += 1;
        }
    }
    let _ = plugin.flush();
    let elapsed = start.elapsed();
    let ns_per_line = elapsed.as_nanos() as f64 / corpus.len() as f64;
    println!(
        "  {label:<28} {ns_per_line:>7.0} ns/line   ({} passed of {})",
        passed,
        corpus.len()
    );
}

fn main() {
    let corpus = build_corpus();
    println!("corpus: {} lines\n", corpus.len());

    println!("Individual rules:");
    run(
        "suppress_prefix",
        r#"command = "any"
[[rules]]
type = "suppress_prefix"
prefix = "warning:"
"#,
        &corpus,
        "any",
        "",
    );

    run(
        "suppress_regex",
        r#"command = "any"
[[rules]]
type = "suppress_regex"
pattern = "^(Compiling|Finished|test result)"
"#,
        &corpus,
        "any",
        "",
    );

    run(
        "collapse_common_prefix",
        r#"command = "any"
[[rules]]
type = "collapse_common_prefix"
prefix_len = 20
keep_first = 2
"#,
        &corpus,
        "any",
        "",
    );

    run(
        "truncate_long",
        r#"command = "any"
[[rules]]
type = "truncate_long"
max_len = 40
"#,
        &corpus,
        "any",
        "",
    );

    run(
        "dedup_with_count",
        r#"command = "any"
[[rules]]
type = "dedup_with_count"
pattern = "(error\\[E\\d+\\])"
"#,
        &corpus,
        "any",
        "",
    );

    run(
        "group_by_capture",
        r#"command = "any"
[[rules]]
type = "group_by_capture"
pattern = "src/([^:]+):"
header = "files:"
"#,
        &corpus,
        "any",
        "",
    );

    println!("\nReal filter packs (loaded from /filters/):");
    let filter_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../filters");
    // Each pack scopes itself to specific subcommands — pick one each pack
    // actually matches so the bench measures rule cost, not selector-miss.
    let packs: &[(&str, &str)] = &[
        ("cargo", "build"),
        ("git", "log"),
        ("docker", "build"),
        ("npm", "install"),
    ];
    for (pack, subcmd) in packs {
        let path = filter_dir.join(format!("{pack}.toml"));
        let toml = std::fs::read_to_string(&path).expect("read pack");
        run(
            &format!("{pack}.toml ({subcmd})"),
            &toml,
            &corpus,
            pack,
            subcmd,
        );
    }

    println!("\nDispatch overhead (10 non-matching groups, then 1 matching):");
    let mut many = String::new();
    for i in 0..10 {
        many.push_str(&format!(
            "[[rules]]\ntype = \"suppress_prefix\"\nprefix = \"NEVER_MATCHES_{i}\"\n\n"
        ));
    }
    many.push_str("[[rules]]\ntype = \"suppress_prefix\"\nprefix = \"warning:\"\n");
    // Wrap in a single group selector to exercise the per-line group filter:
    let many = format!("command = \"any\"\n{many}");
    run("11 rules, last matches", &many, &corpus, "any", "");

    println!();
}
