# Native handlers (`tkr`)

tkr can run **structured, RTK-style handlers** for some commands *before* the generic TOML line-filter pipeline. These paths capture child output (streaming where possible), compress or cap it in Rust, then record the same analytics as normal proxy runs.

## Commands

| Shell command | Env to disable | Notes |
|---------------|----------------|--------|
| `grep`, `egrep`, `fgrep`, `rg` | `TKR_NATIVE_GREP=0` | Groups hits by file, caps lines per file and total, drops noisy paths (same intent as `filters/grep.toml`). Streams stdout. Parses Windows `C:\…` paths and optional `--column` output. For **`rg` with `--json`** / `--json-lines` (ripgrep JSON stream), parses match records and prints the same grouped summary. |
| `cat` (positional files only) | `TKR_NATIVE_READ=0` | No flags, existing files only; reads each file with line/byte caps. |
| `ls` | `TKR_NATIVE_LS=0` | Streams lines with a line cap and per-line byte cap (huge dirs). |
| `git` (`diff`) | `TKR_NATIVE_GIT_DIFF=0` | Condenses unified diffs (drop `index`, collapse context, shorten `@@` headers). Skips if stdout > 8 MiB, `--stat`/`--word-diff`/etc., or passthrough wanted. |
| `cargo` (`test`) | `TKR_NATIVE_CARGO_TEST=0` | Streams merged stdout/stderr, elides `test … ok` lines (failure-first), caps repetitive `Compiling`/`Checking` noise. |
| `go` (`test`) | `TKR_NATIVE_GO_TEST=0` | Elides verbose **`=== RUN`** / **`--- PASS`** lines; skips native when **`-json`**, **`-bench`**, or **`-fuzz`** is set. |
| `npm`, …, `deno`, `bun`, **`jest`**, **`vitest`**, **`mocha`**, **`playwright`**, **`cypress`** | `TKR_NATIVE_JS_TEST=0` | **Package managers** (as before); **standalone** **`jest` / `vitest` / `mocha`**; **`playwright test`**, **`cypress run`**. Elides vitest **✓**, jest **PASS**, **`deno` `test … ok`**, **`bun` `(pass)`**. |
| **`pytest`** / **`python -m pytest`** / **`uv run pytest`** / **`poetry`/`pipenv`/`pdm run pytest`** | `TKR_NATIVE_PYTEST=0` | Elides verbose **`::… PASSED`** lines and long **`.py … dots … [NN%]`** progress rows; keeps failures, errors, skips, xfails. |
| `git` (`status`) | `TKR_NATIVE_GIT=0` | When eligible, runs `git status -sb` instead of long porcelain. |
| `git` (`add` / `commit` / `push` / `pull`) | `TKR_NATIVE_GIT_COMPACT=0` | RTK-style **success** summaries (see below). Failures print **full** stdout/stderr unchanged. |

## Environment variables

### Git

| Variable | Default | Meaning |
|----------|---------|---------|
| `TKR_NATIVE_GIT` | (on) | Set to `0` to disable **all** native `git` shortcuts (status + diff condense + compact transactions). |
| `TKR_NATIVE_GIT_DIFF` | (on) | Set to `0` / `passthrough` so **`git diff`** uses the normal stream + `filters/git.toml` instead of unified-diff condense. |
| `TKR_NATIVE_GIT_COMPACT` | (on) | Set to `0` so **`git add` / `git commit` / `git push` / `git pull`** use the streaming pipeline + **`filters/git.toml`** instead of one-line success output. |

Native **`git status`** rewrites to **`git status -sb`** when we do not see `--porcelain`, verbose, or stash-only long flags.

Native **`git diff`** runs the real `git diff`, then post-processes **stdout** when practical: drops `index` / similarity lines, collapses repeated unified-diff **context** (` `) lines, and shortens long `@@` headers. Skips when `--stat`, `--word-diff`, `--output`, etc. are present; diffs larger than **8 MiB** fall back to the TOML pipeline.

