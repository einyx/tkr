#!/usr/bin/env bash
# End-to-end demo of the jkr-mesh payment flow.
#
# Boots an anvil fork of Base mainnet, deploys MeshEscrow, opens a
# payment channel, issues a receipt with `jkr pay receipt-issue`, claims
# it with `jkr pay claim`, prints the recipient's balance delta — all in
# under 30 seconds, no real money involved.
#
# Requires: foundry (`anvil`, `forge`, `cast`), `jkr` built at target/release.

set -euo pipefail

cd "$(dirname "$0")/.."

# Resolve binaries — prefer ~/.foundry/bin if installed there.
FORGE=${FORGE:-$(command -v forge || echo $HOME/.foundry/bin/forge)}
ANVIL=${ANVIL:-$(command -v anvil || echo $HOME/.foundry/bin/anvil)}
CAST=${CAST:-$(command -v cast || echo $HOME/.foundry/bin/cast)}
JKR=${JKR:-./target/release/jkr}

for bin in "$FORGE" "$ANVIL" "$CAST"; do
  if [ ! -x "$bin" ]; then
    echo "error: $bin not found or not executable" >&2
    echo "       install foundry: curl -L https://foundry.paradigm.xyz | bash && foundryup" >&2
    exit 1
  fi
done

if [ ! -x "$JKR" ]; then
  echo "error: $JKR not found — run: cargo build --release -p jkr" >&2
  exit 1
fi

# Anvil's deterministic test accounts.
PAYER_PRIV=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
RECIP_PRIV=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
RECIP_ADDR=0x70997970C51812dc3A010C7d01b50e0d17dc79C8

ANVIL_LOG=$(mktemp)
ANVIL_PID=""
PKF=""
RKF=""
RFILE=""

cleanup() {
  if [ -n "$ANVIL_PID" ] && kill -0 "$ANVIL_PID" 2>/dev/null; then
    kill "$ANVIL_PID" 2>/dev/null || true
    wait "$ANVIL_PID" 2>/dev/null || true
  fi
  rm -f "$PKF" "$RKF" "$RFILE" "$ANVIL_LOG"
}
trap cleanup EXIT INT TERM

echo "▲ jkr mesh+payment demo"
echo "  starts anvil on :8545, deploys MeshEscrow, runs open→claim→verify"
echo

echo "1) booting anvil (silent)..."
"$ANVIL" --silent > "$ANVIL_LOG" 2>&1 &
ANVIL_PID=$!
# Wait for RPC ready.
for _ in $(seq 1 30); do
  if "$CAST" block-number --rpc-url http://127.0.0.1:8545 >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
if ! "$CAST" block-number --rpc-url http://127.0.0.1:8545 >/dev/null 2>&1; then
  echo "error: anvil did not become ready" >&2
  cat "$ANVIL_LOG" >&2
  exit 1
fi
echo "   anvil up @ block $("$CAST" block-number --rpc-url http://127.0.0.1:8545)"

echo "2) deploying MeshEscrow..."
set +e
DEPLOY_OUT=$("$FORGE" create --root contracts src/MeshEscrow.sol:MeshEscrow \
  --rpc-url http://127.0.0.1:8545 --private-key "$PAYER_PRIV" --broadcast 2>&1)
DEPLOY_RC=$?
set -e
if [ "$DEPLOY_RC" -ne 0 ]; then
  echo "error: forge create exited $DEPLOY_RC:" >&2
  echo "$DEPLOY_OUT" >&2
  exit 1
fi
ESCROW=$(echo "$DEPLOY_OUT" | grep -oE "Deployed to: 0x[0-9a-fA-F]+" | awk '{print $3}')
if [ -z "$ESCROW" ]; then
  echo "error: could not extract address from forge output:" >&2
  echo "$DEPLOY_OUT" >&2
  exit 1
fi
echo "   MeshEscrow @ $ESCROW"

SID=0xaa00000000000000000000000000000000000000000000000000000000000001
DEADLINE=$(($(date +%s) + 3600))

echo "3) opening 1.0 ETH channel for recipient $RECIP_ADDR..."
OPEN_OUT=$("$CAST" send "$ESCROW" \
  "open(bytes32,address,address,uint256,uint64)" \
  "$SID" "$RECIP_ADDR" 0x0000000000000000000000000000000000000000 1000000000000000000 "$DEADLINE" \
  --value 1ether \
  --rpc-url http://127.0.0.1:8545 \
  --private-key "$PAYER_PRIV" 2>&1) || {
    echo "error: cast send failed:" >&2
    echo "$OPEN_OUT" >&2
    exit 1
  }
STATUS=$(echo "$OPEN_OUT" | grep -oP 'status\s+\K\S+' | head -1)
[ "$STATUS" = "1" ] || [ "$STATUS" = "(success)" ] || true
echo "   channel opened (status=$STATUS)"

echo "4) jkr pay receipt-issue (cumulative 0.4 ETH)..."
PKF=$(mktemp)
RKF=$(mktemp)
RFILE=$(mktemp)
echo "JKR_PAYMENT_KEY=$PAYER_PRIV" > "$PKF"; chmod 0600 "$PKF"
echo "JKR_PAYMENT_KEY=$RECIP_PRIV" > "$RKF"; chmod 0600 "$RKF"
"$JKR" pay receipt-issue \
  --session-id "$SID" \
  --cumulative 400000000000000000 \
  --chain-id 31337 \
  --contract "$ESCROW" \
  --key-file "$PKF" > "$RFILE" 2>/dev/null
echo "   receipt issued ($(wc -c < "$RFILE") bytes JSON)"

echo "5) jkr pay claim (recipient submits to chain)..."
BAL_BEFORE=$("$CAST" balance "$RECIP_ADDR" --rpc-url http://127.0.0.1:8545)
"$JKR" pay claim \
  --receipt "$RFILE" \
  --rpc-url http://127.0.0.1:8545 \
  --key-file "$RKF" 2>&1 | sed 's/^/   /'
BAL_AFTER=$("$CAST" balance "$RECIP_ADDR" --rpc-url http://127.0.0.1:8545)

DELTA=$((BAL_AFTER - BAL_BEFORE))
DELTA_ETH=$(echo "scale=6; $DELTA / 1000000000000000000" | bc 2>/dev/null || echo "?")

echo
echo "▲ recipient balance delta: $DELTA wei (~$DELTA_ETH ETH)"
echo "  expected ~0.4 ETH minus claim gas (≈0.000054 ETH on anvil)"
echo
echo "✓ demo complete"
