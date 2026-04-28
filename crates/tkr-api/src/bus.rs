use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub target: String,
    pub method: String,
    #[serde(default)] pub payload: Value,
    pub caller: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    #[serde(default)] pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub topic: String,
    #[serde(default)] pub payload: Value,
    pub emitter: String,
}

pub trait Bus: Send + Sync {
    fn request(&self, req: Request) -> Result<Reply>;
    fn emit(&self, evt: Event) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips() {
        let r = Request {
            target: "burn".into(),
            method: "today_spend".into(),
            payload: serde_json::json!({}),
            caller: "mesh".into(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: Request = serde_json::from_str(&s).unwrap();
        assert_eq!(back.method, "today_spend");
    }

    #[test]
    fn event_round_trips() {
        let e = Event {
            topic: "command.finished".into(),
            payload: serde_json::json!({"ms": 12}),
            emitter: "host".into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back.topic, "command.finished");
    }
}
