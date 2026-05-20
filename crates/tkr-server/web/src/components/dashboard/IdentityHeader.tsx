// Signed-in user header for the Dashboard. Logto-authenticated
// sessions carry `displayName` + `email`; the password-fallback dev
// login carries dev defaults — either way we show the best available
// label. The sign-out link runs the parent's mutation, which clears
// react-query and bounces back to the public landing.

import type { Me } from "../../api";

interface Props {
  me: Me;
  onSignOut: () => void;
}

export function IdentityHeader({ me, onSignOut }: Props) {
  const showEmailSuffix =
    me.user.email && me.user.email !== me.user.displayName;
  return (
    <header>
      <div className="brand">tkr</div>
      <div className="who">
        {me.user.displayName || me.user.email || "anon"}
        {showEmailSuffix ? <span className="muted"> · {me.user.email}</span> : null}
        {" · "}
        <a
          href="#"
          onClick={(e) => {
            e.preventDefault();
            onSignOut();
          }}
        >
          sign out
        </a>
      </div>
    </header>
  );
}
