#!/usr/bin/env bash
# scripts/seed-devnet.sh — deploy MeshEscrow + JobBoard to the local anvil
# (or wipe-and-redeploy after a chain restart) so the dashboard, MCP
# jkr_jobs_list tool, and JobsPanel UI have something to render.
#
# Idempotent: if both contracts already exist at their expected addresses,
# the script exits cleanly without sending any tx. If only one is missing,
# only that one is redeployed (this isn't supported because addresses are
# nonce-derived; redeploys are all-or-nothing).
#
# Targets local anvil only. Uses anvil's account[0] — public dev key,
# safe to ship in source. DO NOT POINT THIS AT A PUBLIC CHAIN.
#
# Addresses are deterministic — derived from CREATE(deployer, nonce):
#   MeshEscrow = 0x5FbD…0aa3  (account[0] nonce 0)
#   JobBoard   = 0xe7f1…0512  (account[0] nonce 1)
# The dashboard, CLI, and MCP server hard-code these values. If you re-seed
# with a different deployer or out of order, update the constants in:
#   crates/jkr/src/cli.rs                     DEFAULT_JOB_BOARD
#   crates/jkr-server/web/src/views/JobsPanel.tsx                 JOB_BOARD
#   crates/jkr-server/web/src/views/Landing.tsx                  MESH_ESCROW_ADDR

set -euo pipefail
cd "$(dirname "$0")/.."

