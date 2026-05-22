---
description: Show a structural outline (symbols + line ranges) of a file using tkr_outline_file instead of dumping its contents.
---

Call the `tkr_outline_file` MCP tool with `path = $ARGUMENTS`.

If `$ARGUMENTS` is empty, ask the user for an absolute path and stop —
don't guess.

After the outline arrives, render it as-is. If the file is short
(< ~30 symbols) and the user is likely about to edit, offer to
`Read` a specific symbol's line range as a follow-up; otherwise just
leave the outline.

This command exists because `Read` on large files is the single biggest
token cost in most sessions — outlining first lets you decide what to
actually read.
