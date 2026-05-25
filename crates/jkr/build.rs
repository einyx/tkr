use std::path::Path;

/// Minimum rustc for this workspace (keep aligned with `rust-toolchain.toml` and `rust-version`).
const MIN_RUST_MINOR: u32 = 88;

fn assert_rustc_at_least_1_88() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = match std::process::Command::new(&rustc).arg("--version").output() {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => {
            panic!(
                "jkr build: `{rustc} --version` failed (status {}). \
                 Fix your Rust install or use rustup: https://rustup.rs",
                o.status
            );
        }
        Err(e) => panic!("jkr build: could not run `{rustc} --version`: {e}"),
    };
    let text = String::from_utf8_lossy(&out);
    let rest = text
        .strip_prefix("rustc ")
        .unwrap_or_else(|| panic!("jkr build: unexpected `rustc --version` output: {text:?}"));
    let ver = rest
        .split_whitespace()
        .next()
        .and_then(|s| s.split('-').next())
        .unwrap_or_else(|| panic!("jkr build: could not parse version from: {text:?}"));
    let mut parts = ver.split('.');
    let major: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("jkr build: could not parse major from rustc version {ver:?}"));
    let minor: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("jkr build: could not parse minor from rustc version {ver:?}"));
    let ok = major > 1 || (major == 1 && minor >= MIN_RUST_MINOR);
    if !ok {
        panic!(
            "jkr requires Rust 1.{MIN_RUST_MINOR} or newer — you have {ver}.\n\n\
             This repo pins 1.{MIN_RUST_MINOR}.0 in rust-toolchain.toml for rustup.\n\
             Homebrew's standalone `rustc`/`cargo` ignores that file.\n\n\
             Fix:\n\
             1. Install or use rustup: https://rustup.rs\n\
             2. rustup toolchain install 1.{MIN_RUST_MINOR}.0 && rustup default 1.{MIN_RUST_MINOR}.0\n\
             3. Put ~/.cargo/bin before Homebrew on PATH, then run `which cargo` (should be under ~/.cargo/bin)\n\
             4. Or run: rustup run 1.{MIN_RUST_MINOR}.0 cargo build --release\n"
        );
    }
}

fn main() {
    assert_rustc_at_least_1_88();

    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let filters_dir = Path::new(&manifest).join("../../filters");
    println!(
        "cargo:rustc-env=JKR_BUNDLED_FILTERS_DIR={}",
        filters_dir.display()
    );
    println!("cargo:rerun-if-changed=../../filters");
}
