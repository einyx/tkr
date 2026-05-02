import { useQuery } from "@tanstack/react-query";
import { api, type MeshStatus } from "../api";

interface Props {
  onSignIn: () => void;
}

interface ChainStats {
  chainId: number | null;
  blockNumber: number | null;
  gasPriceGwei: number | null;
}

async function rpc<T = unknown>(method: string, params: unknown[] = []): Promise<T> {
  const res = await fetch("/api/v1/chain/rpc", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  if (!res.ok) throw new Error(`rpc ${method}: ${res.status}`);
  const j = await res.json();
  if (j.error) throw new Error(`rpc ${method}: ${j.error.message ?? "error"}`);
  return j.result as T;
}

async function fetchChainStats(): Promise<ChainStats> {
  // Hit the three RPCs in parallel; tolerate failures (devnet unreachable).
  const [chainId, blockNumber, gasPrice] = await Promise.allSettled([
    rpc<string>("eth_chainId"),
    rpc<string>("eth_blockNumber"),
    rpc<string>("eth_gasPrice"),
  ]);
  const fromHex = (s: string) => parseInt(s, 16);
  return {
    chainId: chainId.status === "fulfilled" ? fromHex(chainId.value) : null,
    blockNumber: blockNumber.status === "fulfilled" ? fromHex(blockNumber.value) : null,
    gasPriceGwei:
      gasPrice.status === "fulfilled"
        ? Math.round(fromHex(gasPrice.value) / 1e9)
        : null,
  };
}

export function LandingView({ onSignIn }: Props) {
  const { data: status } = useQuery<MeshStatus>({
    queryKey: ["mesh-status"],
    queryFn: () => api<MeshStatus>("/api/v1/mesh/status"),
    refetchInterval: 5_000,
  });

  const { data: chain } = useQuery<ChainStats>({
    queryKey: ["chain-stats"],
    queryFn: fetchChainStats,
    refetchInterval: 4_000,
  });

  const total_connected = status?.total_connected ?? 0;
  const total_meshes = status?.total_meshes ?? 0;
  const enrolled = (status?.meshes ?? []).reduce(
    (a, m) => a + (m.enrolled ?? 0),
    0,
  );
  const brokerWss = `wss://${location.host}/api/v1/mesh/ws`;
  const chainRpc = `https://${location.host}/api/v1/chain/rpc`;

  return (
    <main className="lp">
      <section className="lp-hero">
        <h1 className="lp-title">tkr</h1>
        <p className="lp-subtitle">
          token-optimized CLI proxy + <strong>agent mesh</strong> + on-chain
          payments.
          <br />
          encrypted peer messaging through this broker, EIP-712 receipts,
          MeshEscrow on Base.
        </p>
        <div className="lp-stats">
          <Stat value={String(total_connected)} label="peers online" />
          <Stat value={String(total_meshes)} label="meshes" />
          <Stat value={String(enrolled)} label="members enrolled" />
          <Stat
            value={chain?.blockNumber != null ? `#${chain.blockNumber}` : "—"}
            label={
              chain?.chainId != null
                ? `chain ${chain.chainId} · ${chain.gasPriceGwei ?? "?"} gwei`
                : "tkr devnet"
            }
          />
        </div>
        <div className="lp-ctas">
          <a
            className="lp-cta lp-cta-primary"
            href="https://github.com/einyx/tkr"
            target="_blank"
            rel="noopener noreferrer"
          >
            github →
          </a>
          <a
            className="lp-cta lp-cta-secondary"
            href="#"
            onClick={(e) => {
              e.preventDefault();
              onSignIn();
            }}
          >
            sign in
          </a>
        </div>
      </section>

      <section>
        <CmdBlock
          label="install"
          cmd="curl -fsSL https://github.com/einyx/tkr/releases/latest/download/install.sh | bash"
        />
      </section>

      <section>
        <div className="lp-install">
          <div className="lp-install-label">join the mesh</div>
          <pre className="lp-cmd">{`tkr mesh join <invite-url>
tkr mesh tail demo                   # listen
tkr mesh send demo --to <addr> --recipient-pubkey <pub> 'hi'`}</pre>
          <div className="lp-broker-line">
            broker: <code>{brokerWss}</code>
            <br />
            chain rpc: <code>{chainRpc}</code>
          </div>
        </div>
      </section>

      <section className="lp-features-section">
        <h2 className="lp-section-title">what's inside</h2>
        <div className="lp-features">
          <Feature
            title="filter"
            body="60–95% fewer tokens on git, ls, npm, find, grep, cargo, kubectl, docker. Filters are TOML rules — extend without recompiling."
          />
          <Feature
            title="mesh"
            body="secp256k1 identity = wallet address. EIP-712 invites verifiable in any wallet. WSS broker, ECDH+AES-GCM, encrypted DMs the broker can't read."
          />
          <Feature
            title="pay"
            body="MeshEscrow.sol on Base: open channel, sign EIP-712 receipts off-chain, claim on-chain. Forge-tested, end-to-end demo via `make demo-payment`."
          />
        </div>
      </section>

      <footer className="lp-footer">
        Apache-2.0 ·{" "}
        <a
          href="https://github.com/einyx/tkr"
          target="_blank"
          rel="noopener noreferrer"
        >
          github.com/einyx/tkr
        </a>{" "}
        · live at this broker
      </footer>
    </main>
  );
}

function Stat({ value, label }: { value: string; label: string }) {
  return (
    <div className="lp-stat">
      <div className="lp-stat-value">{value}</div>
      <div className="lp-stat-label">{label}</div>
    </div>
  );
}

function Feature({ title, body }: { title: string; body: string }) {
  return (
    <div className="lp-feature">
      <div className="lp-feature-title">{title}</div>
      <div className="lp-feature-body">{body}</div>
    </div>
  );
}

function CmdBlock({ label, cmd }: { label: string; cmd: string }) {
  return (
    <div className="lp-install">
      <div className="lp-install-label">{label}</div>
      <pre className="lp-cmd">{cmd}</pre>
    </div>
  );
}
