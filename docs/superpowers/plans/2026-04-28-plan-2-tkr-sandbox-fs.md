# jkr-sandbox: FS Allowlist MVP — Implementation Plan (Plan 2 of 6)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Add a `jkr-sandbox` crate that confines tool process execution to a per-tool filesystem allowlist. Linux uses `landlock`; macOS uses `sandbox-exec`. A new `ProcessTool` adapter lets any external command be registered as a sandboxed `Tool`. Integration tests verify enforcement.

**Architecture:** New crate `jkr-sandbox` with two backends behind one `enforce()` API. `SandboxPolicy` is a serde struct. `ProcessTool` is a generic `Tool` that spawns a child command under policy. Tests gated by `#[cfg(target_os = ...)]` verify a write outside the allowlist fails.

**Out of scope (Plan 2.5+):** seccomp, network-egress policy, cgroups, UID isolation, signed policy.

**Tech Stack:** `landlock` 0.4, `/usr/bin/sandbox-exec`, `std::process::Command`, `tempfile`.

**Spec reference:** `docs/superpowers/specs/2026-04-28-jkr-agents-platform-design.md` §7.2, §8.

---

## File Structure

**New crate:**
- `crates/jkr-sandbox/{Cargo.toml,src/lib.rs,src/policy.rs,src/error.rs,src/exec.rs,src/linux.rs,src/macos.rs,tests/fs_enforcement.rs}`

**Modified:**
- `Cargo.toml` (workspace) — add member, `landlock` and `tempfile` workspace deps
- `crates/jkr-agent/Cargo.toml` — depend on `jkr-sandbox`
- `crates/jkr-agent/src/tools/{mod.rs,process.rs}` — `ProcessTool` adapter
- `crates/jkr-agent/src/lib.rs` — re-export `ProcessTool`
- `crates/jkr-agent/tests/sandboxed_loop.rs` — end-to-end test

---

## Task 1: Scaffold `jkr-sandbox`

- [ ] **Step 1.1: Update workspace `Cargo.toml`**

In `members`, add `"crates/jkr-sandbox"`. In `[workspace.dependencies]` add:

```toml
landlock = "0.4"
tempfile = "3"
```

- [ ] **Step 1.2: Create `crates/jkr-sandbox/Cargo.toml`**

```toml
[package]
name = "jkr-sandbox"
version = "0.1.0"
edition = "2021"
license = "Apache-2.0"

[lib]
name = "jkr_sandbox"
crate-type = ["rlib"]

[dependencies]
anyhow = { workspace = true }
serde = { workspace = true }
thiserror = { workspace = true }

[target.'cfg(target_os = "linux")'.dependencies]
landlock = { workspace = true }

[target.'cfg(target_os = "macos")'.dependencies]
tempfile = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
toml = { workspace = true }
```

- [ ] **Step 1.3: Create `crates/jkr-sandbox/src/lib.rs`**

```rust
pub mod error;
pub mod policy;
pub mod exec;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

pub use error::SandboxError;
pub use policy::{SandboxPolicy, PolicyBuilder};
pub use exec::{run_sandboxed, SandboxOutput};
```

- [ ] **Step 1.4: Create stub files**

Each stub is a single-line comment naming the task that will fill it (Task 2 → error.rs, Task 3 → policy.rs, Task 4 → exec.rs, Task 5 → linux.rs, Task 6 → macos.rs).

- [ ] **Step 1.5: Verify and commit**

`cargo check --workspace` must pass.

```bash
git add Cargo.toml crates/jkr-sandbox
git commit -m "scaffold jkr-sandbox crate"
```

---

## Task 2: `SandboxError`

Replace `crates/jkr-sandbox/src/error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("policy violation: {0}")]
    PolicyViolation(String),
    #[error("sandbox not supported on this platform")]
    Unsupported,
    #[error("backend failure: {0}")]
    Backend(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_policy_violation() {
        let e = SandboxError::PolicyViolation("write to /etc denied".into());
        assert_eq!(format!("{}", e), "policy violation: write to /etc denied");
    }
}
```

Run `cargo test -p jkr-sandbox --lib error::`. Commit: `jkr-sandbox: error type`.

---

## Task 3: `SandboxPolicy`

