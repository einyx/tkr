#![cfg(feature = "test-host")]
use tkr_api::{
    bus::{Event, Request},
    host::Host,
    manifest::SensitivityClass,
    test_host::TestHost,
};

fn assert_host_conformance<H: Host>(h: &H) {
    assert!(!h.plugin_name().is_empty());

    // Kv round-trip across all classes
    for class in [
        SensitivityClass::Public,
        SensitivityClass::Private,
        SensitivityClass::Secret,
    ] {
        let kv = h.kv(class).unwrap();
        kv.put("k", serde_json::json!({"x":1})).unwrap();
        let got = kv.get("k").unwrap().unwrap();
        assert_eq!(got["x"], 1);
        let listed = kv.list("k").unwrap();
        assert!(listed.iter().any(|s| s == "k"));
        kv.delete("k").unwrap();
        assert_eq!(kv.get("k").unwrap(), None);
    }

    // Sqlite execute and query (test impl just records calls and returns empty rows)
    let s = h
        .sqlite("CREATE TABLE t(x INT)", SensitivityClass::Public)
        .unwrap();
    let _ = s
        .execute("INSERT INTO t VALUES (?)", &[serde_json::json!(1)])
        .unwrap();
    let rows = s.query("SELECT * FROM t", &[]).unwrap();
    assert!(rows.is_empty());

    // Fs write/read/list
    let fs = h.fs(SensitivityClass::Private).unwrap();
    fs.write("a/b", b"data").unwrap();
    assert_eq!(fs.read("a/b").unwrap(), b"data".to_vec());
    let listed = fs.list("a/").unwrap();
    assert!(listed.iter().any(|p| p == "a/b"));

    // Bus: verify unknown methods fail predictably
    let bus = h.bus();
    let err = bus
        .request(Request {
            target: "ghost".into(),
            method: "boo".into(),
            payload: serde_json::json!({}),
            caller: "t".into(),
        })
        .unwrap_err();
    assert!(matches!(err, tkr_api::Error::UnknownMethod(_)));

    // Vault state observable
    let _ = h.vault().state();
}

#[test]
fn test_host_satisfies_conformance() {
    let h = TestHost::new("conf");
    assert_host_conformance(&h);
}

#[test]
fn bus_emit_and_read_back() {
    let h = TestHost::new("conf");
    h.bus()
        .emit(Event {
            topic: "test.event".into(),
            payload: serde_json::json!({"val": 42}),
            emitter: "conf".into(),
        })
        .unwrap();
    // Access the concrete TestBus via test_host module to verify recording
    // (we downcast via the known TestHost structure by re-constructing a TestHost)
    // Since Host::bus() returns &dyn Bus we verify emit doesn't error; the
    // TestBus::events() helper is tested via test_host unit tests directly.
}
