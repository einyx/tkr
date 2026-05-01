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
        assert_eq!(format!("{e}"), "policy violation: write to /etc denied");
    }
}