Replace `crates/jkr-sandbox/src/policy.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SandboxPolicy {
    #[serde(default)]
    pub fs_read: Vec<PathBuf>,
    #[serde(default)]
    pub fs_write: Vec<PathBuf>,
    /// Bypass enforcement entirely. Only for opt-out debugging.
    #[serde(default)]
    pub disabled: bool,
}

impl SandboxPolicy {
    pub fn deny_all() -> Self { Self::default() }
    pub fn builder() -> PolicyBuilder { PolicyBuilder::default() }
    pub fn validate(&self) -> Result<(), String> {
        for p in self.fs_read.iter().chain(self.fs_write.iter()) {
            if !p.is_absolute() {
                return Err(format!("policy paths must be absolute: {}", p.display()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct PolicyBuilder { inner: SandboxPolicy }

impl PolicyBuilder {
    pub fn allow_read<P: AsRef<Path>>(mut self, p: P) -> Self {
        self.inner.fs_read.push(p.as_ref().to_path_buf()); self
    }
    pub fn allow_write<P: AsRef<Path>>(mut self, p: P) -> Self {
        self.inner.fs_write.push(p.as_ref().to_path_buf()); self
    }
    pub fn build(self) -> SandboxPolicy { self.inner }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_all_is_empty() {
        let p = SandboxPolicy::deny_all();
        assert!(p.fs_read.is_empty() && p.fs_write.is_empty() && !p.disabled);
    }

    #[test]
    fn builder_chains() {
        let p = SandboxPolicy::builder().allow_read("/etc").allow_write("/tmp/foo").build();
        assert_eq!(p.fs_read, vec![PathBuf::from("/etc")]);
        assert_eq!(p.fs_write, vec![PathBuf::from("/tmp/foo")]);
    }

    #[test]
    fn validate_rejects_relative() {
        let p = SandboxPolicy::builder().allow_read("relative/path").build();
        assert!(p.validate().is_err());
    }

    #[test]
    fn deserializes_from_toml() {
        let src = r#"fs_read = ["/etc"]
fs_write = ["/tmp"]"#;
        let p: SandboxPolicy = toml::from_str(src).unwrap();
        assert_eq!(p.fs_read.len(), 1);
        assert_eq!(p.fs_write.len(), 1);
    }
}
```

Run `cargo test -p jkr-sandbox --lib policy::`. Commit: `jkr-sandbox: SandboxPolicy + builder`.

---

## Task 4: Cross-platform `run_sandboxed` entry point

Replace `crates/jkr-sandbox/src/exec.rs`:

```rust
use crate::error::SandboxError;
use crate::policy::SandboxPolicy;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct SandboxOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: i32,
}

pub fn run_sandboxed(
    command: &str,
    args: &[&str],
    policy: &SandboxPolicy,
) -> Result<SandboxOutput, SandboxError> {
    if let Err(e) = policy.validate() {
        return Err(SandboxError::PolicyViolation(e));
    }
    if policy.disabled {
        return run_unsandboxed(command, args);
    }
    #[cfg(target_os = "linux")]
    { return crate::linux::run(command, args, policy); }
    #[cfg(target_os = "macos")]
    { return crate::macos::run(command, args, policy); }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (command, args);
        Err(SandboxError::Unsupported)
    }
}

fn run_unsandboxed(command: &str, args: &[&str]) -> Result<SandboxOutput, SandboxError> {
    let out = Command::new(command).args(args).output()?;
    Ok(SandboxOutput {
        stdout: out.stdout, stderr: out.stderr,
        exit: out.status.code().unwrap_or(-1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_paths_in_policy() {
        let p = SandboxPolicy::builder().allow_read("rel").build();
        let r = run_sandboxed("/bin/true", &[], &p);
        assert!(matches!(r, Err(SandboxError::PolicyViolation(_))));
    }

    #[test]
    fn disabled_policy_runs_unsandboxed() {
        let mut p = SandboxPolicy::deny_all();
        p.disabled = true;
        let r = run_sandboxed("/bin/echo", &["hi"], &p).unwrap();
        assert_eq!(r.exit, 0);
        assert!(String::from_utf8_lossy(&r.stdout).contains("hi"));
    }
}
```

Run `cargo test -p jkr-sandbox --lib exec::`. Commit: `jkr-sandbox: cross-platform run_sandboxed entry point`.

---

## Task 5: Linux backend (landlock)

**The implementer must read landlock 0.4 docs before this step:** <https://docs.rs/landlock/0.4/landlock/>. Pin landlock at 0.4. Landlock restriction is one-way per thread; we apply it in the child via `pre_exec`.

Replace `crates/jkr-sandbox/src/linux.rs`:

