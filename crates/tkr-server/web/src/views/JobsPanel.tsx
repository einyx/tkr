import { useQuery } from "@tanstack/react-query";

// Hardcoded for the live tkr devnet. If you redeploy to a different chain,
// override via window.__TKR_JOB_BOARD or change this constant.
const JOB_BOARD =
  (typeof window !== "undefined" && (window as any).__TKR_JOB_BOARD) ||
  "0x9A676e781A523b5d0C0e43731313A708CB607508";

// Function selectors (4-byte keccak prefixes), precomputed:
//   jobCount()           → 0x4c5d8a0f
//   getJob(uint256)      → 0xbf22c457
const SEL_JOB_COUNT = "0x4c5d8a0f";
const SEL_GET_JOB = "0xbf22c457";

const STATUS_NAMES = ["Open", "Taken", "Completed", "Accepted", "Cancelled", "TimedOut"] as const;
type StatusName = (typeof STATUS_NAMES)[number];

interface Job {
  id: number;
  poster: string;
  worker: string;
  reward: bigint;
  token: string;
  deadline: number;
  status: StatusName;
  preview: string;
}

async function ethCall(to: string, data: string): Promise<string> {
  const res = await fetch("/api/v1/chain/rpc", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "eth_call",
      params: [{ to, data }, "latest"],
    }),
  });
  const j = await res.json();
  if (j.error) throw new Error(`eth_call: ${j.error.message ?? "error"}`);
  return j.result as string;
}

function pad32Hex(n: bigint): string {
  return n.toString(16).padStart(64, "0");
}

function decodeAddress(slot: string): string {
  // 32-byte slot, last 20 bytes are the address.
  const lower = "0x" + slot.slice(24);
  // Return as-is (lowercase) — checksum cosmetics aren't worth a keccak pass here.
  return lower;
}

function decodeUint(slot: string): bigint {
  return BigInt("0x" + slot);
}

function decodeJob(id: number, raw: string): Job {
  // raw is 0x-prefixed ABI-encoded tuple. Tuple shape:
  //   address poster, address worker, uint256 reward, address token,
  //   bytes32 specHash, bytes32 resultHash, uint64 deadline, uint8 status,
  //   string specPreview (dynamic — 32-byte offset, then length+data)
  const hex = raw.startsWith("0x") ? raw.slice(2) : raw;
  const slot = (i: number) => hex.slice(i * 64, (i + 1) * 64);

  const poster = decodeAddress(slot(0));
  const worker = decodeAddress(slot(1));
  const reward = decodeUint(slot(2));
  const token = decodeAddress(slot(3));
  // slot 4 = specHash (skip), slot 5 = resultHash (skip)
  const deadline = Number(decodeUint(slot(6)));
  const statusByte = Number(decodeUint(slot(7)));
  const status = STATUS_NAMES[statusByte] ?? ("?" as StatusName);

  const previewOffset = Number(decodeUint(slot(8))); // bytes from start of return data
  const previewStartChars = previewOffset * 2;
  const previewLen = Number(decodeUint(hex.slice(previewStartChars, previewStartChars + 64) as string));
  const previewBytesHex = hex.slice(previewStartChars + 64, previewStartChars + 64 + previewLen * 2);
  // utf-8 decode
  const previewBytes = new Uint8Array(previewLen);
  for (let i = 0; i < previewLen; i++) {
    previewBytes[i] = parseInt(previewBytesHex.slice(i * 2, i * 2 + 2), 16);
  }
  const preview = new TextDecoder().decode(previewBytes);

  return { id, poster, worker, reward, token, deadline, status, preview };
}

async function fetchJobs(): Promise<Job[]> {
  const countHex = await ethCall(JOB_BOARD, SEL_JOB_COUNT);
  const count = Number(BigInt(countHex));
  if (count === 0) return [];
  // Cap to last 8 jobs for screen sanity; reverse-iterate so newest first.
  const start = Math.max(1, count - 7);
  const ids: number[] = [];
  for (let i = count; i >= start; i--) ids.push(i);

  const jobs = await Promise.all(
    ids.map(async (id) => {
      const data = SEL_GET_JOB + pad32Hex(BigInt(id));
      try {
        const raw = await ethCall(JOB_BOARD, data);
        return decodeJob(id, raw);
      } catch {
        return null;
      }
    }),
  );
  return jobs.filter((j): j is Job => j !== null);
}

function fmtEth(wei: bigint): string {
  if (wei === 0n) return "0";
  const eth = Number(wei) / 1e18;
  if (eth < 0.001) return `${(Number(wei) / 1e15).toFixed(0)} mwei`;
  if (eth < 1) return `${eth.toFixed(3)} ETH`;
  return `${eth.toFixed(2)} ETH`;
}

function fmtAddr(addr: string): string {
  if (addr === "0x0000000000000000000000000000000000000000") return "—";
  return `${addr.slice(0, 6)}…${addr.slice(-4)}`;
}

function fmtDeadline(ts: number): string {
  const sec = ts - Math.floor(Date.now() / 1000);
  if (sec <= 0) return "expired";
  if (sec < 60) return `${sec}s`;
  if (sec < 3600) return `${Math.floor(sec / 60)}m`;
  if (sec < 86400) return `${Math.floor(sec / 3600)}h`;
  return `${Math.floor(sec / 86400)}d`;
}

export function JobsPanel() {
  const { data: jobs, error } = useQuery<Job[]>({
    queryKey: ["jobs-list"],
    queryFn: fetchJobs,
    refetchInterval: 6_000,
  });

  return (
    <section>
      <div className="lp-section-title" style={{ marginTop: 18 }}>
        agent jobs · live on the devnet
      </div>
      {error ? (
        <div className="err" style={{ padding: "12px" }}>
          {String(error)}
        </div>
      ) : !jobs ? (
        <div className="empty">loading…</div>
      ) : jobs.length === 0 ? (
        <div className="empty">no jobs posted yet</div>
      ) : (
        <div className="lp-jobs">
          <div className="lp-jobs-row lp-jobs-head">
            <span>id</span>
            <span>status</span>
            <span>reward</span>
            <span>poster</span>
            <span>worker</span>
            <span>deadline</span>
            <span>preview</span>
          </div>
          {jobs.map((j) => (
            <div key={j.id} className="lp-jobs-row">
              <span>#{j.id}</span>
              <span className={`lp-job-status lp-job-${j.status.toLowerCase()}`}>{j.status}</span>
              <span>{fmtEth(j.reward)}</span>
              <span className="muted">{fmtAddr(j.poster)}</span>
              <span className="muted">{fmtAddr(j.worker)}</span>
              <span className="muted">{fmtDeadline(j.deadline)}</span>
              <span className="lp-job-preview">{j.preview}</span>
            </div>
          ))}
          <div className="lp-broker-line" style={{ marginTop: 10 }}>
            board: <code>{JOB_BOARD}</code>
          </div>
        </div>
      )}
    </section>
  );
}
