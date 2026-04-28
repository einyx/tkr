use crate::{
    Result,
    bus::{Event, Reply, Request},
    host::Host,
    manifest::Manifest,
};

#[derive(Debug, Clone)]
pub struct CommandCtx {
    pub command: String,
    pub args: String,
    pub line_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterDecision {
    Pass,
    Suppress,
    Replace(String),
    Annotate(String),
    SuppressWithNote(u64),
}

pub trait Plugin: Send {
    fn manifest(&self) -> Manifest;

    fn on_load(&mut self, host: &dyn Host) -> Result<()>;

    fn on_start(&mut self) -> Result<()> { Ok(()) }

    // Filter-shaped hooks (opt-in via Manifest capabilities)
    fn on_command_begin(&mut self, _ctx: &CommandCtx) -> Result<()> { Ok(()) }
    fn on_line(&mut self, _line: &str, _ctx: &CommandCtx) -> Result<FilterDecision> {
        Ok(FilterDecision::Pass)
    }
    fn on_command_end(&mut self, _ctx: &CommandCtx) -> Result<String> { Ok(String::new()) }

    // Bus-shaped hooks (opt-in via Manifest)
    fn on_event(&mut self, _evt: Event) -> Result<()> { Ok(()) }
    fn on_request(&mut self, req: Request) -> Result<Reply> {
        Err(crate::Error::UnknownMethod(req.method))
    }

    fn on_shutdown(&mut self) -> Result<()> { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use crate::host::Host;

    struct Noop;
    impl Plugin for Noop {
        fn manifest(&self) -> Manifest { Manifest { name: "noop".into(), version: "0".into(), ..Default::default() } }
        fn on_load(&mut self, _: &dyn Host) -> crate::Result<()> { Ok(()) }
    }

    #[test] fn defaults_compile() {
        let _: Box<dyn Plugin> = Box::new(Noop);
    }

    #[test] fn default_on_request_returns_unknown_method() {
        let mut p = Noop;
        let req = crate::bus::Request { target: "noop".into(), method: "what".into(), payload: serde_json::json!({}), caller: "x".into() };
        let err = p.on_request(req).unwrap_err();
        assert!(matches!(err, crate::Error::UnknownMethod(m) if m == "what"));
    }
}