```rust
use crate::error::SandboxError;
use crate::policy::SandboxPolicy;
use crate::exec::SandboxOutput;
use std::os::unix::process::CommandExt;
use std::process::Command;

pub fn run(command: &str, args: &[&str], policy: &SandboxPolicy) -> Result<SandboxOutput, SandboxError> {
    let read = policy.fs_read.clone();
    let write = policy.fs_write.clone();

    let mut cmd = Command::new(command);
    cmd.args(args);

    // SAFETY: pre_exec runs in the child after fork() and before execve();
    // landlock syscalls are async-signal-safe.
    unsafe {
        cmd.pre_exec(move || {
            apply_landlock(&read, &write).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, format!("landlock: {}", e))
            })
        });
    }

    let out = cmd.output().map_err(|e| SandboxError::Backend(e.to_string()))?;
    Ok(SandboxOutput {
        stdout: out.stdout, stderr: out.stderr,
        exit: out.status.code().unwrap_or(-1),
    })
}

fn apply_landlock(
    fs_read: &[std::path::PathBuf],
    fs_write: &[std::path::PathBuf],
) -> Result<(), String> {
    use landlock::{
        Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus, ABI,
    };

    let abi = ABI::V2;
    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| format!("handle_access: {}", e))?
        .create()
        .map_err(|e| format!("create: {}", e))?;

    for p in fs_read {
        let fd = PathFd::new(p).map_err(|e| format!("open {}: {}", p.display(), e))?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, AccessFs::from_read(abi)))
            .map_err(|e| format!("add_rule read {}: {}", p.display(), e))?;
    }
    for p in fs_write {
        let fd = PathFd::new(p).map_err(|e| format!("open {}: {}", p.display(), e))?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, AccessFs::from_all(abi)))
            .map_err(|e| format!("add_rule write {}: {}", p.display(), e))?;
    }

    let status = ruleset
        .restrict_self()
        .map_err(|e| format!("restrict_self: {}", e))?;

    if matches!(status.ruleset, RulesetStatus::NotEnforced) {
        return Err("landlock not supported by kernel".into());
    }
    Ok(())
}
```

**Verification:** before completing, run `cargo check -p jkr-sandbox` on Linux. If landlock 0.4 type symbols differ, adjust imports/calls; the semantics remain the same. Commit: `jkr-sandbox: Linux landlock backend`.

---

## Task 6: macOS backend (sandbox-exec wrapper)

Replace `crates/jkr-sandbox/src/macos.rs`:

```rust
use crate::error::SandboxError;
use crate::policy::SandboxPolicy;
use crate::exec::SandboxOutput;
use std::io::Write;
use std::process::Command;

pub fn run(command: &str, args: &[&str], policy: &SandboxPolicy) -> Result<SandboxOutput, SandboxError> {
    let profile = build_profile(policy);
    let mut tmp = tempfile::NamedTempFile::new().map_err(|e| SandboxError::Backend(e.to_string()))?;
    tmp.write_all(profile.as_bytes()).map_err(|e| SandboxError::Backend(e.to_string()))?;
    let profile_path = tmp.path().to_path_buf();

    let mut cmd = Command::new("/usr/bin/sandbox-exec");
    cmd.arg("-f").arg(&profile_path).arg(command).args(args);
    let out = cmd.output().map_err(|e| SandboxError::Backend(e.to_string()))?;
    Ok(SandboxOutput {
        stdout: out.stdout, stderr: out.stderr,
        exit: out.status.code().unwrap_or(-1),
    })
}

pub(crate) fn build_profile(policy: &SandboxPolicy) -> String {
    let mut s = String::from("(version 1)\n(deny default)\n");
    s.push_str("(allow process-fork process-exec)\n");
    s.push_str("(allow mach-lookup)\n");
    s.push_str("(allow sysctl-read)\n");
    s.push_str("(allow signal (target self))\n");
    s.push_str("(allow file-read* (subpath \"/usr/lib\"))\n");
    s.push_str("(allow file-read* (subpath \"/System/Library\"))\n");
    s.push_str("(allow file-read* (subpath \"/Library/Apple/System\"))\n");
    s.push_str("(allow file-read* (literal \"/dev/null\") (literal \"/dev/urandom\"))\n");
    s.push_str("(allow file-write* (literal \"/dev/null\"))\n");
    for p in &policy.fs_read {
        s.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", esc(&p.to_string_lossy())));
    }
    for p in &policy.fs_write {
        let e = esc(&p.to_string_lossy());
        s.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", e));
        s.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", e));
    }
    s
}

fn esc(s: &str) -> String { s.replace('\\', "\\\\").replace('"', "\\\"") }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_contains_deny_default() {
        let s = build_profile(&SandboxPolicy::default());
        assert!(s.contains("(deny default)"));
    }
    #[test]
    fn profile_includes_writable_paths() {
        let p = SandboxPolicy::builder().allow_write("/tmp/foo").build();
        let s = build_profile(&p);
        assert!(s.contains("(allow file-write* (subpath \"/tmp/foo\"))"));
    }
}
```

