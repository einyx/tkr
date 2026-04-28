// Legacy plugin trait. Kept while the v1 semantic plugin migrates to v2.
// FilterResult uses owned Rust types — no C ABI, no raw pointers, no leaks.

#[derive(Debug, Clone, PartialEq)]
pub enum FilterResult {
    /// Emit the line unchanged.
    Pass,
    /// Emit a different line.
    Replace(String),
    /// Drop the line silently.
    Suppress,
    /// Drop the line and increment counter bucket[id]; flush emits a summary.
    SuppressWithNote(u64),
    /// Append an annotation to the line.
    Annotate(String),
}

pub trait Plugin: Send {
    fn init(config: &str) -> Box<dyn Plugin>
    where
        Self: Sized;

    fn filter(&mut self, line: &str, command: &str, args: &str, index: u64) -> FilterResult;

    /// Newline-delimited summary lines, or empty string.
    fn flush(&mut self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_result_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<FilterResult>();
    }

    #[test]
    fn filter_result_pass_eq() {
        assert_eq!(FilterResult::Pass, FilterResult::Pass);
    }

    #[test]
    fn replace_carries_string() {
        let r = FilterResult::Replace("abc".to_string());
        assert_eq!(r, FilterResult::Replace("abc".to_string()));
    }
}
