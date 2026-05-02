#!/usr/bin/env bash
# Status line for tkr. Reads `tkr gain --plain` and emits one compact line:
#   tkr · saved 8,432 / 28,673 (29%) · 36 cmds today
#
# If tkr isn't on PATH, prints nothing (a status line should fail silently;
# noisy errors here would clobber the prompt on every keystroke).
#
# Wire it up via Claude Code settings.json:
#   {
#     "statusLine": {
#       "type": "command",
#       "command": "${CLAUDE_PLUGIN_ROOT}/statusline.sh",
#       "padding": 1,
#       "refreshInterval": 5
#     }
#   }

set -e
command -v tkr >/dev/null 2>&1 || exit 0

OUT=$(tkr gain --plain 2>/dev/null) || exit 0

# Pull the three numbers + reduction. The format from `tkr gain --plain`:
#   tokens in             28,673
#   tokens saved           8,432  filtered out
#   reduction              29.4%  of total
#   runs                     212  across 36 commands
in_n=$(printf '%s' "$OUT"   | awk '/^  tokens in / {gsub(",", "", $3); print $3; exit}')
save_n=$(printf '%s' "$OUT" | awk '/^  tokens saved / {gsub(",", "", $3); print $3; exit}')
red=$(printf '%s' "$OUT"    | awk '/^  reduction / {print $2; exit}')
cmds=$(printf '%s' "$OUT"   | awk '/^  runs / {print $4; exit}')

# If parsing failed (no analytics yet), bail quietly rather than render
# something half-broken in the prompt area.
[ -n "$save_n" ] && [ -n "$in_n" ] || exit 0

# Format with thousands separators using printf — works on coreutils + bsd.
fmt() { printf "%'d" "$1" 2>/dev/null || printf "%s" "$1"; }
printf 'tkr · saved %s / %s (%s)' "$(fmt "$save_n")" "$(fmt "$in_n")" "$red"
[ -n "$cmds" ] && printf ' · %s cmds today' "$cmds"
printf '\n'