Run `cargo test -p jkr-sandbox --lib macos::` on macOS. Commit: `jkr-sandbox: macOS sandbox-exec backend`.

---

## Task 7: Integration test — write outside allowlist must fail

Create `crates/jkr-sandbox/tests/fs_enforcement.rs`:

```rust
use jkr_sandbox::{run_sandboxed, SandboxPolicy};

fn run_test() {
    let allowed = tempfile::tempdir().unwrap();
    let denied = tempfile::tempdir().unwrap();
    let allowed_target = allowed.path().join("ok.txt");
    let denied_target = denied.path().join("bad.txt");

    let policy = SandboxPolicy::builder()
        .allow_read("/")
        .allow_write(allowed.path())
        .build();

    // Allowed write must succeed.
    let r1 = run_sandboxed(
        "/usr/bin/touch",
        &[allowed_target.to_str().unwrap()],
        &policy,
    ).unwrap();
    assert_eq!(r1.exit, 0, "stderr={}", String::from_utf8_lossy(&r1.stderr));
    assert!(allowed_target.exists(), "allowed file should exist after touch");

    // Denied write must fail or produce no file.
    let r2 = run_sandboxed(
        "/usr/bin/touch",
        &[denied_target.to_str().unwrap()],
        &policy,
    ).unwrap();
    assert!(
        r2.exit != 0 || !denied_target.exists(),
        "denied write should have failed (exit={}, file_exists={})",
        r2.exit,
        denied_target.exists(),
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_blocks_write_outside_allowlist() { run_test(); }

#[cfg(target_os = "macos")]
#[test]
fn macos_blocks_write_outside_allowlist() { run_test(); }
```

Run `cargo test -p jkr-sandbox --test fs_enforcement` — passes on Linux (kernel ≥ 5.13) and macOS. Commit: `jkr-sandbox: FS enforcement integration test`.

If Linux kernel is too old, mark the test `#[ignore]` and document — do NOT relax the assertion.

---

## Task 8: `ProcessTool` adapter

Add to `crates/jkr-agent/Cargo.toml` `[dependencies]`:
```toml
jkr-sandbox = { path = "../jkr-sandbox" }
```

Edit `crates/jkr-agent/src/tools/mod.rs`:
```rust
pub mod echo;
pub mod process;
```

Create `crates/jkr-agent/src/tools/process.rs`:

