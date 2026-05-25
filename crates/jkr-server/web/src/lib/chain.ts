// Browser-side EVM JSON-RPC client used by the Landing page to surface
// jkr-devnet vitals. Routes through jkr-server's `/api/v1/chain/rpc`
// passthrough so the browser doesn't need direct access to the anvil
// node — same-origin, no CORS dance.
//
// Lifted out of Landing.tsx so the public page doesn't carry 150 lines
// of RPC plumbing in the middle of its render tree.

/// MeshEscrow address on the jkr devnet. Deterministic — first deploy
/// from anvil[0] always lands here.
export const MESH_ESCROW_ADDR = "0x5FbDB2315678afecb367f032d93F642f64180aa3";

export interface ChainStats {
  chainId: number | null;
  blockNumber: number | null;
  gasPriceGwei: number | null;
  blockTimeMs: number | null;       // avg over the last ~10 blocks
  txsInLatestBlock: number | null;
  latestBlockHash: string | null;   // truncated 0x… prefix
  uptimeSec: number | null;         // since chain genesis
  escrowBalanceEth: number | null;  // ETH locked in MeshEscrow
  pendingTxs: number | null;        // anvil txpool size
  priorityFeeGwei: number | null;   // eth_maxPriorityFeePerGas
  blockUtilization: number | null;  // gasUsed / gasLimit, percent
  blockSizeBytes: number | null;    // RLP-encoded block size
  miner: string | null;             // truncated 0x… prefix
  queuedTxs: number | null;         // anvil txpool queued
  clientVersion: string | null;     // web3_clientVersion (anvil/<ver>)
}

interface RpcBlock {
  number: string;
  hash: string;
  timestamp: string;
  transactions: string[];
  miner?: string;
  size?: string;
  gasUsed?: string;
  gasLimit?: string;
}

interface RpcTxpoolStatus {
  pending: string;
  queued: string;
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

/// Fetch every chain stat we display in one parallel wave. Each
/// individual call is wrapped in `Promise.allSettled` so a single
/// upstream hiccup (e.g. the second block-time sample failing)
/// degrades the affected tile to `null` instead of nuking the whole
/// dashboard.
export async function fetchChainStats(): Promise<ChainStats> {
  const fromHex = (s: string) => parseInt(s, 16);
  const okOr = <T,>(p: PromiseSettledResult<T>): T | null =>
    p.status === "fulfilled" ? p.value : null;

  // First wave: chain id + latest block + gas price + escrow balance +
  // txpool + priority fee + client version. All parallel.
  const [
    chainId,
    latest,
    gasPrice,
    escrowBal,
    txpool,
    priorityFee,
    clientVersion,
  ] = await Promise.allSettled([
    rpc<string>("eth_chainId"),
    rpc<RpcBlock>("eth_getBlockByNumber", ["latest", false]),
    rpc<string>("eth_gasPrice"),
    rpc<string>("eth_getBalance", [MESH_ESCROW_ADDR, "latest"]),
    rpc<RpcTxpoolStatus>("txpool_status"),
    rpc<string>("eth_maxPriorityFeePerGas"),
    rpc<string>("web3_clientVersion"),
  ]);

  const latestBlock = okOr(latest);
  const blockNumber = latestBlock ? fromHex(latestBlock.number) : null;
  const latestTs = latestBlock ? fromHex(latestBlock.timestamp) : null;

  // Second wave: sample older block for avg-block-time + chain uptime.
  // Skipped if we couldn't read the latest block.
  let blockTimeMs: number | null = null;
  let uptimeSec: number | null = null;
  if (blockNumber != null && blockNumber > 10) {
    const prevHex = "0x" + (blockNumber - 10).toString(16);
    const [prev, genesis] = await Promise.allSettled([
      rpc<RpcBlock>("eth_getBlockByNumber", [prevHex, false]),
      rpc<RpcBlock>("eth_getBlockByNumber", ["0x0", false]),
    ]);
    const prevBlock = okOr(prev);
    if (prevBlock && latestTs != null) {
      const prevTs = fromHex(prevBlock.timestamp);
      blockTimeMs = Math.round(((latestTs - prevTs) * 1000) / 10);
    }
    const genesisBlock = okOr(genesis);
    if (genesisBlock && latestTs != null) {
      uptimeSec = latestTs - fromHex(genesisBlock.timestamp);
    }
  }

  // Block size + utilization + miner come straight from `latest`.
  const blockSizeBytes = latestBlock?.size ? fromHex(latestBlock.size) : null;
  const blockUtilization =
    latestBlock?.gasUsed && latestBlock?.gasLimit
      ? Math.round((fromHex(latestBlock.gasUsed) * 100) / fromHex(latestBlock.gasLimit))
      : null;
  const miner = latestBlock?.miner ? `${latestBlock.miner.slice(0, 8)}…` : null;

  return {
    chainId: chainId.status === "fulfilled" ? fromHex(chainId.value) : null,
    blockNumber,
    gasPriceGwei:
      gasPrice.status === "fulfilled" ? Math.round(fromHex(gasPrice.value) / 1e9) : null,
    blockTimeMs,
    txsInLatestBlock: latestBlock?.transactions?.length ?? null,
    latestBlockHash: latestBlock?.hash ? `${latestBlock.hash.slice(0, 10)}…` : null,
    uptimeSec,
    escrowBalanceEth:
      escrowBal.status === "fulfilled" ? Number(BigInt(escrowBal.value)) / 1e18 : null,
    pendingTxs:
      txpool.status === "fulfilled" ? fromHex(txpool.value.pending) : null,
    priorityFeeGwei:
      priorityFee.status === "fulfilled" ? Math.round(fromHex(priorityFee.value) / 1e9) : null,
    blockUtilization,
    blockSizeBytes,
    miner,
    queuedTxs:
      txpool.status === "fulfilled" ? fromHex(txpool.value.queued) : null,
    clientVersion:
      clientVersion.status === "fulfilled"
        ? clientVersion.value.split("/").slice(0, 2).join("/").slice(0, 24)
        : null,
  };
}
