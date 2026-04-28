use std::ffi::c_char;
use std::ffi::c_void;

// ---------------------------------------------------------------------------
// LineContext — passed to every plugin for each output line
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct LineContext {
    pub line: *const c_char,
    pub line_len: usize,
    pub command: *const c_char,
    pub command_len: usize,
    pub args: *const c_char,
    pub args_len: usize,
    pub line_index: u64,
}

// ---------------------------------------------------------------------------
// FilterResult — the decision returned by a plugin for each line
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone)]
pub enum FilterResult {
    /// Emit the line unchanged.
    Pass,
    /// Emit different text. Caller owns the allocation.
    Replace(*mut c_char, usize),
    /// Drop the line silently.
    Suppress,
    /// Drop the line and increment counter bucket[id]; flush emits a summary.
    SuppressWithNote(u64),
    /// Append an annotation to the line. Caller owns the allocation.
    Annotate(*mut c_char, usize),
}

// SAFETY: raw pointers inside FilterResult are accessed only on the calling
// thread within a single command lifetime — no concurrent access occurs.
unsafe impl Send for FilterResult {}

impl PartialEq for FilterResult {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FilterResult::Pass, FilterResult::Pass) => true,
            (FilterResult::Suppress, FilterResult::Suppress) => true,
            (FilterResult::SuppressWithNote(a), FilterResult::SuppressWithNote(b)) => a == b,
            // Pointer variants: compare discriminant only (safe for test usage).
            (FilterResult::Replace(pa, la), FilterResult::Replace(pb, lb)) => {
                pa == pb && la == lb
            }
            (FilterResult::Annotate(pa, la), FilterResult::Annotate(pb, lb)) => {
                pa == pb && la == lb
            }
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin — safe Rust interface for built-in plugins
// ---------------------------------------------------------------------------

pub trait Plugin: Send {
    fn init(config: &str) -> Box<dyn Plugin>
    where
        Self: Sized;

    fn filter(&mut self, line: &str, command: &str, args: &str, index: u64) -> FilterResult;

    /// Newline-delimited summary lines, or empty string.
    fn flush(&mut self) -> String;
}

// ---------------------------------------------------------------------------
// C ABI function pointer types for dynamic (cdylib) plugins
// ---------------------------------------------------------------------------

/// Create plugin state from a null-terminated config string.
/// Returns an opaque pointer; caller owns the allocation.
pub type FnInit = unsafe extern "C" fn(config: *const c_char) -> *mut c_void;

/// Filter one line. Returns a `FilterResult`.
pub type FnFilter =
    unsafe extern "C" fn(ctx: *const LineContext, state: *mut c_void) -> FilterResult;

/// Return a null-terminated, newline-delimited summary (or null).
/// Caller must NOT free the returned pointer.
pub type FnFlush = unsafe extern "C" fn(state: *mut c_void) -> *const c_char;

/// Destroy plugin state.
pub type FnDestroy = unsafe extern "C" fn(state: *mut c_void);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
}
