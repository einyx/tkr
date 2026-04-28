use anyhow::{Context, Result};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

pub struct LineStream {
    pub child: Child,
    reader: BufReader<std::process::ChildStdout>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

impl Iterator for LineStream {
    type Item = Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => {
                if line.ends_with('\n') { line.pop(); if line.ends_with('\r') { line.pop(); } }
                Some(Ok(line))
            }
            Err(e) => Some(Err(e.into())),
        }
    }
}

impl Drop for LineStream {
    fn drop(&mut self) {
        let _ = self.child.wait();
        if let Some(t) = self.stderr_thread.take() { let _ = t.join(); }
    }
}

pub fn stream_command(cmd: &str, args: &[&str]) -> Result<LineStream> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning `{cmd}`"))?;

    let stderr = child.stderr.take().expect("stderr piped");
    let stderr_thread = std::thread::spawn(move || {
        use std::io::Write;
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if let Ok(line) = line {
                let _ = writeln!(std::io::stderr(), "{line}");
            }
        }
    });

    let stdout = child.stdout.take().expect("stdout piped");
    Ok(LineStream {
        child,
        reader: BufReader::new(stdout),
        stderr_thread: Some(stderr_thread),
    })
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
}
