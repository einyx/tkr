//! Placeholder for Task 4.2; full impl lands next commit.
pub struct InProcBus;
impl InProcBus {
    pub fn new() -> Self {
        Self
    }
}
impl tkr_api::bus::Bus for InProcBus {
    fn request(&self, _req: tkr_api::bus::Request) -> tkr_api::Result<tkr_api::bus::Reply> {
        Err(tkr_api::Error::UnknownMethod("placeholder bus".into()))
    }
    fn emit(&self, _evt: tkr_api::bus::Event) -> tkr_api::Result<()> {
        Ok(())
    }
}
