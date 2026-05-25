// Public landing footer — closes with prysm-landing's tkrSpotlight
// tagline ("Token cost is the new build cost.") then the license +
// stack-membership pointers.

export function LandingFooter() {
  return (
    <footer className="lp-footer">
      <p className="lp-footer-tagline">Token cost is the new build cost.</p>
      Apache-2.0 · part of the{" "}
      <a href="https://prysm.sh" target="_blank" rel="noopener noreferrer">
        Prysm
      </a>{" "}
      stack ·{" "}
      <a href="https://github.com/einyx/tkr" target="_blank" rel="noopener noreferrer">
        github.com/einyx/tkr
      </a>
    </footer>
  );
}
