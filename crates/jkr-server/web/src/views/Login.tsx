// Sign-in is Logto-only and seamless: hitting the login route kicks
// the browser straight to `/auth/logto/start`, skipping the previous
// "Continue with Prysm" interstitial. Logto handles the entire UX
// from there — there's nothing for jkr to add at this step that
// isn't friction.
//
// The dev password fallback used to live below the button. Backend
// `/api/auth/login` is still in place for test fixtures (mesh_e2e.rs)
// and the initial setup flow; it's just no longer surfaced.

import { useEffect } from "react";

const LOGTO_START_PATH = "/auth/logto/start";

export function LoginView() {
  useEffect(() => {
    // `replace` (not assign) so the browser's back button doesn't
    // bounce the user into a blank jkr page mid-redirect.
    window.location.replace(LOGTO_START_PATH);
  }, []);

  return (
    <main>
      <div className="login-card">
        <p className="login-redirect">redirecting to sign-in…</p>
      </div>
    </main>
  );
}
