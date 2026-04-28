use anyhow::{anyhow, Context, Result};
use std::path::Path;
use tkr_agent::{tools::echo::EchoTool, Manifest, RunReceipt, ToolRegistry};
use tkr_providers::AnthropicProvider;

pub fn run_agent(manifest_path: &Path) -> Result<()> {
    let manifest = Manifest::load(manifest_path)
        .with_context(|| format!("loading manifest {}", manifest_path.display()))?;

    let mut tools = ToolRegistry::new();
    for decl in &manifest.tools {
        match decl.name.as_str() {
            "echo" => tools.register(Box::new(EchoTool)),
            other => return Err(anyhow!("unknown tool '{}' (v1 only ships 'echo')", other)),
        }
    }

    let provider = match manifest.model.provider.as_str() {
        "anthropic" => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| anyhow!("ANTHROPIC_API_KEY not set"))?;
            AnthropicProvider::new(key, &manifest.model.name)
        }
        other => return Err(anyhow!("unknown provider '{}' (v1 only ships 'anthropic')", other)),
    };

    let outcome = tkr_agent::run(&manifest, &provider, &mut tools, None)?;
    println!("{}", outcome.final_text);
    println!();
    println!("{}", RunReceipt::from_outcome(&manifest.name, &outcome));
    Ok(())
}
