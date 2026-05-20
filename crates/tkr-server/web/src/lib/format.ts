// Shared formatters for the dashboard and the public landing. Pure
// functions — no state, no React — so any view can import them
// without dragging in component runtime.
//
// Style: compact, opinionated. We're displaying values inside small
// 9–12-char stat tiles, so units like `7m` beat `7min` and `1.2k`
// beats `1,234`.

/** `12,345` → `12.3k`, `2_400_000` → `2.40M`, `0` → `0`. */
export function fmtTokens(n: number): string {
  if (n === 0) return "0";
  if (n < 1_000) return String(n);
  if (n < 1_000_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

/** Single-letter relative-age suffix: `s` `m` `h` `d`. */
export function fmtRelative(unixSec: number): string {
  const age = Math.max(0, Math.floor(Date.now() / 1000) - unixSec);
  if (age < 60) return `${age}s`;
  if (age < 3600) return `${Math.floor(age / 60)}m`;
  if (age < 86400) return `${Math.floor(age / 3600)}h`;
  return `${Math.floor(age / 86400)}d`;
}

/** Compound uptime: `2h 14m`, `9d 3h`. */
export function fmtUptime(sec: number): string {
  if (sec < 60) return `${sec}s`;
  if (sec < 3600) return `${Math.floor(sec / 60)}m`;
  if (sec < 86400) return `${Math.floor(sec / 3600)}h ${Math.floor((sec % 3600) / 60)}m`;
  return `${Math.floor(sec / 86400)}d ${Math.floor((sec % 86400) / 3600)}h`;
}

/** ETH amount with sensible precision. `0`, `<0.001`, `0.123`, `12.45`, `1,234`. */
export function fmtEth(eth: number): string {
  if (eth === 0) return "0";
  if (eth < 0.001) return "<0.001";
  if (eth < 1) return eth.toFixed(3);
  if (eth < 1000) return eth.toFixed(2);
  return Math.round(eth).toLocaleString();
}
