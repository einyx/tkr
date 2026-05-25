use tkr_sandbox::{run_sandboxed, SandboxPolicy};

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
    let (r1, _trace) = run_sandboxed(
        "/usr/bin/touch",
        &[allowed_target.to_str().unwrap()],
        &policy,
    )
    .unwrap();
    assert_eq!(r1.exit, 0, "stderr={}", String::from_utf8_lossy(&r1.stderr));
    assert!(
        allowed_target.exists(),
        "allowed file should exist after touch"
    );

    // Denied write must fail or produce no file.
    let (r2, _trace) = run_sandboxed(
        "/usr/bin/touch",
        &[denied_target.to_str().unwrap()],
        &policy,
    )
    .unwrap();
    assert!(
        r2.exit != 0 || !denied_target.exists(),
        "denied write should have failed (exit={}, file_exists={})",
        r2.exit,
        denied_target.exists(),
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_blocks_write_outside_allowlist() {
    run_test();
}

#[cfg(target_os = "macos")]
#[test]
fn macos_blocks_write_outside_allowlist() {
    run_test();
}