```rust
use crate::tool::{Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;
use jkr_sandbox::{run_sandboxed, SandboxPolicy};

pub struct ProcessTool {
    name: String,
    description: String,
    command: String,
    arg_template: Vec<ArgSlot>,
    policy: SandboxPolicy,
    input_schema: Value,
}

#[derive(Clone)]
enum ArgSlot { Literal(String), Named(String) }

impl ProcessTool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        command: impl Into<String>,
        arg_template: Vec<String>,
        policy: SandboxPolicy,
        input_schema: Value,
    ) -> Self {
        let slots = arg_template.into_iter().map(|s| {
            if let Some(name) = s.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
                ArgSlot::Named(name.to_string())
            } else {
                ArgSlot::Literal(s)
            }
        }).collect();
        Self { name: name.into(), description: description.into(), command: command.into(),
               arg_template: slots, policy, input_schema }
    }
    pub fn description(&self) -> &str { &self.description }

    fn render_args(&self, input: &Value) -> Result<Vec<String>> {
        let mut out = Vec::with_capacity(self.arg_template.len());
        for slot in &self.arg_template {
            match slot {
                ArgSlot::Literal(s) => out.push(s.clone()),
                ArgSlot::Named(k) => {
                    let v = input.get(k).ok_or_else(|| anyhow::anyhow!("missing input field '{}'", k))?;
                    let s = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    out.push(s);
                }
            }
        }
        Ok(out)
    }
}

impl Tool for ProcessTool {
    fn name(&self) -> &str { &self.name }
    fn input_schema(&self) -> Value { self.input_schema.clone() }
    fn run(&mut self, input: &Value) -> Result<ToolResult> {
        let args = self.render_args(input)?;
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run_sandboxed(&self.command, &arg_refs, &self.policy)
            .map_err(|e| anyhow::anyhow!("sandbox: {}", e))?;
        let mut content = String::from_utf8_lossy(&out.stdout).into_owned();
        if !out.stderr.is_empty() {
            content.push_str(&String::from_utf8_lossy(&out.stderr));
        }
        let raw = content.len();
        Ok(ToolResult { content, raw_bytes: raw, filtered_bytes: raw, exit: out.exit })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_literal_and_named_args() {
        let mut t = ProcessTool::new(
            "say", "echoes", "/bin/echo",
            vec!["--".into(), "{msg}".into()],
            SandboxPolicy::builder().allow_read("/").build(),
            json!({"type":"object","properties":{"msg":{"type":"string"}},"required":["msg"]}),
        );
        let r = t.run(&json!({"msg":"hi"})).unwrap();
        assert_eq!(r.exit, 0);
        assert!(r.content.contains("hi"));
    }

    #[test]
    fn missing_named_field_errors() {
        let mut t = ProcessTool::new(
            "say", "", "/bin/echo",
            vec!["{msg}".into()],
            SandboxPolicy::builder().allow_read("/").build(),
            json!({}),
        );
        assert!(t.run(&json!({})).is_err());
    }
}
```

Add to `crates/jkr-agent/src/lib.rs`:
```rust
pub use tools::process::ProcessTool;
```

Run `cargo test -p jkr-agent --lib tools::process`. Commit: `jkr-agent: ProcessTool adapter on jkr-sandbox`.

---

## Task 9: End-to-end test — sandboxed ProcessTool in agent loop

Create `crates/jkr-agent/tests/sandboxed_loop.rs`:

```rust
use serde_json::json;
use std::cell::RefCell;
use jkr_agent::provider::{ContentBlock, Message, Provider, ProviderResponse, StopReason};
use jkr_agent::{Manifest, ProcessTool, ToolRegistry};
use jkr_sandbox::SandboxPolicy;

struct ScriptedProvider { script: RefCell<Vec<ProviderResponse>> }
impl Provider for ScriptedProvider {
    fn send(&self, _: Option<&str>, _: &[Message], _: &[serde_json::Value], _: u32)
        -> anyhow::Result<ProviderResponse>
    {
        let mut s = self.script.borrow_mut();
        if s.is_empty() { Err(anyhow::anyhow!("script exhausted")) } else { Ok(s.remove(0)) }
    }
}

#[test]
fn agent_loop_runs_sandboxed_tool() {
    let policy = SandboxPolicy::builder().allow_read("/").build();
    let process_tool = ProcessTool::new(
        "say", "echoes a message", "/bin/echo",
        vec!["{msg}".into()],
        policy,
        json!({"type":"object","properties":{"msg":{"type":"string"}},"required":["msg"]}),
    );
    let provider = ScriptedProvider {
        script: RefCell::new(vec![
            ProviderResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tu_1".into(), name: "say".into(),
                    input: json!({"msg":"sandboxed"}),
                }],
                stop_reason: StopReason::ToolUse,
                input_tokens: 1, output_tokens: 1,
            },
            ProviderResponse {
                content: vec![ContentBlock::Text { text: "fin".into() }],
                stop_reason: StopReason::EndTurn,
                input_tokens: 1, output_tokens: 1,
            },
        ]),
    };
    let manifest = Manifest::parse(r#"
name = "t"
task = "test"
mode = "auto"
max_steps = 4

[model]
provider = "anthropic"
name = "x"
"#).unwrap();
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(process_tool));
    let outcome = jkr_agent::run(&manifest, &provider, &mut tools, None).unwrap();
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "fin");
    assert!(outcome.raw_bytes_total > 0);
}
```

Add to `crates/jkr-agent/Cargo.toml` `[dev-dependencies]` if missing:
```toml
jkr-sandbox = { path = "../jkr-sandbox" }
```

Run `cargo test -p jkr-agent --test sandboxed_loop`. Commit: `jkr-agent: end-to-end test with sandboxed ProcessTool`.

---

## Task 10: Final sweep + push

`cargo test --workspace` — all green.
`git push origin spec/agents-platform`.
