//! Native command handlers (RTK-style): capture full child output, compress with
//! purpose-built Rust logic, then emit. Runs *before* the TOML line-filter
//! pipeline when enabled — see `try_run`.
//!
//! Inspired by the [RTK](https://github.com/rtk-ai/rtk) approach (`grep_cmd`
//! and similar): structured summarization beats generic line rules on huge
//! search output.
//!
//! See `docs/native-handlers.md` for env vars and IDE/tooling notes.

mod cargo;
mod cat;
mod git;
mod go_test;
mod grep;
mod js_test;
mod ls;
mod pytest;
mod session_log;

use crate::stream::PipelineResult;
use anyhow::Result;
use std::path::Path;

/// Outcome of a native handler — same analytics shape as [`crate::stream::run_pipeline_direct`].
pub struct NativeOutcome {
    pub pipeline: PipelineResult,
    /// Child exit code to propagate.
    pub exit_code: i32,
}

/// When `TKR_NATIVE_SESSION_LOG=1`, append one JSON line per native run (see `session_log`).
pub fn log_session_line(cmd: &str, args: &[String], pipeline: &PipelineResult, exit_code: i32) {
    session_log::maybe_append(cmd, args, pipeline, exit_code);
}

fn normalized_command_base(cmd: &str) -> String {
    let mut b = Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd);
    for suf in [".exe", ".EXE", ".cmd", ".CMD"] {
        b = b.strip_suffix(suf).unwrap_or(b);
    }
    b.to_ascii_lowercase()
}

/// When this returns `Ok(Some(..))`, the proxy must not run the streaming TOML pipeline.
pub fn try_run(cmd: &str, cmd_args: &[String]) -> Result<Option<NativeOutcome>> {
    let base = normalized_command_base(cmd);

    match base.as_str() {
        "git" => {
            if git::env_disabled() {
                return Ok(None);
            }
            git::run(cmd, cmd_args)
        }
        "grep" | "egrep" | "fgrep" | "rg" => {
            if grep::env_disabled() {
                return Ok(None);
            }
            grep::run(cmd, cmd_args)
        }
        "cat" => {
            if cat::env_disabled() {
                return Ok(None);
            }
            cat::run(cmd, cmd_args)
        }
        "ls" => {
            if ls::env_disabled() {
                return Ok(None);
            }
            ls::run(cmd, cmd_args)
        }
        "cargo" => {
            if cargo::env_disabled() {
                return Ok(None);
            }
            cargo::run(cmd, cmd_args)
        }
        "go" => {
            if go_test::env_disabled() {
                return Ok(None);
            }
            go_test::run(cmd, cmd_args)
        }
        "npm" | "pnpm" | "yarn" | "npx" | "corepack" | "deno" | "bun" | "bunx"
        | "jest" | "vitest" | "mocha" | "playwright" | "cypress" => {
            if js_test::env_disabled() {
                return Ok(None);
            }
            js_test::run(cmd, cmd_args)
        }
        _ if pytest::might_be_invocation(cmd) => {
            if pytest::env_disabled() {
                return Ok(None);
            }
            pytest::run(cmd, cmd_args)
        }
        _ => Ok(None),
    }
}
