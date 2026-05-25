---
name: using-jkr-tools
description: Use when reading large source files, locating symbol definitions, or running recursive greps — prefer jkr_outline_file / jkr_find_symbol / jkr_grep_summary over native Read/Grep/Glob to cut token cost.
---

# jkr — token-saving tool selection

When the `jkr` MCP server is available, prefer its tools over native
`Read` / `Grep` / `Glob` in these cases:

| Situation                                              | Use                                                          |
|--------------------------------------------------------|--------------------------------------------------------------|
| Need to know what's in a file > ~200 lines             | `jkr_outline_file`                                           |
| Looking for a specific symbol's definition             | `jkr_find_symbol`                                            |
| Recursive grep that may match across many files        | `jkr_grep_summary`                                           |
| Need actual line ranges of a function before editing   | `jkr_outline_file` then native `Read` with `offset`/`limit`  |
| Listing jobs on the jkr JobBoard                       | `jkr_jobs_list`                                              |

Native `Read` is fine for small files (< 200 lines) and for the exact
ranges you've already pinpointed.

Native `Grep` is fine for narrow searches with `path` / `type` /
`head_limit` already constraining the result set.

These are guidelines, not hard rules — pick the tool that lets you
answer the actual question with the fewest tokens.

## When this skill activates

The plugin's MCP server (`jkr mcp`) registers four tools. This skill
nudges the model toward them when the user is asking about file
contents or symbol locations. Skip the skill for:

- "open this file" with a known small file
- exact-line edits where you already know the range
- `Bash` commands (different code path — handled by the plugin's
  PreToolUse hook, not by these tools)

## Requires

- `jkr` binary on PATH (install via `cargo install --path crates/jkr`
  from a clone, or `curl -fsSL https://tkr.prysm.sh/install.sh | sh`).
- Restart Claude Code after `/plugin install` so the MCP server boots.

## Slash commands shipped with the plugin

- `/jkr-plugin:jobs [board?]` — list current JobBoard jobs
- `/jkr-plugin:stats [full?]` — today's token savings (uses `jkr gain`)
- `/jkr-plugin:mesh [slug?]` — live mesh peer count + escrow balance
- `/jkr-plugin:outline <path>` — file outline via `jkr_outline_file`

## Optional: status line

The plugin ships `statusline.sh` that emits e.g.
`jkr · saved 8,432 / 28,673 (29.4%) · 36 cmds today`.

To enable it, add to your `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "${CLAUDE_PLUGIN_ROOT}/statusline.sh",
    "padding": 1,
    "refreshInterval": 5
  }
}
```

(`${CLAUDE_PLUGIN_ROOT}` resolves to the plugin's install location at
runtime.) Skip if you don't want extra prompt-line noise.
