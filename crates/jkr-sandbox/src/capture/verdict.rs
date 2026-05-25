use super::trace::{FileEvent, FileOp, Flag, NetEvent, Verdict, VerdictLevel};

/// Substrings that mark a path as sensitive. Lowercased compare.
const SENSITIVE_MARKERS: &[&str] = &[
    "/.ssh/", "/.aws/", "/.config/gh/", "/etc/shadow",
    "/.gnupg/", "login.keychain", "/.netrc",
];
const SENSITIVE_SUFFIXES: &[&str] = &[".pem", ".env"];

fn is_sensitive(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    SENSITIVE_MARKERS.iter().any(|m| p.contains(m))
        || SENSITIVE_SUFFIXES.iter().any(|s| p.ends_with(s))
}

fn is_loopback(addr: &str) -> bool {
    addr.starts_with("127.") || addr.starts_with("[::1]") || addr.starts_with("::1")
}

fn max_level(a: VerdictLevel, b: VerdictLevel) -> VerdictLevel {
    use VerdictLevel::*;
    match (&a, &b) {
        (Suspicious, _) | (_, Suspicious) => Suspicious,
        (Notable, _) | (_, Notable) => Notable,
        _ => Clean,
    }
}

/// Distinct allowed external hosts above which egress is suspicious.
const EGRESS_HOST_SUSPICIOUS: usize = 5;
/// Denial count above which a denied_storm flag fires.
const DENIED_STORM: u32 = 50;

pub fn compute_verdict(
    files: &[FileEvent],
    net: &[NetEvent],
    writable_roots: &[String],
) -> Verdict {
    let mut flags: Vec<Flag> = Vec::new();

    for f in files {
        if is_sensitive(&f.path) {
            let (severity, verb) = if f.allowed {
                (VerdictLevel::Suspicious, "read")
            } else {
                (VerdictLevel::Notable, "attempted (denied) read of")
            };
            flags.push(Flag {
                kind: "sensitive_path".into(),
                severity,
                detail: format!("{verb} {}", f.path),
            });
        }
        if f.allowed
            && f.op == FileOp::Write
            && !writable_roots.iter().any(|root| f.path.starts_with(root))
        {
            flags.push(Flag {
                kind: "write_outside_cwd".into(),
                severity: VerdictLevel::Suspicious,
                detail: format!("allowed write outside declared roots: {}", f.path),
            });
        }
    }

    let external: Vec<&NetEvent> = net.iter()
        .filter(|n| n.allowed && !is_loopback(&n.addr))
        .collect();
    if !external.is_empty() {
        let severity = if external.len() >= EGRESS_HOST_SUSPICIOUS {
            VerdictLevel::Suspicious
        } else {
            VerdictLevel::Notable
        };
        flags.push(Flag {
            kind: "egress".into(),
            severity,
            detail: format!("{} external connection(s)", external.len()),
        });
    }

    let denied: u32 = files.iter().filter(|f| !f.allowed).map(|f| f.count).sum::<u32>()
        + net.iter().filter(|n| !n.allowed).map(|n| n.count).sum::<u32>();
    if denied >= DENIED_STORM {
        flags.push(Flag {
            kind: "denied_storm".into(),
            severity: VerdictLevel::Notable,
            detail: format!("{denied} denied operations"),
        });
    }

    let level = flags.iter().fold(VerdictLevel::Clean, |acc, f| max_level(acc, f.severity.clone()));
    Verdict { level, flags }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::trace::NetFamily;

    fn fe(path: &str, op: FileOp, allowed: bool) -> FileEvent {
        FileEvent { path: path.into(), op, allowed, errno: None, count: 1 }
    }
    fn ne(addr: &str, allowed: bool) -> NetEvent {
        NetEvent { addr: addr.into(), family: NetFamily::V4, allowed, count: 1 }
    }

    #[test]
    fn clean_when_nothing_notable() {
        let v = compute_verdict(&[fe("/tmp/x", FileOp::Read, true)], &[], &["/tmp".into()]);
        assert_eq!(v.level, VerdictLevel::Clean);
        assert!(v.flags.is_empty());
    }

    #[test]
    fn denied_sensitive_read_is_notable() {
        let v = compute_verdict(&[fe("/home/u/.ssh/id_rsa", FileOp::Read, false)], &[], &[]);
        assert_eq!(v.level, VerdictLevel::Notable);
        assert_eq!(v.flags[0].kind, "sensitive_path");
    }

    #[test]
    fn allowed_sensitive_read_is_suspicious() {
        let v = compute_verdict(&[fe("/home/u/.ssh/id_rsa", FileOp::Read, true)], &[], &[]);
        assert_eq!(v.level, VerdictLevel::Suspicious);
    }

    #[test]
    fn single_external_connect_is_notable() {
        let v = compute_verdict(&[], &[ne("93.184.216.34:443", true)], &[]);
        assert_eq!(v.level, VerdictLevel::Notable);
    }

    #[test]
    fn loopback_connect_is_clean() {
        let v = compute_verdict(&[], &[ne("127.0.0.1:5432", true)], &[]);
        assert_eq!(v.level, VerdictLevel::Clean);
    }

    #[test]
    fn allowed_write_outside_roots_is_suspicious() {
        let v = compute_verdict(&[fe("/etc/cron.d/x", FileOp::Write, true)], &[], &["/work".into()]);
        assert_eq!(v.level, VerdictLevel::Suspicious);
        assert!(v.flags.iter().any(|f| f.kind == "write_outside_cwd"));
    }

    #[test]
    fn many_denials_flag_storm() {
        let mut f = fe("/etc/x", FileOp::Stat, false);
        f.count = 60;
        let v = compute_verdict(&[f], &[], &[]);
        assert!(v.flags.iter().any(|f| f.kind == "denied_storm"));
    }
}
