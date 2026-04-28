use anyhow::{Context, Result};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

pub struct LineStream {
    pub child: Child,
    rx: mpsc::Receiver<Result<String>>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Iterator for LineStream {
    type Item = Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        self.rx.recv().ok()
    }
}

impl Drop for LineStream {
    fn drop(&mut self) {
        let _ = self.child.wait();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Spawn `cmd` with `args`, merging stdout and stderr into a single line stream
/// (cargo, npm, and most tools write progress/warnings to stderr — those need
/// to pass through the filter pipeline too).
pub fn stream_command(cmd: &str, args: &[&str]) -> Result<LineStream> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning `{cmd}`"))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let (tx, rx) = mpsc::channel::<Result<String>>();

    let tx_out = tx.clone();
    let stdout_thread = std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let item = line.map(strip_eol).map_err(Into::into);
            if tx_out.send(item).is_err() {
                break;
            }
        }
    });

    let stderr_thread = std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let item = line.map(strip_eol).map_err(Into::into);
            if tx.send(item).is_err() {
                break;
            }
        }
    });

    Ok(LineStream {
        child,
        rx,
        threads: vec![stdout_thread, stderr_thread],
    })
}

fn strip_eol(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_echo_collects_lines() {
        let lines: Vec<_> = stream_command("echo", &["hello world"]).unwrap().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].as_deref().unwrap(), "hello world");
    }

    #[test]
    fn run_nonexistent_command_errors() {
        let result = stream_command("__tkr_no_such_cmd_xyz", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn merges_stdout_and_stderr() {
        // sh -c "echo to-stdout; echo to-stderr 1>&2" — both should appear
        let lines: Vec<String> = stream_command("sh", &["-c", "echo to-stdout; echo to-stderr 1>&2"])
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(lines.iter().any(|l| l == "to-stdout"));
        assert!(lines.iter().any(|l| l == "to-stderr"));
    }
}
