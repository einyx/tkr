// Three-step install: get the CLI (brew or curl), point your agent
// at the gateway, optionally run shell commands under jkr's sandbox.
// The sandbox step is opt-in by phrasing ("Optional") so users who
// just want the proxy don't feel pushed into more setup.

import { useState } from "react";
import { CopyCmd } from "../CopyCmd";

const BREW_CMD = "brew install jkr";
const CURL_CMD =
  "curl -fsSL https://github.com/einyx/jkr/releases/latest/download/install.sh | bash";

type InstallTab = "brew" | "curl";

export function Install() {
  const host = location.host;
  const anthropic = `export ANTHROPIC_BASE_URL=https://${host}`;
  const openai = `export OPENAI_BASE_URL=https://${host}/v1`;
  const sandboxRun = "jkr sandbox run -- cargo test";
  const sandboxLogin = `jkr login --url https://${host}`;

  const [tab, setTab] = useState<InstallTab>("brew");

  return (
    <section id="install" className="lp-section lp-reveal lp-reveal-delay-2">
      <h2 className="lp-section-title">install</h2>
      <p className="lp-section-body">
        Three steps on Linux or macOS. No agent code changes — just point at
        the gateway URL.
      </p>

      <div className="lp-install">
        <div className="lp-install-step">
          <span className="lp-install-step-num">1</span>
          <div className="lp-install-step-body">
            <p className="lp-install-step-title">Install the CLI</p>
            <div className="lp-tabs" role="tablist" aria-label="Install method">
              <button
                role="tab"
                aria-selected={tab === "brew"}
                className={`lp-tab ${tab === "brew" ? "lp-tab-on" : ""}`}
                onClick={() => setTab("brew")}
                type="button"
              >
                Homebrew
              </button>
              <button
                role="tab"
                aria-selected={tab === "curl"}
                className={`lp-tab ${tab === "curl" ? "lp-tab-on" : ""}`}
                onClick={() => setTab("curl")}
                type="button"
              >
                curl
              </button>
            </div>
            <CopyCmd text={tab === "brew" ? BREW_CMD : CURL_CMD} />
          </div>
        </div>

        <div className="lp-install-step">
          <span className="lp-install-step-num">2</span>
          <div className="lp-install-step-body">
            <p className="lp-install-step-title">Point your agent at the gateway</p>
            <p className="lp-install-step-hint">
              Claude Code reads <code>ANTHROPIC_BASE_URL</code>; Cursor and Codex
              read <code>OPENAI_BASE_URL</code>. Set whichever your tool uses.
            </p>
            <CopyCmd text={anthropic} className="lp-cmd-block lp-cmd-block-sm" />
            <CopyCmd text={openai} className="lp-cmd-block lp-cmd-block-sm" />
          </div>
        </div>

        <div className="lp-install-step">
          <span className="lp-install-step-num">3</span>
          <div className="lp-install-step-body">
            <p className="lp-install-step-title">
              Run shell commands under the sandbox{" "}
              <span className="lp-install-step-opt">optional</span>
            </p>
            <p className="lp-install-step-hint">
              Wrap any command in <code>jkr sandbox run --</code> to execute it
              under Landlock (Linux) or sandbox-exec (macOS). Sign in once to
              stream runs into your dashboard.
            </p>
            <CopyCmd text={sandboxRun} className="lp-cmd-block lp-cmd-block-sm" />
            <CopyCmd text={sandboxLogin} className="lp-cmd-block lp-cmd-block-sm" />
          </div>
        </div>
      </div>
    </section>
  );
}
