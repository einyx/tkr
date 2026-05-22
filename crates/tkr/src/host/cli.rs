use anyhow::Result;
use clap::{Arg, Command};
use serde_json::json;
use tkr_api::bus::Bus;
use tkr_api::manifest::CliSpec;

use crate::host::loader::PluginRegistry;

pub struct CliMount {
    pub plugin: String,
    pub specs: Vec<CliSpec>,
}

/// Build the clap subcommand tree for plugin-mounted CLIs.
pub fn build_plugin_cli(mounts: &[CliMount]) -> Command {
    let mut root = Command::new("tkr-plugins").subcommand_required(false);
    for m in mounts {
        // Leak strings to satisfy clap's `Into<Str>` bound (which requires `&'static str`
        // or a `String`-backed type). We only do this at CLI build time (startup, not hot path).
        let plugin_name: &'static str = Box::leak(m.plugin.clone().into_boxed_str());
        let mut plugin_cmd = Command::new(plugin_name);
        for spec in &m.specs {
            let spec_name: &'static str = Box::leak(spec.name.clone().into_boxed_str());
            let sub = Command::new(spec_name).arg(
                Arg::new("args")
                    .num_args(0..)
                    .trailing_var_arg(true)
                    .help("forwarded to plugin"),
            );
            plugin_cmd = plugin_cmd.subcommand(sub);
        }
        root = root.subcommand(plugin_cmd);
    }
    root
}

/// Dispatch `tkr <plugin> <subcmd> [args...]`. Returns exit code.
///
/// `argv[0]` is the plugin name, `argv[1]` the subcommand name; rest are args.
///
/// Plugin registry [`register`](crate::host::loader::PluginRegistry::register) installs a bus
/// handler for `(plugin_name, "cli.invoke")` when the manifest declares `cli_subcommands`.
///
/// NOTE(deferred): Richer per-spec arg parsing (flag extraction, required args, etc.) is
/// deferred. For now, every subcommand accepts trailing positional args as `Vec<String>`.
pub fn dispatch(reg: &PluginRegistry, bus: &dyn Bus, argv: &[String]) -> Result<i32> {
    if argv.len() < 2 {
        return Err(anyhow::anyhow!("usage: tkr <plugin> <subcmd> [args...]"));
    }
    let plugin = &argv[0];
    let subcmd = &argv[1];
    let rest = &argv[2..];

    if !reg.names().contains(plugin) {
        return Err(anyhow::anyhow!("unknown plugin: {plugin}"));
    }

    let req = tkr_api::bus::Request {
        target: plugin.clone(),
        method: "cli.invoke".into(),
        payload: json!({ "subcmd": subcmd, "args": rest }),
        caller: "host".into(),
    };
    let reply = bus.request(req)?;
    // Reply payload shape: { exit_code: i64, stdout: String, stderr: String }
    if let Some(stdout) = reply.payload.get("stdout").and_then(|v| v.as_str()) {
        if !stdout.is_empty() {
            print!("{stdout}");
        }
    }
    if let Some(stderr) = reply.payload.get("stderr").and_then(|v| v.as_str()) {
        if !stderr.is_empty() {
            eprint!("{stderr}");
        }
    }
    let exit = reply
        .payload
        .get("exit_code")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    Ok(exit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::bus::InProcBus;
    use crate::host::loader::PluginRegistry;
    use crate::host::vault::store::{MemStore, Store};
    use crate::host::vault::HostVault;
    use std::sync::Arc;
    use tkr_api::bus::Reply;
    use tkr_api::manifest::Manifest;
    use tkr_api::plugin::Plugin;

    struct EchoPlugin;

    impl Plugin for EchoPlugin {
        fn manifest(&self) -> Manifest {
            Manifest {
                name: "demo".into(),
                version: "0".into(),
                cli_subcommands: vec![tkr_api::manifest::CliSpec {
                    name: "say".into(),
                    args: serde_json::json!({}),
                }],
                capabilities_required: vec![tkr_api::capability::CLI_SUBCOMMAND.into()],
                ..Default::default()
            }
        }
        fn on_load(
            &mut self,
            _host: std::sync::Arc<dyn tkr_api::host::Host>,
        ) -> tkr_api::Result<()> {
            Ok(())
        }
        fn on_request(&mut self, req: tkr_api::bus::Request) -> tkr_api::Result<Reply> {
            if req.method == "cli.invoke" {
                let args: Vec<String> = req
                    .payload
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(Reply {
                    payload: serde_json::json!({
                        "exit_code": 0,
                        "stdout": format!("you said: {}\n", args.join(" ")),
                        "stderr": ""
                    }),
                })
            } else {
                Err(tkr_api::Error::UnknownMethod(req.method))
            }
        }
    }

    fn make_reg_and_bus() -> (PluginRegistry, Arc<InProcBus>) {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [9u8; 32]);
        v.unseal_full();
        let v = Arc::new(v);
        let bus = Arc::new(InProcBus::new());
        let reg = PluginRegistry::new(v.clone(), bus.clone());
        (reg, bus)
    }

    #[test]
    fn dispatch_calls_plugin_cli_invoke() {
        let (mut reg, bus) = make_reg_and_bus();
        let mut caps = tkr_api::capability::CapSet::default();
        caps.grant(tkr_api::capability::CLI_SUBCOMMAND);
        reg.grant("demo", caps);
        reg.register(Box::new(EchoPlugin)).unwrap();

        let argv: Vec<String> = ["demo", "say", "hi", "there"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let exit = dispatch(&reg, bus.as_ref(), &argv).unwrap();
        assert_eq!(exit, 0);
    }

    #[test]
    fn unknown_plugin_errors() {
        let (reg, bus) = make_reg_and_bus();
        let argv: Vec<String> = ["ghost", "say"].iter().map(|s| s.to_string()).collect();
        assert!(dispatch(&reg, bus.as_ref(), &argv).is_err());
    }

    #[test]
    fn build_plugin_cli_mounts_subcommands() {
        let mounts = vec![CliMount {
            plugin: "demo".into(),
            specs: vec![
                CliSpec {
                    name: "say".into(),
                    args: serde_json::json!({}),
                },
                CliSpec {
                    name: "shout".into(),
                    args: serde_json::json!(null),
                },
            ],
        }];
        let cmd = build_plugin_cli(&mounts);
        // Verify structure: root has "demo" subcommand, which has "say" and "shout".
        let sub = cmd.find_subcommand("demo").expect("demo subcommand");
        assert!(sub.find_subcommand("say").is_some());
        assert!(sub.find_subcommand("shout").is_some());
    }
}
