#![cfg(target_os = "linux")]
use std::path::PathBuf;
use tkr_sandbox::capture::run_with_capture;
use tkr_sandbox::capture::trace::{CaptureKind, FileOp};
use tkr_sandbox::policy::SandboxPolicy;

fn policy_allow(read: Vec<PathBuf>, write: Vec<PathBuf>) -> SandboxPolicy {
    let mut p = SandboxPolicy::default();
    p.fs_read = read;
    p.fs_write = write;
    p
}

fn base_reads(extra: PathBuf) -> Vec<PathBuf> {
    vec![extra, "/usr".into(), "/bin".into(), "/lib".into(), "/lib64".into(), "/etc".into()]
}

fn scenario_allowed_read() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("hello.txt");
    std::fs::write(&f, b"hi").unwrap();
    let policy = policy_allow(base_reads(tmp.path().into()), vec![]);
    let (_out, trace) = run_with_capture("cat", &[f.to_str().unwrap()], &policy).unwrap();
    assert_eq!(trace.capture_kind, CaptureKind::Full);
    assert!(trace.files.iter().any(|e|
        e.path.ends_with("hello.txt") && e.op == FileOp::Read && e.allowed),
        "expected an allowed read of hello.txt; got: {:?}", trace.files);
}

fn scenario_denied_read() {
    let secret_dir = tempfile::tempdir().unwrap();
    let secret = secret_dir.path().join("secret");
    std::fs::write(&secret, b"x").unwrap();
    let allowed_dir = tempfile::tempdir().unwrap();   // only extra allowed root
    let policy = policy_allow(base_reads(allowed_dir.path().into()), vec![]);
    let (_out, trace) = run_with_capture("cat", &[secret.to_str().unwrap()], &policy).unwrap();
    let ev = trace.files.iter().find(|e| e.path.ends_with("secret"));
    assert!(ev.is_some(), "expected a file event for the denied read; got: {:?}", trace.files);
    assert!(!ev.unwrap().allowed, "denied read should be allowed=false");
    assert!(ev.unwrap().errno.is_some());
}

fn scenario_follows_children() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("data");
    std::fs::write(&f, b"hi").unwrap();
    let policy = policy_allow(base_reads(tmp.path().into()), vec![]);
    let cmd = format!("cat {}", f.to_str().unwrap());
    let (_out, trace) = run_with_capture("sh", &["-c", &cmd], &policy).unwrap();
    assert!(
        trace.execs.iter().any(|e| e.argv0.contains("cat") || e.argv_preview.contains("cat"))
        || trace.files.iter().any(|e| e.path.ends_with("data")),
        "should have traced the child process spawned by sh; execs={:?} files={:?}",
        trace.execs, trace.files);
}

#[test]
fn ptrace_capture_scenarios() {
    scenario_allowed_read();
    scenario_denied_read();
    scenario_follows_children();
}
