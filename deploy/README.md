# jkr-server deploy

Two supported shapes for running `jkr-server` behind nginx at
`tkr.prysm.sh`. **Pick one — they manage the same port.** Compose is the
default.

- **Docker Compose** (recommended) — `docker-compose.yml` in the repo
  root, see *Compose install* below.
- **systemd** — `deploy/jkr-server.service`, see *systemd install*.

---

## Compose install (recommended)

```sh
# 1. One-time: prepare the env file with a strong password
install -m 0600 deploy/jkr-server.env.example jkr-server.env
sed -i "s|^JKR_ADMIN_PASSWORD=.*|JKR_ADMIN_PASSWORD=$(openssl rand -hex 32)|" jkr-server.env

# 2. Build + start
docker compose up -d --build

# 3. Verify
docker compose ps
curl -fsS http://127.0.0.1:4000/health
docker compose logs -f jkr-server
```

The container binds to `127.0.0.1:4000` on the host (loopback-only). nginx
upstreams to it exactly as before.

### After updating the code

```sh
docker compose up -d --build
```

### Rotate the password

```sh
sed -i "s|^JKR_ADMIN_PASSWORD=.*|JKR_ADMIN_PASSWORD=$(openssl rand -hex 32)|" jkr-server.env
docker compose up -d
```

### Container hardening applied

- Read-only rootfs + tmpfs `/tmp`.
- `cap_drop: ALL`, `no-new-privileges: true`.
- Non-root user (uid 1000).
- Resource caps: 1 CPU, 256 MB.
- Healthcheck hitting `/health` every 30s.

---

## systemd install (alternative)

```sh
# 1. Build the release binary
cargo build -p jkr-server --release

# 2. Create the env directory and drop in your secrets
sudo install -d -m 0755 /etc/jkr
sudo install -m 0600 deploy/jkr-server.env.example /etc/jkr/jkr-server.env
sudoedit /etc/jkr/jkr-server.env       # set JKR_ADMIN_PASSWORD

# Quick way to mint a strong password into the file:
#   sudo sed -i "s|^JKR_ADMIN_PASSWORD=.*|JKR_ADMIN_PASSWORD=$(openssl rand -hex 32)|" /etc/jkr/jkr-server.env

# 3. Install the unit and enable it
sudo install -m 0644 deploy/jkr-server.service /etc/systemd/system/jkr-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now jkr-server
```

## Verify

```sh
systemctl status jkr-server
curl -fsS http://127.0.0.1:4000/health
journalctl -u jkr-server -f          # live tail
```

## After updating the binary

```sh
cargo build -p jkr-server --release
sudo systemctl restart jkr-server
```

## Rotating the password

```sh
sudoedit /etc/jkr/jkr-server.env
sudo systemctl restart jkr-server
```

Existing browser sessions become invalid (the in-memory session store is
wiped on restart); users sign in again with the new password.

## Hardening notes

The unit applies common systemd hardening: `NoNewPrivileges`,
`ProtectSystem=strict`, `ProtectHome=read-only`, `PrivateTmp`,
`MemoryDenyWriteExecute`, empty `CapabilityBoundingSet`. The service is
allowed to write only to `/home/alessio/jkr` (build/cargo state).

If you move the repo or run as a different user, edit the `User=`,
`Group=`, `WorkingDirectory=`, `ExecStart=`, and `ReadWritePaths=` lines.

## nginx upstream snippet

For reference, the nginx server block that routes `tkr.prysm.sh` →
`127.0.0.1:4000` looks like:

```nginx
server {
    listen 443 ssl http2;
    server_name tkr.prysm.sh;

    # ssl_certificate / ssl_certificate_key managed by certbot or similar.

    location / {
        proxy_pass http://127.0.0.1:4000;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # WebSocket upgrade — required by /api/v1/stream.
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 3600s;
    }
}
```
