use crate::config::Config;
use crate::stream::chars_to_tokens;
use anyhow::Result;
use tkr_analytics::AnalyticsStore;

pub fn run(cfg: Config, args: &[String]) -> Result<()> {
    let (cmd, cmd_args) = args.split_first().expect("at least one arg");
    let cmd_args_str = cmd_args.join(" ");

    let sess = crate::session::Session::connect(&cfg.core.socket_path);
    sess.command_start(cmd, &cmd_args_str);

    let mut chain = crate::dispatch::build_chain(&cfg, cmd)?;
    let str_args: Vec<&str> = cmd_args.iter().map(String::as_str).collect();
    let lines = crate::runner::stream_command(cmd, &str_args)?;
    let result = crate::stream::run_pipeline(lines, &mut chain, cmd, &cmd_args_str);

    let tokens_in = chars_to_tokens(result.chars_in);
    let tokens_saved = chars_to_tokens(result.chars_suppressed);
    sess.command_end(tokens_in, tokens_saved);

    // Record savings to the analytics store. The analytics plugin's flush() can't
    // see suppressed lines (they short-circuit at the filter/semantic plugins),
    // so the proxy records the canonical totals here using PipelineResult.
    let db_path = &cfg.plugins.analytics.db_path;
    if let Some(parent) = std::path::Path::new(db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(store) = AnalyticsStore::open(db_path) {
        let subcmd = cmd_args.first().map(String::as_str).unwrap_or("");
        let _ = store.record(cmd, subcmd, result.chars_in, result.chars_suppressed);
    }

    Ok(())
}
