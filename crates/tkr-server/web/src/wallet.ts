// Lean EIP-1193 wallet hook. No wagmi/viem — the page is a single-file
// inlined bundle and the JobsPanel actions only need connect / chain
// switch / sendTransaction. ~120 lines beats 250KB of dependencies.

import { useEffect, useState, useCallback } from "react";

export const TKR_CHAIN_ID = 31337420;
export const TKR_CHAIN_HEX = "0x1de2bcc";
const TKR_RPC_URL =
  typeof window !== "undefined" && window.location.protocol === "https:"
    ? `${window.location.origin}/api/v1/chain/rpc`
    : "http://127.0.0.1:4000/api/v1/chain/rpc";

interface Eip1193 {
  request: (args: { method: string; params?: unknown[] }) => Promise<unknown>;
  on?: (event: string, handler: (...args: unknown[]) => void) => void;
  removeListener?: (event: string, handler: (...args: unknown[]) => void) => void;
}

declare global {
  interface Window {
    ethereum?: Eip1193;
  }
}

export interface WalletState {
  available: boolean;          // window.ethereum exists
  account: string | null;      // connected EOA, lowercase
  chainId: number | null;      // current chain
  onTkrDevnet: boolean;        // chainId === TKR_CHAIN_ID
  connecting: boolean;
  error: string | null;
  connect: () => Promise<void>;
  switchToTkr: () => Promise<void>;
  sendTx: (to: string, data: string, value?: bigint) => Promise<string>;
  waitForReceipt: (txHash: string) => Promise<{ status: boolean; blockNumber: number }>;
}

export function useWallet(): WalletState {
  const eth = typeof window !== "undefined" ? window.ethereum ?? null : null;
  const [account, setAccount] = useState<string | null>(null);
  const [chainId, setChainId] = useState<number | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Read current state on mount (in case the user already authorized this origin).
  useEffect(() => {
    if (!eth) return;
    let cancelled = false;
    (async () => {
      try {
        const accs = (await eth.request({ method: "eth_accounts" })) as string[];
        if (!cancelled && accs[0]) setAccount(accs[0].toLowerCase());
        const cid = (await eth.request({ method: "eth_chainId" })) as string;
        if (!cancelled) setChainId(Number.parseInt(cid, 16));
      } catch {
        // Silent — wallet may be locked. User clicks Connect to retry.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [eth]);

  // Live updates from the wallet UI (account switch, network change).
  useEffect(() => {
    if (!eth?.on) return;
    const onAccs = (...args: unknown[]) => {
      const accs = args[0] as string[];
      setAccount(accs[0]?.toLowerCase() ?? null);
    };
    const onChain = (...args: unknown[]) => {
      const cid = args[0] as string;
      setChainId(Number.parseInt(cid, 16));
    };
    eth.on("accountsChanged", onAccs);
    eth.on("chainChanged", onChain);
    return () => {
      eth.removeListener?.("accountsChanged", onAccs);
      eth.removeListener?.("chainChanged", onChain);
    };
  }, [eth]);

  const connect = useCallback(async () => {
    if (!eth) {
      setError("no wallet detected — install MetaMask, Rabby, or another EIP-1193 wallet");
      return;
    }
    setConnecting(true);
    setError(null);
    try {
      const accs = (await eth.request({ method: "eth_requestAccounts" })) as string[];
      setAccount(accs[0]?.toLowerCase() ?? null);
      const cid = (await eth.request({ method: "eth_chainId" })) as string;
      setChainId(Number.parseInt(cid, 16));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setConnecting(false);
    }
  }, [eth]);

  const switchToTkr = useCallback(async () => {
    if (!eth) throw new Error("no wallet");
    try {
      await eth.request({
        method: "wallet_switchEthereumChain",
        params: [{ chainId: TKR_CHAIN_HEX }],
      });
    } catch (e: unknown) {
      // 4902 = chain not added. Suggest it.
      const code = (e as { code?: number })?.code;
      if (code === 4902) {
        await eth.request({
          method: "wallet_addEthereumChain",
          params: [
            {
              chainId: TKR_CHAIN_HEX,
              chainName: "tkr devnet",
              nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
              rpcUrls: [TKR_RPC_URL],
              blockExplorerUrls: [],
            },
          ],
        });
      } else {
        throw e;
      }
    }
  }, [eth]);

  const sendTx = useCallback(
    async (to: string, data: string, value?: bigint): Promise<string> => {
      if (!eth) throw new Error("no wallet");
      if (!account) throw new Error("not connected");
      const params: Record<string, string> = { from: account, to, data };
      if (value && value > 0n) params.value = "0x" + value.toString(16);
      const hash = (await eth.request({
        method: "eth_sendTransaction",
        params: [params],
      })) as string;
      return hash;
    },
    [eth, account],
  );

  // Poll for receipt — the wallet's provider goes to the wallet's RPC, not
  // our /api/v1/chain/rpc proxy, so this works even on networks we don't proxy.
  const waitForReceipt = useCallback(
    async (txHash: string): Promise<{ status: boolean; blockNumber: number }> => {
      if (!eth) throw new Error("no wallet");
      const deadline = Date.now() + 60_000;
      while (Date.now() < deadline) {
        const r = (await eth.request({
          method: "eth_getTransactionReceipt",
          params: [txHash],
        })) as { status: string; blockNumber: string } | null;
        if (r) {
          return {
            status: r.status === "0x1",
            blockNumber: Number.parseInt(r.blockNumber, 16),
          };
        }
        await new Promise((r) => setTimeout(r, 1500));
      }
      throw new Error("timed out waiting for receipt");
    },
    [eth],
  );

  return {
    available: !!eth,
    account,
    chainId,
    onTkrDevnet: chainId === TKR_CHAIN_ID,
    connecting,
    error,
    connect,
    switchToTkr,
    sendTx,
    waitForReceipt,
  };
}

// pad uint256/uint64/uint8 etc as 32-byte right-aligned hex (no 0x).
export function pad32(n: bigint): string {
  return n.toString(16).padStart(64, "0");
}
