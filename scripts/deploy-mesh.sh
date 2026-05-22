#!/usr/bin/env bash
# Deploy MeshEscrow.sol to any EVM chain (Base, Arbitrum, OP, your own
# Conduit/Caldera/Alchemy rollup, anvil, …). Single command, single
# environment-variable surface.
#
# Required env:
#   RPC_URL         JSON-RPC endpoint (e.g. https://your-chain.conduit.xyz)
#   PRIVATE_KEY_FILE  Path to a 0600 file with `TKR_PAYMENT_KEY=0x...`
#                     OR a single bare hex line (with/without 0x prefix)
# Optional:
#   CHAIN_ID        Numeric chain id (--chain-id flag, recommended)
#   VERIFIER_URL    Block explorer API URL for forge --verify
#   ETHERSCAN_KEY   API key for the block explorer (forge --etherscan-api-key)
#
# Usage:
#   RPC_URL=https://your-chain.conduit.xyz \
#   PRIVATE_KEY_FILE=deploy/keys/mychain-deployer.env \
#   CHAIN_ID=12345 \
#   ./scripts/deploy-mesh.sh

set -euo pipefail
cd "$(dirname "$0")/.."

FORGE=${FORGE:-$(command -v forge || echo $HOME/.foundry/bin/forge)}
CAST=${CAST:-$(command -v cast || echo $HOME/.foundry/bin/cast)}

[ -x "$FORGE" ] || { echo "error: forge not found at $FORGE" >&2; exit 1; }
[ -x "$CAST" ]  || { echo "error: cast not found at $CAST" >&2; exit 1; }

: "${RPC_URL:?RPC_URL is required (e.g. https://your-chain.conduit.xyz)}"
: "${PRIVATE_KEY_FILE:?PRIVATE_KEY_FILE is required (path to 0600 key file)}"

if [ ! -f "$PRIVATE_KEY_FILE" ]; then
  echo "error: $PRIVATE_KEY_FILE not found" >&2
  exit 1
fi

# Pull the private key out of the file. Accepts either:
#   TKR_PAYMENT_KEY=0xabcd…
#   0xabcd…
#   abcd…
PRIVATE_KEY=$(grep -m1 -oE '0x[0-9a-fA-F]{64}|[0-9a-fA-F]{64}' "$PRIVATE_KEY_FILE" | head -1 || true)
case "$PRIVATE_KEY" in
  0x*) ;;
  *) PRIVATE_KEY="0x$PRIVATE_KEY" ;;
esac
if [ "${#PRIVATE_KEY}" != "66" ]; then
  echo "error: could not extract a 32-byte hex key from $PRIVATE_KEY_FILE" >&2
  exit 1
fi

DEPLOYER=$("$CAST" wallet address "$PRIVATE_KEY")
echo "deploying as: $DEPLOYER"
echo "rpc:          $RPC_URL"

# Pre-flight: balance check so the user gets a clean error instead of a
# cryptic forge/cast failure if the wallet is empty.
BAL=$("$CAST" balance "$DEPLOYER" --rpc-url "$RPC_URL" 2>/dev/null || echo 0)
echo "balance:      $BAL wei"
if [ "$BAL" = "0" ]; then
  echo "error: deployer wallet has zero balance on this chain" >&2
  echo "       fund the address from the chain's faucet, then re-run" >&2
  exit 1
fi

# Resolve chain id from RPC if not supplied.
if [ -z "${CHAIN_ID:-}" ]; then
  CHAIN_ID=$("$CAST" chain-id --rpc-url "$RPC_URL" 2>/dev/null || echo "")
fi
echo "chain id:     ${CHAIN_ID:-<unknown>}"

echo
echo "=== forge create MeshEscrow ==="
DEPLOY_ARGS=(
  --root contracts
  src/MeshEscrow.sol:MeshEscrow
  --rpc-url "$RPC_URL"
  --private-key "$PRIVATE_KEY"
  --broadcast
)
[ -n "${CHAIN_ID:-}" ] && DEPLOY_ARGS+=(--chain-id "$CHAIN_ID")
[ -n "${VERIFIER_URL:-}" ] && DEPLOY_ARGS+=(--verifier-url "$VERIFIER_URL" --verify)
[ -n "${ETHERSCAN_KEY:-}" ] && DEPLOY_ARGS+=(--etherscan-api-key "$ETHERSCAN_KEY")

DEPLOY_OUT=$("$FORGE" create "${DEPLOY_ARGS[@]}" 2>&1)
echo "$DEPLOY_OUT"
ESCROW=$(echo "$DEPLOY_OUT" | grep -oE 'Deployed to: 0x[0-9a-fA-F]+' | awk '{print $3}')

if [ -z "$ESCROW" ]; then
  echo
  echo "error: could not extract deployed address from forge output" >&2
  exit 1
fi

echo
echo "✓ MeshEscrow deployed"
echo "  address:  $ESCROW"
echo "  chain:    ${CHAIN_ID:-?}"
echo "  rpc:      $RPC_URL"
echo
echo "Use this address with tkr pay:"
echo "  tkr pay receipt-issue --contract $ESCROW --chain-id ${CHAIN_ID:-CHAIN_ID} ..."
echo "  tkr pay claim         --rpc-url $RPC_URL --receipt R.json --key-file $PRIVATE_KEY_FILE"
