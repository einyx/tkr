---
description: List current jobs on the tkr JobBoard contract (Open / Taken / Completed) using the tkr_jobs_list MCP tool.
---

Use the `tkr_jobs_list` MCP tool to fetch jobs from the tkr JobBoard.
Default args (no `board` / `rpc_url`) hit the public devnet board.

Render the result as a compact table the user can scan:
- Job id, status, reward in ETH, deadline (relative — "5h", "2d"), preview text
- Skip jobs in `Cancelled` / `TimedOut` status unless the user asked for the full history

After the table, suggest the most relevant next action:
- If there are `Open` jobs and the user hasn't taken one in this session,
  point at `tkr job take <id>` (mention they need a key file with funds).
- If they've already taken one and it's `Taken`, suggest
  `tkr job complete <id> --result-hash 0x…`.
- Otherwise just leave it.

If `$ARGUMENTS` is provided, treat it as a board address override and
pass it to the tool as `{"board": "$ARGUMENTS"}`.
