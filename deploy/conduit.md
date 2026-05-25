# Deploy MeshEscrow to a Conduit rollup

Conduit (https://conduit.xyz) hosts OP Stack rollups as a service. The
free tier is enough to spin up a testnet for jkr.

This walkthrough deploys `MeshEscrow.sol` to your own rollup so the
mesh + payment flow has a permanent contract address that's not Base.

## 1. Create the rollup

1. Sign in at https://conduit.xyz (GitHub OAuth).
2. **Create New Rollup** → pick **OP Stack** → choose **Testnet** (free
   tier; settles to Sepolia).
3. Give it a name (e.g. `jkr-mesh-testnet`). Set:
   - **Settlement layer**: Sepolia
   - **DA layer**: Ethereum calldata (cheapest)
   - **Native gas token**: ETH
4. Wait ~5 minutes for the chain to come up. You'll get:
   - **Chain ID** (e.g. `11155420`-style number — Conduit assigns one)
   - **RPC URL** (e.g. `https://rpc-jkr-mesh-testnet.t.conduit.xyz`)
   - **Block explorer URL**
   - **Faucet URL** (testnet ETH, free, tied to your Conduit account)

## 2. Mint a deploy key + fund it

```sh
# fresh deploy address (gitignored)
~/.foundry/bin/cast wallet new --json | tee deploy/keys/conduit-deployer.env
chmod 0600 deploy/keys/conduit-deployer.env
```

Edit `deploy/keys/conduit-deployer.env` so it contains a single line:

```
JKR_PAYMENT_KEY=0x<the private_key from cast wallet new>
```

Copy the **address** from the same `cast wallet new` output and request
testnet ETH from the Conduit faucet to that address (~30s).

## 3. Deploy MeshEscrow

```sh
RPC_URL=https://rpc-<your-chain>.t.conduit.xyz \
PRIVATE_KEY_FILE=deploy/keys/conduit-deployer.env \
make deploy-mesh
```

Output ends with:

```
✓ MeshEscrow deployed
  address:  0x...
  chain:    <id>
  rpc:      https://rpc-<your-chain>.t.conduit.xyz
```

Save that contract address — you'll pass it as `--contract` to
`jkr pay receipt-issue`.

## 4. Open a channel + claim a receipt

```sh
ESCROW=0x...                   # from step 3
RPC=https://rpc-<your-chain>.t.conduit.xyz
PAYER_KEY=$(grep -oE '0x[0-9a-fA-F]{64}' deploy/keys/conduit-deployer.env)

# Mint a recipient key alongside the deployer
~/.foundry/bin/cast wallet new --json > /tmp/recipient.env
RECIP_KEY=$(grep -oE '0x[0-9a-fA-F]{64}' /tmp/recipient.env)
RECIP_ADDR=$(~/.foundry/bin/cast wallet address $RECIP_KEY)

# Send a tiny amount of testnet ETH to the recipient so they can pay gas
# on the claim tx. Same Conduit faucet, same flow, ~30s.

# Open: 0.01 ETH channel, 1h deadline
SID=0xaa00000000000000000000000000000000000000000000000000000000000001
~/.foundry/bin/cast send $ESCROW \
  "open(bytes32,address,address,uint256,uint64)" \
  $SID $RECIP_ADDR 0x0 10000000000000000 $(($(date +%s)+3600)) \
  --value 10000000000000000 \
  --rpc-url $RPC --private-key $PAYER_KEY

# Issue a receipt for 0.005 ETH cumulative
target/release/jkr pay receipt-issue \
  --session-id $SID \
  --cumulative 5000000000000000 \
  --chain-id <CHAIN_ID> \
  --contract $ESCROW \
  --key-file deploy/keys/conduit-deployer.env > /tmp/receipt.json

# Claim
echo "JKR_PAYMENT_KEY=$RECIP_KEY" > /tmp/recipkey.env
chmod 0600 /tmp/recipkey.env
target/release/jkr pay claim \
  --receipt /tmp/receipt.json \
  --rpc-url $RPC \
  --key-file /tmp/recipkey.env
```

You'll see `✓ claim confirmed` with a block number from your own chain.

## 5. Tear down

Conduit testnets are free but use storage. If you're done:

1. Conduit dashboard → your rollup → **Settings** → **Delete rollup**.
2. Local: `git rm deploy/keys/conduit-deployer.env` (already gitignored,
   but remove from disk too).

## When to graduate to mainnet

Conduit's mainnet tier is paid (~$500/mo at the time of writing). The
case for paying is real-money throughput on a chain you own — useful if
the per-message volume on Base ever becomes a real cost.

Until then: **stay on Base mainnet for production**, use Conduit testnet
for experiments, and let the Base USDC liquidity do the heavy lifting.
