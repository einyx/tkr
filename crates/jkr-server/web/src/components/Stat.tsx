// Small stat tile: big monospace value over a muted label. Used in
// every grid of stats on the landing + dashboard, so it gets its own
// component to keep diff noise out of the views when we tweak typography.
//
// Styling lives in styles.css under `.lp-stat` / `.lp-stat-value` /
// `.lp-stat-label`.

export function Stat({
  value,
  label,
  className = "",
}: {
  value: string;
  label: string;
  className?: string;
}) {
  return (
    <div className={`lp-stat${className ? ` ${className}` : ""}`}>
      <div className="lp-stat-value" title={value.length > 14 ? value : undefined}>
        {value}
      </div>
      <div className="lp-stat-label">{label}</div>
    </div>
  );
}
