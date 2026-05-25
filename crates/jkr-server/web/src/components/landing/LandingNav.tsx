interface Props {
  onSignIn: () => void;
}

const LINKS = [
  { href: "#pipeline", label: "pipeline" },
  { href: "#install", label: "install" },
] as const;

export function LandingNav({ onSignIn }: Props) {
  return (
    <header className="lp-nav">
      <a className="lp-nav-brand" href="#">
        Tkr
      </a>
      <nav className="lp-nav-links" aria-label="Page sections">
        {LINKS.map((l) => (
          <a key={l.href} className="lp-nav-link" href={l.href}>
            {l.label}
          </a>
        ))}
      </nav>
      <div className="lp-nav-actions">
        <a
          className="lp-nav-ghost"
          href="https://github.com/einyx/jkr"
          target="_blank"
          rel="noopener noreferrer"
        >
          github
        </a>
        <button type="button" className="lp-nav-signin" onClick={onSignIn}>
          sign in
        </button>
      </div>
    </header>
  );
}
