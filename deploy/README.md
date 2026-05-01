# tkr-server deploy

systemd unit + env template for running `tkr-server` behind nginx at
`tkr.prysm.sh`.

## One-time install

```sh
# 1. Build the release binary
cargo build -p tkr-server --release

# 2. Create the env directory and drop in your secrets
sudo install -d -m 0755 /etc/tkr
sudo install -m 0600 deploy/tkr-server.env.example /etc/tkr/tkr-server.env
sudoedit /etc/tkr/tkr-server.env       # set TKR_ADMIN_PASSWORD

# Quick way to mint a strong password into the file:
#   sudo sed -i "s|^TKR_ADMIN_PASSWORD=.*|TKR_ADMIN_PASSWORD=$(openssl rand -hex 32)|" /etc/tkr/tkr-server.env

# 3. Install the unit and enable it
sudo install -m 0644 deploy/tkr-server.service /etc/systemd/system/tkr-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now tkr-server
```

## Verify

```sh
systemctl status tkr-server
curl -fsS http://127.0.0.1:4000/health
journalctl -u tkr-server -f          # live tail
```

## After updating the binary

```sh
cargo build -p tkr-server --release
sudo systemctl restart tkr-server
```

## Rotating the password

```sh
sudoedit /etc/tkr/tkr-server.env
sudo systemctl restart tkr-server
```

Existing browser sessions become invalid (the in-memory session store is
wiped on restart); users sign in again with the new password.

## Hardening notes

The unit applies common systemd hardening: `NoNewPrivileges`,
`ProtectSystem=strict`, `ProtectHome=read-only`, `PrivateTmp`,
`MemoryDenyWriteExecute`, empty `CapabilityBoundingSet`. The service is
allowed to write only to `/home/alessio/tkr` (build/cargo state).

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
