import { useQuery } from "@tanstack/react-query";
import { api, type MeshStatus } from "../api";

interface Props {
  onSignIn: () => void;
}

/// MeshEscrow address on the tkr devnet. Deterministic — first deploy
/// from anvil[0] always lands here. If the chain ever gets wiped + we
/// redeploy, this stays valid.
const MESH_ESCROW_ADDR = "0x5FbDB2315678afecb367f032d93F642f64180aa3";

interface ChainStats {
  chainId: number | null;
  blockNumber: number | null;
  gasPriceGwei: number | null;
  blockTimeMs: number | null;       // avg over the last ~10 blocks
  txsInLatestBlock: number | null;
  latestBlockHash: string | null;   // truncated 0x… prefix
  uptimeSec: number | null;         // since chain genesis
  escrowBalanceEth: number | null;  // ETH locked in MeshEscrow
  pendingTxs: number | null;        // anvil txpool size
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

interface RpcBlock {
  number: string;
  hash: string;
  timestamp: string;
  transactions: string[];
}

interface RpcTxpoolStatus {
  pending: string;
  queued: string;
}

async function fetchChainStats(): Promise<ChainStats> {
  const fromHex = (s: string) => parseInt(s, 16);
  const okOr = <T,>(p: PromiseSettledResult<T>): T | null =>
    p.status === "fulfilled" ? p.value : null;

  // First wave: chain id + latest block (timestamp, hash, tx count) + gas
  // price + escrow balance + txpool status. All parallel.
  const [chainId, latest, gasPrice, escrowBal, txpool] = await Promise.allSettled([
    rpc<string>("eth_chainId"),
    rpc<RpcBlock>("eth_getBlockByNumber", ["latest", false]),
    rpc<string>("eth_gasPrice"),
    rpc<string>("eth_getBalance", [MESH_ESCROW_ADDR, "latest"]),
    rpc<RpcTxpoolStatus>("txpool_status"),
  ]);

  const latestBlock = okOr(latest);
  const blockNumber = latestBlock ? fromHex(latestBlock.number) : null;
  const latestTs = latestBlock ? fromHex(latestBlock.timestamp) : null;

  // Second wave: prior block (10 back, capped) + genesis. Both depend on
  // having the latest block number first, so they're sequential after
  // wave 1 but parallel with each other.
  let blockTimeMs: number | null = null;
  let uptimeSec: number | null = null;
  if (blockNumber != null) {
    const priorN = Math.max(0, blockNumber - 10);
    const [prior, genesis] = await Promise.allSettled([
      priorN === blockNumber
        ? Promise.resolve(latestBlock!)
        : rpc<RpcBlock>("eth_getBlockByNumber", [`0x${priorN.toString(16)}`, false]),
      rpc<RpcBlock>("eth_getBlockByNumber", ["0x0", false]),
    ]);
    const priorBlock = okOr(prior);
    if (priorBlock && latestTs != null) {
      const priorTs = fromHex(priorBlock.timestamp);
      const span = blockNumber - fromHex(priorBlock.number);
      if (span > 0) {
        blockTimeMs = Math.round(((latestTs - priorTs) * 1000) / span);
      }
    }
    const genesisBlock = okOr(genesis);
    if (genesisBlock && latestTs != null) {
      uptimeSec = latestTs - fromHex(genesisBlock.timestamp);
    }
  }

  const gasPriceGwei =
    gasPrice.status === "fulfilled" ? Math.round(fromHex(gasPrice.value) / 1e9) : null;
  const escrowBalanceEth =
    escrowBal.status === "fulfilled" ? fromHex(escrowBal.value) / 1e18 : null;

  return {
    chainId: chainId.status === "fulfilled" ? fromHex(chainId.value) : null,
    blockNumber,
    gasPriceGwei,
    blockTimeMs,
    txsInLatestBlock: latestBlock ? latestBlock.transactions.length : null,
    latestBlockHash: latestBlock ? latestBlock.hash.slice(0, 10) : null,
    uptimeSec,
    escrowBalanceEth,
    pendingTxs:
      txpool.status === "fulfilled" ? fromHex(txpool.value.pending) : null,
  };
}

function fmtUptime(sec: number): string {
  if (sec < 60) return `${sec}s`;
  if (sec < 3600) return `${Math.floor(sec / 60)}m`;
  if (sec < 86400) return `${Math.floor(sec / 3600)}h ${Math.floor((sec % 3600) / 60)}m`;
  return `${Math.floor(sec / 86400)}d ${Math.floor((sec % 86400) / 3600)}h`;
}

function fmtEth(eth: number): string {
  if (eth === 0) return "0";
  if (eth < 0.001) return "<0.001";
  if (eth < 1) return eth.toFixed(3);
  if (eth < 1000) return eth.toFixed(2);
  return Math.round(eth).toLocaleString();
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

        <div className="lp-section-title" style={{ marginTop: 18 }}>
          tkr devnet — live
        </div>
        <div className="lp-stats lp-stats-3">
          <Stat
            value={chain?.blockTimeMs != null ? `${chain.blockTimeMs}ms` : "—"}
            label="avg block time"
          />
          <Stat
            value={chain?.txsInLatestBlock != null ? String(chain.txsInLatestBlock) : "—"}
            label="txs in latest block"
          />
          <Stat
            value={chain?.pendingTxs != null ? String(chain.pendingTxs) : "—"}
            label="pending txs"
          />
          <Stat
            value={chain?.escrowBalanceEth != null ? `${fmtEth(chain.escrowBalanceEth)} ETH` : "—"}
            label="locked in MeshEscrow"
          />
          <Stat
            value={chain?.uptimeSec != null ? fmtUptime(chain.uptimeSec) : "—"}
            label="chain uptime"
          />
          <Stat
            value={chain?.latestBlockHash ?? "—"}
            label="latest block hash"
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
