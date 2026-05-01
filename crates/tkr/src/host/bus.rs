use std::collections::HashMap;
use std::sync::RwLock;
use tkr_api::bus::{Bus, Event, Reply, Request};
use tkr_api::capability::CapSet;
use tkr_api::{Error, Result as ApiResult};

type Handler = Box<dyn Fn(Request) -> ApiResult<Reply> + Send + Sync>;
type Subscriber = Box<dyn Fn(Event) -> ApiResult<()> + Send + Sync>;
type HandlerEntry = (Handler, Vec<String>);

pub struct InProcBus {
    /// (plugin_target, method) -> (handler, required_caps_for_call).
    handlers: RwLock<HashMap<(String, String), HandlerEntry>>,
    /// topic -> subscribers
    subscribers: RwLock<HashMap<String, Vec<(String, Subscriber)>>>,
    /// caller plugin -> capability set
    caps: RwLock<HashMap<String, CapSet>>,
}

impl InProcBus {
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
            subscribers: RwLock::new(HashMap::new()),
            caps: RwLock::new(HashMap::new()),
        }
    }

    pub fn register_handler<F>(
        &self,
        target: impl Into<String>,
        method: impl Into<String>,
        required_caps: Vec<String>,
        f: F,
    ) where
        F: Fn(Request) -> ApiResult<Reply> + Send + Sync + 'static,
    {
        self.handlers
            .write()
            .unwrap()
            .insert((target.into(), method.into()), (Box::new(f), required_caps));
    }

    pub fn subscribe<F>(&self, plugin: impl Into<String>, topic: impl Into<String>, f: F)
    where
        F: Fn(Event) -> ApiResult<()> + Send + Sync + 'static,
    {
        self.subscribers
            .write()
            .unwrap()
            .entry(topic.into())
            .or_default()
            .push((plugin.into(), Box::new(f)));
    }

    pub fn set_caps(&self, plugin: impl Into<String>, caps: CapSet) {
        self.caps.write().unwrap().insert(plugin.into(), caps);
    }
}

impl Bus for InProcBus {
    fn request(&self, req: Request) -> ApiResult<Reply> {
        // Capability check: caller must hold all required caps for this (target, method).
        let key = (req.target.clone(), req.method.clone());
        let handlers = self.handlers.read().unwrap();
        let (handler, required) = handlers
            .get(&key)
            .ok_or_else(|| Error::UnknownMethod(format!("{}::{}", req.target, req.method)))?;
        if !required.is_empty() {
            let caps_map = self.caps.read().unwrap();
            let caller_caps = caps_map.get(&req.caller);
            for cap in required {
                let ok = caller_caps.map(|c| c.holds(cap)).unwrap_or(false);
                if !ok {
                    return Err(Error::CapabilityDenied {
                        cap: cap.clone(),
                        plugin: req.caller.clone(),
                    });
                }
            }
        }
        handler(req)
    }

    fn emit(&self, evt: Event) -> ApiResult<()> {
        let subs = self.subscribers.read().unwrap();
        if let Some(list) = subs.get(&evt.topic) {
            for (_plugin, sub) in list {
                let _ = sub(evt.clone());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_routes_to_target() {
        let bus = InProcBus::new();
        bus.register_handler("burn", "ping", vec![], |req| {
            Ok(Reply {
                payload: json!({"echo": req.method}),
            })
        });
        let r = bus
            .request(Request {
                target: "burn".into(),
                method: "ping".into(),
                payload: json!({}),
                caller: "x".into(),
            })
            .unwrap();
        assert_eq!(r.payload, json!({"echo":"ping"}));
    }

    #[test]
    fn unknown_target_errors() {
        let bus = InProcBus::new();
        let err = bus
            .request(Request {
                target: "nope".into(),
                method: "x".into(),
                payload: json!({}),
                caller: "y".into(),
            })
            .unwrap_err();
        assert!(matches!(err, Error::UnknownMethod(_)));
    }

    #[test]
    fn capability_denied_errors() {
        let bus = InProcBus::new();
        bus.register_handler(
            "burn",
            "secret",
            vec!["cap:vault.read.secret".into()],
            |_req| Ok(Reply { payload: json!({}) }),
        );
        // caller has no caps registered
        let err = bus
            .request(Request {
                target: "burn".into(),
                method: "secret".into(),
                payload: json!({}),
                caller: "demo".into(),
            })
            .unwrap_err();
        assert!(matches!(err, Error::CapabilityDenied { .. }));
    }

    #[test]
    fn capability_granted_succeeds() {
        let bus = InProcBus::new();
        let mut caps = CapSet::default();
        caps.grant("cap:vault.read.secret");
        bus.set_caps("demo", caps);
        bus.register_handler(
            "burn",
            "secret",
            vec!["cap:vault.read.secret".into()],
            |_req| {
                Ok(Reply {
                    payload: json!(true),
                })
            },
        );
        let r = bus
            .request(Request {
                target: "burn".into(),
                method: "secret".into(),
                payload: json!({}),
                caller: "demo".into(),
            })
            .unwrap();
        assert_eq!(r.payload, json!(true));
    }

    #[test]
    fn emit_fanout() {
        let bus = InProcBus::new();
        let received = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let r1 = received.clone();
        let r2 = received.clone();
        bus.subscribe("a", "evt", move |_| {
            r1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        bus.subscribe("b", "evt", move |_| {
            r2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        bus.emit(Event {
            topic: "evt".into(),
            payload: json!({}),
            emitter: "host".into(),
        })
        .unwrap();
        assert_eq!(received.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
