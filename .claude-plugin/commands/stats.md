---
description: Show today's token savings from jkr's filter pipeline — total in/saved/reduction across commands.
---

Run `jkr gain --plain` and summarise the result for the user in 2-3
lines:

- Total tokens-in, tokens-saved, reduction%
- The single biggest contributor (command + saved tokens)
- Today's runs

Don't paste the full table back unless `$ARGUMENTS` contains the word
`full` or `breakdown` — in that case run `jkr gain --plain --breakdown`
and show everything.

If `jkr` isn't on PATH or `jkr gain` errors, surface the error directly
and stop. Don't guess at savings; the analytics DB is the source of
truth.
