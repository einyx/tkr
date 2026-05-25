# jkr-server web dashboard

React + TypeScript + Vite. Builds to a **single inlined HTML file** at
`crates/jkr-server/static/index.html` so the existing
`include_str!("../static/index.html")` in `main.rs` continues to work
unchanged.

## Local dev

```sh
cd crates/jkr-server/web
npm install   # one-time
npm run dev   # vite dev server on http://localhost:5173
              # /api/* is proxied to localhost:4000 (run jkr-server separately)
```

Run the backend in another terminal:

```sh
HOST=127.0.0.1 PORT=4000 cargo run -p jkr-server
```

Open `http://localhost:5173`.

## Production build

```sh
cd crates/jkr-server/web
npm run build
```

Outputs `crates/jkr-server/static/index.html` (single file, all JS + CSS
inlined via `vite-plugin-singlefile`). Then a normal Rust release build:

```sh
cargo build --release -p jkr-server
```

The Dockerfile does both stages automatically: `docker compose up --build`.

## Layout

```
web/
├─ index.html              # Vite entry (dev only)
├─ vite.config.ts
├─ tsconfig.json
├─ package.json
└─ src/
   ├─ main.tsx             # React entrypoint
   ├─ App.tsx              # 4-state router (auto/login/dashboard/session)
   ├─ api.ts               # fetch wrapper + domain types
   ├─ styles.css
   └─ views/
      ├─ Landing.tsx       # public unauth landing
      ├─ Login.tsx         # password sign-in
      ├─ Dashboard.tsx     # mesh panel + sessions panel
      └─ SessionDetail.tsx # per-session events table
```

## Why single-file output

`jkr-server` is a single Rust binary. Embedding the dashboard via
`include_str!` keeps that property — no static-asset coordination, no
runtime file lookup, the dashboard travels with the binary.

`vite-plugin-singlefile` inlines all JS chunks and CSS into the HTML at
build time. For a small app (< 100 kB gzipped) this is fine. If the
bundle grows past ~500 kB, switch to multi-file output and serve assets
from disk.
