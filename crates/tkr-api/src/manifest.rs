use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default)] pub capabilities_required: Vec<String>,
    #[serde(default)] pub services_exposed: Vec<ServiceSpec>,
    #[serde(default)] pub events_subscribed: Vec<String>,
    #[serde(default)] pub events_emitted: Vec<String>,
    #[serde(default)] pub cli_subcommands: Vec<CliSpec>,
    #[serde(default)] pub storage_requests: Vec<StorageRequest>,
    #[serde(default)] pub config_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSpec {
    pub method: String,
    #[serde(default)] pub required_caps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSpec {
    pub name: String,
    #[serde(default)] pub args: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SensitivityClass { Public, Private, Secret }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum StorageKind { Kv, Sqlite, Fs }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRequest { pub kind: StorageKind, pub class: SensitivityClass }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_serializes() {
        let m = Manifest {
            name: "demo".into(),
            version: "0.1.0".into(),
            capabilities_required: vec!["cap:vault.read.public".into()],
            services_exposed: vec![ServiceSpec { method: "demo.ping".into(), required_caps: vec![] }],
            events_subscribed: vec!["command.finished".into()],
            events_emitted: vec![],
            cli_subcommands: vec![],
            storage_requests: vec![StorageRequest { kind: StorageKind::Kv, class: SensitivityClass::Public }],
            config_schema: serde_json::json!({"type":"object"}),
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("demo.ping"));
    }
}
