// Dashboard — the AI gateway control surface for authenticated
// operators. Composes one panel per concern; each panel lives in
// components/dashboard/ and owns its own loading/empty/error story.
//
// Polling cadence is set here (not inside the panels) so the whole
// view has a single source of truth for "what's live" — easier to
// reason about thundering herds when adding new endpoints.

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  api,
  type Me,
  type MeshStatus,
  type SessionMeta,
  type LlmRecentResponse,
  type FilterStats,
  type LlmReceiptStats,
  type SandboxStats,
} from "../api";
import { IdentityHeader } from "../components/dashboard/IdentityHeader";
import { TokenUsagePanel } from "../components/dashboard/TokenUsagePanel";
import { GetStartedCard } from "../components/dashboard/GetStartedCard";
import { FilterPanel } from "../components/dashboard/FilterPanel";
import { ReceiptsPanel } from "../components/dashboard/ReceiptsPanel";
import { SandboxPanel } from "../components/dashboard/SandboxPanel";
import { MeshPanel } from "../components/dashboard/MeshPanel";
import { SessionsPanel } from "../components/dashboard/SessionsPanel";
import { CapturedPanel } from "../components/dashboard/CapturedPanel";
import { StatusBanner } from "../components/dashboard/StatusBanner";
import { ReceiptVerifyPanel } from "../components/dashboard/ReceiptVerifyPanel";
import { CliTokensPanel } from "../components/dashboard/CliTokensPanel";

interface Props {
  me: Me;
  onSelectSession: (id: string) => void;
  onSignedOut: () => void;
}

export function DashboardView({ me, onSelectSession, onSignedOut }: Props) {
  const qc = useQueryClient();

  const llm = useQuery<LlmRecentResponse>({
    queryKey: ["llm-recent"],
    queryFn: () => api<LlmRecentResponse>("/api/v1/llm/recent"),
    refetchInterval: 3_000,
  });
  const filter = useQuery<FilterStats>({
    queryKey: ["filter-stats"],
    queryFn: () => api<FilterStats>("/api/v1/filter/stats"),
    refetchInterval: 10_000,
  });
  const receipts = useQuery<LlmReceiptStats>({
    queryKey: ["llm-receipt-stats"],
    queryFn: () => api<LlmReceiptStats>("/api/v1/llm/receipts/stats"),
    refetchInterval: 5_000,
  });
  const sandbox = useQuery<SandboxStats>({
    queryKey: ["sandbox-stats"],
    queryFn: () => api<SandboxStats>("/api/v1/sandbox/stats"),
    refetchInterval: 10_000,
  });
  // Capture endpoint is cheap (always returns enabled+entries shape).
  // Polled here so StatusBanner can show its on/off chip without
  // CapturedPanel having to share state up.
  const captured = useQuery<{ enabled: boolean }>({
    queryKey: ["llm-captured-status"],
    queryFn: () => api<{ enabled: boolean }>("/api/v1/llm/captured"),
    refetchInterval: 30_000,
  });
  const meshStatus = useQuery<MeshStatus>({
    queryKey: ["mesh-status"],
    queryFn: () => api<MeshStatus>("/api/v1/mesh/status"),
    refetchInterval: 5_000,
  });
  const sessionsQuery = useQuery<{ sessions: SessionMeta[] }>({
    queryKey: ["sessions"],
    queryFn: () => api<{ sessions: SessionMeta[] }>("/api/v1/sessions"),
  });

  const logout = useMutation({
    mutationFn: () => api("/api/auth/logout", { method: "POST" }),
    onSuccess: () => {
      qc.removeQueries();
      onSignedOut();
    },
  });

  return (
    <>
      <IdentityHeader me={me} onSignOut={() => logout.mutate()} />
      <main>
        <StatusBanner
          captured={captured.data}
          sandbox={sandbox.data}
          receipts={receipts.data}
        />
        {(llm.data?.entries ?? []).length === 0 ? (
          <GetStartedCard />
        ) : (
          <TokenUsagePanel entries={llm.data!.entries} />
        )}
        <div className="dash-grid-2">
          <FilterPanel stats={filter.data} />
          <ReceiptsPanel stats={receipts.data} />
          <MeshPanel status={meshStatus.data} error={meshStatus.error} />
          <SandboxPanel stats={sandbox.data} />
        </div>
        <CapturedPanel />
        <CliTokensPanel />
        <ReceiptVerifyPanel />
        <SessionsPanel
          sessions={sessionsQuery.data?.sessions}
          error={sessionsQuery.error}
          onSelect={onSelectSession}
        />
      </main>
    </>
  );
}
