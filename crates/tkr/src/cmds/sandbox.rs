//! `tkr sandbox run` — execute a command under tkr-sandbox.
//!
//! Defaults are deliberately strict: deny-all fs, empty env, 16 MiB output cap,
//! no timeout. Operators opt in to fs/env/resource grants via flags.

use anyhow::{Context, Result};
use std::path::PathBuf;
use tkr_sandbox::{run_sandboxed, SandboxError, SandboxPolicy};

#[allow(clippy::too_many_arguments)]
pub fn run(
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
    env: Vec<String>,
    memory: Option<u64>,
    cpu: Option<u64>,
    timeout_ms: Option<u64>,
    max_output: Option<u64>,
    no_network: bool,
    allow_connect: Vec<u16>,
    allow_bind: Vec<u16>,
    argv: Vec<String>,
) -> Result<()> {
    let (cmd, args) = argv
        .split_first()
        .map(|(c, rest)| (c.clone(), rest.to_vec()))
        .context("sandbox: missing command")?;

    let mut builder = SandboxPolicy::builder();
    for p in read {
        builder = builder.allow_read(p);
    }
    for p in write {
        builder = builder.allow_write(p);
    }
    for e in env {
        builder = builder.allow_env(e);
    }
    if let Some(n) = memory {
        builder = builder.memory_bytes(n);
    }
    if let Some(n) = cpu {
        builder = builder.cpu_seconds(n);
    }
    if let Some(n) = timeout_ms {
        builder = builder.timeout_ms(n);
    }
    if let Some(n) = max_output {
        builder = builder.max_output_bytes(n);
    }
    if no_network {
        builder = builder.deny_network();
    }
    for p in allow_connect {
        builder = builder.allow_tcp_connect(p);
    }
    for p in allow_bind {
        builder = builder.allow_tcp_bind(p);
    }
    let policy = builder.build();

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match run_sandboxed(&cmd, &arg_refs, &policy) {
        Ok(out) => {
            // Pass-through stdout/stderr, then propagate the child's exit code.
            use std::io::Write;
            let _ = std::io::stdout().write_all(&out.stdout);
            let _ = std::io::stderr().write_all(&out.stderr);
            std::process::exit(out.exit);
        }
        Err(SandboxError::Timeout(ms)) => {
            eprintln!("tkr sandbox: timeout after {ms}ms");
            std::process::exit(124); // GNU timeout convention.
        }
        Err(SandboxError::OutputCapExceeded(n)) => {
            eprintln!("tkr sandbox: child exceeded {n}-byte output cap");
            std::process::exit(125);
        }
        Err(e) => Err(e.into()),
    }
}
