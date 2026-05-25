//! Streaming hook for the agent loop. The loop calls `emit_*` helpers;
//! a sink assigns the monotonic `seq` and decides where the event goes
//! (NDJSON stdout for `--stream`, or discarded for a plain run).

use jkr_api::{AgentEvent, AgentEventKind};

/// Where agent events go. Implementors must be cheap to call per token.
pub trait EventSink {
    fn write(&mut self, event: AgentEvent);
}

/// Wraps a sink and assigns monotonic seq numbers, so the loop never
/// has to track them itself.
pub struct SeqEmitter<'a> {
    sink: &'a mut dyn EventSink,
    next: u64,
}

impl<'a> SeqEmitter<'a> {
    pub fn new(sink: &'a mut dyn EventSink) -> Self {
        Self { sink, next: 0 }
    }
    pub fn emit(&mut self, kind: AgentEventKind) {
        let seq = self.next;
        self.next += 1;
        self.sink.write(AgentEvent { seq, kind });
    }
}

/// A sink that collects events in a Vec — used by tests.
#[derive(Default)]
pub struct VecSink {
    pub events: Vec<AgentEvent>,
}
impl EventSink for VecSink {
    fn write(&mut self, event: AgentEvent) {
        self.events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_monotonic_seq() {
        let mut sink = VecSink::default();
        {
            let mut em = SeqEmitter::new(&mut sink);
            em.emit(AgentEventKind::Step { step: 1, max: 30 });
            em.emit(AgentEventKind::Token { text: "hi".into() });
        }
        assert_eq!(sink.events[0].seq, 0);
        assert_eq!(sink.events[1].seq, 1);
    }
}
