// Dashboard panel chrome — the boxed container with a header strip
// (status dot + title on the left, count/badge on the right) and an
// arbitrary children body. Used by every section of the Dashboard
// view, which is where the repetition lived before.
//
// `dot` is the colour key for the leading status indicator:
//   • "live" — pulsing accent (default for healthy data feeds)
//   • "warn" — pulsing amber (operator attention needed)
//   • "off"  — neutral, no animation (placeholder / not-yet-wired)
// Classes line up with `.status-dot` / `.status-dot.live` /
// `.status-dot.warn` in styles.css.

import type { ReactNode } from "react";

export type PanelDot = "live" | "warn" | "off";

interface PanelProps {
  title: ReactNode;
  count?: ReactNode;
  /** Extra class on the right-hand count slot. Useful for `muted`. */
  countClass?: string;
  /** Status indicator state. Defaults to `live`. */
  dot?: PanelDot;
  children: ReactNode;
}

export function Panel({
  title,
  count,
  countClass,
  dot = "live",
  children,
}: PanelProps) {
  const dotClass = dot === "off" ? "status-dot" : `status-dot ${dot}`;
  return (
    <div className="panel">
      <div className="panel-head">
        <span>
          <span className={dotClass} />
          {title}
        </span>
        {count != null && (
          <span className={countClass ? `count ${countClass}` : "count"}>
            {count}
          </span>
        )}
      </div>
      {children}
    </div>
  );
}