RPC_URL=${RPC_URL:-http://127.0.0.1:8545}
# Anvil deterministic account[0]. Public knowledge, devnet-only.
DEPLOYER_KEY=${DEPLOYER_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}
DEPLOYER_ADDR=0xf39Fd6e51aad88F6F4ce6aB8827279cfFFb92266

ESCROW_ADDR=0x5FbDB2315678afecb367f032d93F642f64180aa3
JOBBOARD_ADDR=0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512

FORGE=${FORGE:-$(command -v forge || echo "$HOME/.foundry/bin/forge")}
CAST=${CAST:-$(command -v cast || echo "$HOME/.foundry/bin/cast")}
[ -x "$FORGE" ] || { echo "error: forge not found at $FORGE — install foundry" >&2; exit 1; }
[ -x "$CAST" ]  || { echo "error: cast not found at $CAST" >&2; exit 1; }

# Refuse to seed a non-anvil chain. jkr devnet chain id is 31337420; vanilla
# anvil is 31337. Anything else is probably a real network — bail.
CHAIN_ID=$("$CAST" chain-id --rpc-url "$RPC_URL" 2>/dev/null || echo 0)
if [ "$CHAIN_ID" != "31337420" ] && [ "$CHAIN_ID" != "31337" ]; then
  echo "error: chain id $CHAIN_ID isn't a recognized devnet (expected 31337 or 31337420)" >&2
  echo "       refusing to seed — this script is anvil-only" >&2
  exit 1
fi

has_code() {
  local addr=$1
  local code
  code=$("$CAST" code "$addr" --rpc-url "$RPC_URL" 2>/dev/null || echo 0x)
  [ "$code" != "0x" ] && [ -n "$code" ]
}

if has_code "$ESCROW_ADDR" && has_code "$JOBBOARD_ADDR"; then
  # Contracts present — only open the demo channel if it isn't already
  # there, so the dashboard stat stays non-zero across re-runs.
  SESSION_ID="0x$(printf 'demo-channel-1' | "$CAST" keccak | sed 's/^0x//')"
  # Channel exists iff its payer (first tuple field) is non-zero. The token
  # field is also a zero-address for ETH channels, so a substring match on
  # 0x0000…0000 isn't enough — extract the first address explicitly.
  PAYER=$("$CAST" call --rpc-url "$RPC_URL" "$ESCROW_ADDR" \
    "getChannel(bytes32)((address,address,address,uint256,uint256,uint64))" \
    "$SESSION_ID" 2>/dev/null | grep -oE '0x[0-9a-fA-F]{40}' | head -1 || echo "")
  if [ "$PAYER" = "0x0000000000000000000000000000000000000000" ] || [ -z "$PAYER" ]; then
    echo "→ contracts present, opening demo channel…"
    EXPIRES=$(( $(date +%s) + 7*86400 ))
    "$CAST" send --rpc-url "$RPC_URL" --private-key "$DEPLOYER_KEY" \
      "$ESCROW_ADDR" "open(bytes32,address,address,uint256,uint64)" \
      "$SESSION_ID" "0x70997970C51812dc3A010C7d01b50e0d17dc79C8" \
      "0x0000000000000000000000000000000000000000" "2500000000000000000" "$EXPIRES" \
      --value 2500000000000000000 >/dev/null
    echo "  ✓ 2.5 ETH locked"
  fi
  echo "✓ devnet already seeded"
  echo "  MeshEscrow: $ESCROW_ADDR"
  echo "  JobBoard:   $JOBBOARD_ADDR"
  exit 0
fi

echo "→ seeding jkr devnet (chain id $CHAIN_ID)"
echo "  rpc:      $RPC_URL"
echo "  deployer: $DEPLOYER_ADDR"

# Anvil resets nonces on a fresh chain, but if the user has been doing other
# things, our account may have a non-zero nonce. We don't try to recover —
# the address constants only line up from a clean genesis. Bail loudly so the
# operator can wipe state and re-run.
NONCE=$("$CAST" nonce "$DEPLOYER_ADDR" --rpc-url "$RPC_URL")
if [ "$NONCE" != "0" ]; then
  echo "error: deployer nonce is $NONCE (expected 0 from clean genesis)" >&2
  echo "       wipe the chain (docker compose down -v jkr-chain && docker compose up -d) and re-run" >&2
  exit 1
fi

echo
echo "=== nonce 0: deploy MeshEscrow ==="
"$FORGE" create --root contracts src/MeshEscrow.sol:MeshEscrow \
  --rpc-url "$RPC_URL" --private-key "$DEPLOYER_KEY" --broadcast \
  --chain-id "$CHAIN_ID" >/dev/null
has_code "$ESCROW_ADDR" || { echo "error: MeshEscrow not at expected address" >&2; exit 1; }
echo "✓ MeshEscrow @ $ESCROW_ADDR"

echo
echo "=== nonce 1: deploy JobBoard ==="
"$FORGE" create --root contracts src/JobBoard.sol:JobBoard \
  --rpc-url "$RPC_URL" --private-key "$DEPLOYER_KEY" --broadcast \
  --chain-id "$CHAIN_ID" >/dev/null
has_code "$JOBBOARD_ADDR" || { echo "error: JobBoard not at expected address" >&2; exit 1; }
echo "✓ JobBoard   @ $JOBBOARD_ADDR"

# Three sample jobs so the JobsPanel + jkr_jobs_list have data. Posted in
# ETH (token = 0x0). Reward funds come from the deployer's prefunded balance.
#
# postJob(bytes32 specHash, string specPreview, uint256 reward, address token, uint64 deadline)
#   selector: 0xc7c2d2e0  (computed via cast sig)
echo
echo "=== posting 3 sample jobs ==="
DEADLINE=$(( $(date +%s) + 86400 ))   # 24h from now
post_job() {
  local preview=$1
  local reward_wei=$2
  local spec_hash="0x$(printf '%s' "$preview" | "$CAST" keccak | sed 's/^0x//')"
  "$CAST" send --rpc-url "$RPC_URL" --private-key "$DEPLOYER_KEY" \
    "$JOBBOARD_ADDR" \
    "postJob(bytes32,string,uint256,address,uint64)" \
    "$spec_hash" "$preview" "$reward_wei" \
    "0x0000000000000000000000000000000000000000" "$DEADLINE" \
    --value "$reward_wei" >/dev/null
  echo "  ✓ $preview ($reward_wei wei)"
}
post_job "outline crates/jkr-mesh/src/client.rs as a markdown summary"        100000000000000000   #  0.10 ETH
post_job "find all callers of broker::BrokerState::route in jkr-server"      150000000000000000   #  0.15 ETH
post_job "add a /api/v1/mesh/peers endpoint listing connected addrs"         500000000000000000   #  0.50 ETH

echo
echo "=== opening sample 2.5 ETH escrow channel ==="
# So the dashboard's "ETH locked in MeshEscrow" stat is non-zero. Channel
# pays anvil[1] (deterministic, no risk to anyone). 7-day expiry.
SESSION_ID="0x$(printf 'demo-channel-1' | "$CAST" keccak | sed 's/^0x//')"
RECIPIENT=0x70997970C51812dc3A010C7d01b50e0d17dc79C8       # anvil account[1]
EXPIRES=$(( $(date +%s) + 7*86400 ))
"$CAST" send --rpc-url "$RPC_URL" --private-key "$DEPLOYER_KEY" \
  "$ESCROW_ADDR" \
  "open(bytes32,address,address,uint256,uint64)" \
  "$SESSION_ID" "$RECIPIENT" "0x0000000000000000000000000000000000000000" \
  "2500000000000000000" "$EXPIRES" \
  --value 2500000000000000000 >/dev/null
echo "  ✓ channel opened (2.5 ETH locked)"

echo
echo "✓ devnet seeded"
echo "  MeshEscrow: $ESCROW_ADDR  (2.5 ETH locked)"
echo "  JobBoard:   $JOBBOARD_ADDR  (3 jobs posted)"
echo
echo "verify:"
echo "  curl -sS -X POST $RPC_URL -H 'content-type: application/json' \\"
echo "    -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_call\",\"params\":[{\"to\":\"$JOBBOARD_ADDR\",\"data\":\"0x4c5d8a0f\"},\"latest\"]}'"
