# jkr-mesh smart contracts

`MeshEscrow.sol` — per-session payment channels for jkr-mesh agents.
Token-agnostic (`address(0)` = ETH, any ERC-20 otherwise). Designed for
**Base mainnet**.

## One-time setup

Foundry isn't a workspace dep — install it once per dev machine:

```sh
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

Then from the repo root:

```sh
make contracts-bootstrap   # fetches forge-std + openzeppelin into contracts/lib
make contracts-test        # runs the foundry test suite
```

## Local dev loop

In one terminal, fork Base mainnet locally:

```sh
make anvil-fork
```

Anvil prints 10 prefunded accounts. The real Base USDC contract at
`0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` is callable from any of them
exactly as it would be on real Base.

In another terminal, deploy MeshEscrow to the local fork:

```sh
make deploy-local
```

This uses anvil's first prefunded private key. The deploy command prints
the `MeshEscrow` address — wire that into the jkr CLI when slice 3.3 lands.

## Contract layout

```
src/
  MeshEscrow.sol       per-session payment channels
test/
  MeshEscrow.t.sol     forge tests (open/claim/close/replay-safety)
```

## Receipt format

Recipients claim funds against EIP-712 typed-data receipts signed by the
payer:

```
domain  = { name: "jkr-mesh", version: "1", chainId, verifyingContract }
types   = { Receipt: [ {name:"sessionId", type:"bytes32"},
                       {name:"cumulative", type:"uint256"} ] }
message = { sessionId, cumulative }
```

`cumulative` is the *cumulative* paid amount across the channel's
lifetime — each new receipt supersedes the prior. Recipient claims
`(cumulative - alreadyPaid)` per call. This matches the standard
unidirectional payment-channel pattern used by Spankchain, Connext, etc.

## Deployment to Base mainnet

Not yet. When ready: `forge create ... --rpc-url base --verify`.
