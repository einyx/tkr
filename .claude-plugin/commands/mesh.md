---
description: Show live mesh broker status from tkr.prysm.sh — peers connected, last activity, escrow balance.
---

Fetch live mesh stats from the public broker and show them in a compact
form. There are two endpoints worth pulling:

1. `curl -sS https://tkr.prysm.sh/api/v1/mesh/status` — peer count,
   last-message timestamp, broker uptime.
2. `curl -sS -X POST https://tkr.prysm.sh/api/v1/chain/rpc \
       -H 'content-type: application/json' \
       -d '{"jsonrpc":"2.0","method":"eth_getBalance",
            "params":["0x5FbDB2315678afecb367f032d93F642f64180aa3","latest"],
            "id":1}'` — ETH currently locked in MeshEscrow.

Both endpoints are public (no auth). Render in 4-6 lines max:

```
mesh peers:    N online   (last msg: M minutes ago)
broker uptime: H hours
escrow:        X.YZ ETH locked at 0x5FbD…0aa3
```

If `$ARGUMENTS` is a slug name, also fetch
`/api/v1/mesh/{slug}/members` and append the member list.

Don't link to the dashboard URL — the user already knows it.
