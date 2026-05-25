#![cfg(target_os = "linux")]
use std::path::PathBuf;
use jkr_sandbox::capture::run_with_capture;
use jkr_sandbox::capture::trace::{CaptureKind, FileOp};
use jkr_sandbox::policy::SandboxPolicy;

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

#[test]
#[ignore = "timing benchmark; run manually with --ignored --nocapture"]
fn ptrace_overhead_benchmark() {
    use std::time::Instant;
    use jkr_sandbox::exec::run_sandboxed_output_only;

    let worktree: std::path::PathBuf = "/home/alessio/jkr-sandbox-analysis".into();
    let crates_dir = worktree.join("crates");

    // Mirror the allowlist used in the other scenarios, plus the worktree root
    // so grep can traverse the crates directory.
    let policy = policy_allow(
        vec![
            worktree.clone(),
            "/usr".into(),
            "/bin".into(),
            "/lib".into(),
            "/lib64".into(),
            "/etc".into(),
            // grep binary may live under /proc/self/exe symlink resolution
            "/proc".into(),
        ],
        vec![],
    );

    let crates_str = crates_dir.to_str().unwrap();
    let reps = 5;
    let mut capture_total = std::time::Duration::ZERO;
    let mut plain_total = std::time::Duration::ZERO;
    let mut last_file_count = 0usize;

    for i in 0..reps {
        // capture path
        let t = Instant::now();
        let (_, trace) = run_with_capture("grep", &["-r", "fn ", crates_str], &policy)
            .expect("run_with_capture failed");
        capture_total += t.elapsed();
        if i == reps - 1 {
            last_file_count = trace.files.len();
        }

        // output-only path
        let t = Instant::now();
        run_sandboxed_output_only("grep", &["-r", "fn ", crates_str], &policy)
            .expect("run_sandboxed_output_only failed");
        plain_total += t.elapsed();
    }

    println!("ptrace capture: {:?} total over {reps} reps", capture_total);
    println!("output-only:    {:?} total over {reps} reps", plain_total);
    println!(
        "overhead ratio: {:.2}x",
        capture_total.as_secs_f64() / plain_total.as_secs_f64()
    );
    println!("trace.files.len() on last capture run: {last_file_count}");

    assert!(
        last_file_count > 0,
        "trace.files was empty — Landlock likely blocked grep from reading crates/"
    );
}
