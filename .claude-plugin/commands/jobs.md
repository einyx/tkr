---
description: List current jobs on the jkr JobBoard contract (Open / Taken / Completed) using the jkr_jobs_list MCP tool.
---

Use the `jkr_jobs_list` MCP tool to fetch jobs from the jkr JobBoard.
Default args (no `board` / `rpc_url`) hit the public devnet board.

Render the result as a compact table the user can scan:
- Job id, status, reward in ETH, deadline (relative — "5h", "2d"), preview text
- Skip jobs in `Cancelled` / `TimedOut` status unless the user asked for the full history

After the table, suggest the most relevant next action:
- If there are `Open` jobs and the user hasn't taken one in this session,
  point at `jkr job take <id>` (mention they need a key file with funds).
- If they've already taken one and it's `Taken`, suggest
  `jkr job complete <id> --result-hash 0x…`.
- Otherwise just leave it.

If `$ARGUMENTS` is provided, treat it as a board address override and
pass it to the tool as `{"board": "$ARGUMENTS"}`.
