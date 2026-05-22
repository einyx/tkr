# Connecting Cursor to `mcp.meshx.app` (Cloudflare Access)

## Root cause (confirmed)

`mcp.meshx.app` is behind **Cloudflare Access** with MCP OAuth. The authorize step **requires** an RFC 8707 `resource` query parameter (e.g. `resource=https://mcp.meshx.app/mcp`).

Cursor’s built-in MCP OAuth client does **not** send `resource` today. Cloudflare responds by redirecting to:

```text
http://localhost:8787/callback?error=invalid_target&error_description=No+resource+parameter+found
```

The browser then tries `localhost:8787` with an error payload — Cursor reports this as **“Could not connect to server”**. Other MCPs (Stripe, Figma, Linear) use their own IdPs and do not hit this Cloudflare rule, which is why “Cursor/Claude works” for everything except MeshX.

This is **not** the same issue as `tkr.prysm.sh` / Logto session persistence.

## Workaround: Bearer token via `cloudflared` (recommended)

Run on the machine where **your browser** can complete Cloudflare login (usually your Mac, not only the SSH host):

```bash
cloudflared access login https://mcp.meshx.app
cloudflared access token -app=https://mcp.meshx.app
```

Add to `~/.cursor/mcp.json` (or project `.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "meshx-cloud": {
      "url": "https://mcp.meshx.app/mcp",
      "headers": {
        "Authorization": "Bearer <paste-token-here>"
      }
    }
  }
}
```

Use the **`/mcp`** path (not the site root). Tokens expire; refresh with `cloudflared access token` when you get 401s.

For **Remote SSH**: put `mcp.json` on the remote host (`~/.cursor/mcp.json` under the SSH user) and obtain the token on a machine where you can finish the Cloudflare browser login, then paste it into that file.

## Verify the server

```bash
# Should return login redirect when resource is set (good)
curl -sI -o /dev/null -w '%{redirect_url}\n' \
  'https://meshxdata.cloudflareaccess.com/cdn-cgi/access/oauth/authorization?response_type=code&client_id=test&redirect_uri=http%3A%2F%2Flocalhost%3A8787%2Fcallback&state=x&code_challenge=x&code_challenge_method=S256&resource=https%3A%2F%2Fmcp.meshx.app%2Fmcp'

# Without resource → invalid_target on localhost callback (Cursor’s failure mode)
curl -sI -o /dev/null -w '%{redirect_url}\n' \
  'https://meshxdata.cloudflareaccess.com/cdn-cgi/access/oauth/authorization?response_type=code&client_id=test&redirect_uri=http%3A%2F%2Flocalhost%3A8787%2Fcallback&state=x&code_challenge=x&code_challenge_method=S256'
```

## Long-term fixes

1. **Cursor**: send `resource` on authorize/token requests for MCP OAuth (Cloudflare Access MCP requirement).
2. **MeshX / Cloudflare**: document the `cloudflared` token path for IDE users until Cursor supports `resource`.
3. **Optional**: Access service token / static OAuth client for team members.
