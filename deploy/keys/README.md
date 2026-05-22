# Deploy keys

Throwaway deploy keys for tkr-mesh smart-contract releases.

**This whole directory (except this README) is gitignored.** Anything
sensitive that lands here stays local.

## Layout

```
deploy/keys/
├── README.md                         (tracked)
├── sepolia-deployer.env              (gitignored — Base Sepolia testnet)
└── mainnet-deployer.env              (gitignored — Base mainnet, when minted)
```

Each `*.env` file holds one `TKR_PAYMENT_KEY=0x...` line in the format
that `tkr pay`'s `--key-file` accepts. `forge create` and `cast send`
both accept `--private-key` from these files via:

```sh
PRIV=$(grep '^TKR_PAYMENT_KEY=' deploy/keys/sepolia-deployer.env | cut -d= -f2)
~/.foundry/bin/forge create --root contracts contracts/src/MeshEscrow.sol:MeshEscrow \
  --rpc-url https://sepolia.base.org \
  --private-key $PRIV \
  --broadcast
```

## Minting a new key

```sh
~/.foundry/bin/cast wallet new --json
```

Pick the address, fund it from a faucet (testnet) or bridge (mainnet),
then drop the JSON into a new `*-deployer.env` file with mode `0600`.

## Threat model

These keys are **throwaway**. They control deploy-only addresses, never
held funds long-term. If a key leaks:

1. The on-chain damage is limited to whatever testnet ETH it holds.
2. The deployed `MeshEscrow` is unchanged — it has no admin keys; the
   contract is fully autonomous after deploy.
3. Mint a new key, fund it, redeploy if needed.

For mainnet production deploys, prefer a hardware wallet. `cast send
--ledger` works directly with a connected Ledger.
