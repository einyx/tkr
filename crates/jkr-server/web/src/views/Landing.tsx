// Public landing — orchestrates section components from
// components/landing/. Data fetching + polling cadence live here.
//
// Flow is intentionally short: Hero → Pillars (what it does) →
// Install (how to use it) → Live (proof, only when non-empty) →
// Footer. Dev tooling (receipt canonicalizer) and internal
// substrate (mesh primitives, jobs board, on-chain stats) live in
// the dashboard for signed-in operators — they muddied the
// conversion path on the public surface.

import { useQuery } from "@tanstack/react-query";
import { api, type LlmRecentResponse } from "../api";
import { LandingNav } from "../components/landing/LandingNav";
import { Hero } from "../components/landing/Hero";
import { LiveStats } from "../components/landing/LiveStats";
import { Pillars } from "../components/landing/Pillars";
import { Install } from "../components/landing/Install";
import { LandingFooter } from "../components/landing/LandingFooter";

interface Props {
  onSignIn: () => void;
}

export function LandingView({ onSignIn }: Props) {
  const { data: llm } = useQuery<LlmRecentResponse>({
    queryKey: ["llm-recent"],
    queryFn: () => api<LlmRecentResponse>("/api/v1/llm/recent"),
    refetchInterval: 5_000,
  });

  const entries = llm?.entries ?? [];

  return (
    <div className="lp-shell">
      <LandingNav onSignIn={onSignIn} />
      <main className="lp">
        <Hero onSignIn={onSignIn} />
        <Pillars />
        <Install />
        {entries.length > 0 && (
          <section className="lp-section lp-reveal">
            <h2 className="lp-section-title">live</h2>
            <p className="lp-section-body">
              Recent calls through this instance — signed receipts only, no
              prompt bodies.
            </p>
            <LiveStats entries={entries} />
          </section>
        )}
        <LandingFooter />
      </main>
    </div>
  );
}
