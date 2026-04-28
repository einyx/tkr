use crate::loop_::RunOutcome;
use std::fmt;

#[derive(Debug, Clone)]
pub struct RunReceipt {
    pub agent: String,
    pub steps: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub raw_bytes: usize,
    pub filtered_bytes: usize,
}

impl RunReceipt {
    pub fn from_outcome(agent: &str, o: &RunOutcome) -> Self {
        Self {
            agent: agent.to_string(),
            steps: o.steps,
            input_tokens: o.input_tokens_total,
            output_tokens: o.output_tokens_total,
            raw_bytes: o.raw_bytes_total,
            filtered_bytes: o.filtered_bytes_total,
        }
    }

    pub fn savings_ratio(&self) -> f64 {
        if self.raw_bytes == 0 { 0.0 } else {
            1.0 - (self.filtered_bytes as f64 / self.raw_bytes as f64)
        }
    }
}

impl fmt::Display for RunReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "── tkr run receipt ──")?;
        writeln!(f, "  agent:           {}", self.agent)?;
        writeln!(f, "  steps:           {}", self.steps)?;
        writeln!(f, "  tokens (in/out): {} / {}", self.input_tokens, self.output_tokens)?;
        writeln!(
            f,
            "  tool output:     {} B raw → {} B filtered ({:.1}% saved)",
            self.raw_bytes,
            self.filtered_bytes,
            self.savings_ratio() * 100.0
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(raw: usize, filt: usize) -> RunOutcome {
        RunOutcome {
            final_text: "ok".into(),
            steps: 2,
            input_tokens_total: 100,
            output_tokens_total: 50,
            raw_bytes_total: raw,
            filtered_bytes_total: filt,
            messages: vec![],
        }
    }

    #[test]
    fn savings_ratio_handles_zero() {
        let r = RunReceipt::from_outcome("a", &outcome(0, 0));
        assert_eq!(r.savings_ratio(), 0.0);
    }

    #[test]
    fn savings_ratio_basic() {
        let r = RunReceipt::from_outcome("a", &outcome(1000, 200));
        assert!((r.savings_ratio() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn display_includes_agent_and_savings() {
        let r = RunReceipt::from_outcome("hello", &outcome(1000, 200));
        let s = format!("{}", r);
        assert!(s.contains("hello"));
        assert!(s.contains("80.0% saved"));
    }
}