**Compact transactions** (aligned with [RTK](https://github.com/rtk-ai/rtk)-style summaries):

- **`git add`** — on success: `ok`. Passthrough (full stream + TOML) for `-i` / `-p` / `--patch` / `--interactive` / `--dry-run` / `-n`.
- **`git commit`** — on success: `ok · <short-sha> · <subject>` when stderr contains the usual `[branch sha] subject` line; otherwise `ok · commit`. Only runs when the invocation is **non-interactive** (e.g. `-m`, `-F`, `--no-edit`, `--reuse-message`, …); bare `git commit` still opens an editor and stays on the TOML path. Passthrough for `-v` / `--verbose` / `--dry-run`.
- **`git push`** — on success: `ok · <branch>` from ref-update lines, `ok · up to date` when everything is up to date, or `ok · push` if no pattern matched. Passthrough for `--dry-run` / `-n` / `--progress`.
- **`git pull`** — on success: `ok · Nf +I -D` from the `files changed` summary when present, `ok · up to date` when already up to date, else `ok · pull`. Passthrough for `--dry-run` / `-n` / `--progress`.

Any **non-zero exit** from git prints the **original** stdout and stderr so nothing is lost on failure.

### ls

| Variable | Default | Meaning |
|----------|---------|---------|
| `TKR_NATIVE_LS` | (on) | Set to `0` to spawn `ls` and use bundled `filters/ls.toml` (or your overrides). |
| `TKR_NATIVE_LS_MAX_LINES` | `400` | Max lines emitted from **stdout**. |
| `TKR_NATIVE_LS_MAX_LINE` | `512` | Byte cap per line (UTF-8 safe truncation). |

### Cargo (`test`)

| Variable | Default | Meaning |
|----------|---------|---------|
| `TKR_NATIVE_CARGO_TEST` | (on) | Set to `0` to use **`filters/cargo.toml`** on streamed `cargo test` output. |
| `TKR_NATIVE_CARGO_COMPILE_LINES` | `8` | Max `Compiling` / `Checking` / … lines shown before eliding the rest. |

### Go (`go test`)

| Variable | Default | Meaning |
|----------|---------|---------|
| `TKR_NATIVE_GO_TEST` | (on) | Set to `0` / `false` / `off` to use **`filters/go.toml`** (or your overrides) on streamed **`go test`** output only. |

When **`-json`**, **`-bench`**, or **`-fuzz`** appears before **`--`**, the native path is bypassed (tool-specific streaming formats).

### Session trail

| Variable | Meaning |
|----------|---------|
| `TKR_NATIVE_SESSION_LOG` | Set to `1` to append one JSON object per **native** run to `~/.tkr/native-handlers.jsonl` (command, args, `chars_in`, `chars_saved`, exit code, unix timestamp). |

### Grep / ripgrep

| Variable | Default | Meaning |
|----------|---------|---------|
| `TKR_NATIVE_GREP` | (on) | Set to `0` / `false` / `off` to use **only** `filters/grep.toml`. |
| `TKR_GREP_NATIVE_RAW_MAX` | `8192` | If **total stdout** stays under this many bytes, print **raw** grep output (no grouping). Use `0` to **always** compress. |
| `TKR_GREP_NATIVE_MAX_RESULTS` | `50` | Max match lines emitted after grouping. |
| `TKR_GREP_NATIVE_PER_FILE` | `2` | Max lines per file. |
| `TKR_GREP_NATIVE_MAX_LINE` | `200` | Character cap per match line. |

**`rg --json`** — uses the same caps as above; stderr is passed through unchanged. Plain `rg` without `--json` still uses the text-line path (optional small-output raw buffer via `TKR_GREP_NATIVE_RAW_MAX`).

### JavaScript / Deno / Bun test runs (`npm`, `pnpm`, `jest`, `vitest`, `deno`, `bun`, …)

| Variable | Default | Meaning |
|----------|---------|---------|
| `TKR_NATIVE_JS_TEST` | (on) | Set to `0` / `false` / `off` so **npm-style** invocations use **`filters/npm.toml`** / **`pnpm.toml`** / **`yarn.toml`** (or your overrides) only — and the same flag disables **Deno** / **Bun** native test shrinking. |

### Pytest

| Variable | Default | Meaning |
|----------|---------|---------|
| `TKR_NATIVE_PYTEST` | (on) | Set to `0` / `false` / `off` to use **`filters/`** TOML only (e.g. **`filters/pytest.toml`** if you maintain one). |

Applies when the executable is **`pytest`** / **`py.test`**, **`python*`** / **`py`** with **`-m pytest`**, or **`uv`/`poetry`/`pipenv`/`pdm run pytest`** (including **`run python -m pytest`** for the poetry-style tools). Color/codes in verbose lines are normalized before matching.

### Read / cat

| Variable | Default | Meaning |
|----------|---------|---------|
| `TKR_NATIVE_READ` | (on) | Set to `0` to disable native cat. |
| `TKR_NATIVE_READ_MAX_LINES` | `400` | Lines kept **per file**. |
| `TKR_NATIVE_READ_MAX_LINE` | `800` | Byte-oriented cap per output line (UTF-8 safe). |

### All proxied commands

| Variable | Default | Meaning |
|----------|---------|---------|
| `TKR_MAX_TOKENS` | (unset) | Hard cap on emitted tokens (see `tkr --help` proxy flags). Helps long `cargo test`, vitest, docker logs. |
| `TKR_TEE` | `failures` | RTK-style transcript retention under **`~/.tkr/tee/`**. `never` / `0` / `false` / `off`: no capture. **`failures`** (also `on`, `yes`, `true`, `1`): keep a raw merged stdout+stderr transcript only when the child exits **non-zero**, then print `[tkr: full output saved to …]`. **`always`** / **`all`**: save every run. |
| `TKR_TEE_MAX_BYTES` | `8388608` | Cap on raw transcript size (bytes); longer runs are truncated with a footer marker. |

The **`TKR_TEE`** transcript is captured **before** line filtering (what the subprocess actually wrote). Native shortcuts (`TKR_NATIVE_*`) already surface failures inline where implemented; tee applies to the **streaming** proxy path.

**Child exit status:** On the streaming filter path, **`tkr`** exits with the proxied process’s exit code (same as native handlers), so hooks and CI see real failure statuses.

## IDE and agent tools

Many assistants expose **Grep**, **Read**, and **Glob** as built-in tools. Those calls **do not** go through your shell, so **`tkr` hooks never see them**. To get compression and analytics, use shell commands instead, e.g. **`tkr rg pattern`**, **`tkr cat file`**, or `tkr head …` (the latter uses the normal filter pipeline unless a future native handler exists).

This is the same class of limitation described for similar proxies in the broader ecosystem; routing search/read through `tkr …` is the reliable fix.

## Smarter analytics

Run **`tkr suggest`** periodically: it surfaces low–savings-ratio commands, sample **suppress_regex** candidates from noise signatures, and prints reminders about native handlers, **`TKR_MAX_TOKENS`**, and custom filters under `~/.tkr/filters/`.

With the optional **`embeddings`** cargo feature, near-duplicate noise lines can be clustered for shared rules.

## Possible next steps

- Richer **session replay** UI on top of `native-handlers.jsonl`.
- Richer **multiline** / **pcre2** grep modes beyond the JSON match summary.
