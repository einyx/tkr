# Always use rustup's Cargo (~/.cargo/bin). Homebrew's `cargo` ignores
# rust-toolchain.toml and stays on an older rustc — plain `cargo install` then fails MSRV.
CARGO   := $(HOME)/.cargo/bin/cargo
RUSTUP  := $(HOME)/.rustup/toolchains/1.88.0-aarch64-apple-darwin/bin
VERSION := $(shell grep -m1 '^version' crates/jkr/Cargo.toml | sed 's/.*= "\(.*\)"/\1/')
TAP     := /opt/homebrew/Library/Taps/einyx/homebrew-tap/Formula/jkr.rb

.PHONY: install build publish _bump-tap \
        contracts-bootstrap contracts-test contracts-build anvil-fork deploy-local \
        demo-payment \
        web-install web-dev web-build

# ---------- Smart contracts (foundry) ----------
# One-time bootstrap installs forge-std + openzeppelin into contracts/lib.
contracts-bootstrap:
	@command -v forge >/dev/null || (echo "error: foundry not installed — run: curl -L https://foundry.paradigm.xyz | bash && foundryup" >&2 && exit 1)
	cd contracts && forge install --no-commit foundry-rs/forge-std openzeppelin/openzeppelin-contracts

contracts-build:
	cd contracts && forge build

contracts-test:
	cd contracts && forge test -vv

# Boot a local anvil node forked from Base mainnet on :8545.
# Real USDC at 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 is callable from test accounts.
anvil-fork:
	@command -v anvil >/dev/null || (echo "error: anvil not installed — run: curl -L https://foundry.paradigm.xyz | bash && foundryup" >&2 && exit 1)
	anvil --fork-url https://mainnet.base.org --chain-id 8453

# Deploy MeshEscrow to the local anvil node. Requires anvil-fork running in another terminal.
deploy-local:
	cd contracts && forge create src/MeshEscrow.sol:MeshEscrow \
		--rpc-url http://127.0.0.1:8545 \
		--private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

# End-to-end mesh+payment demo on a self-contained local anvil node.
# No real money, no faucets. Runs in ~10s. Requires foundry + a release
# build of jkr (cargo build --release -p jkr).
demo-payment:
	@scripts/demo-payment.sh

# Deploy MeshEscrow to any EVM chain (Base, your own Conduit/Caldera
# rollup, anvil, ...). Set RPC_URL + PRIVATE_KEY_FILE in the env.
# See deploy/conduit.md for the Conduit-specific walkthrough.
deploy-mesh:
	@scripts/deploy-mesh.sh

# ---------- Web dashboard (React + Vite) ----------
# Lives at crates/jkr-server/web/. Build output is a single inlined HTML
# at crates/jkr-server/static/index.html, embedded into jkr-server via
# include_str!. Docker rebuilds it on every image build.
web-install:
	cd crates/jkr-server/web && npm install --no-audit --no-fund

web-dev:
	cd crates/jkr-server/web && npm run dev

web-build:
	cd crates/jkr-server/web && npm run build


install:
	@test -x "$(CARGO)" || (echo "error: rustup cargo not found at $(CARGO) — https://rustup.rs" >&2 && exit 1)
	"$(CARGO)" install --path crates/jkr --locked --force

build:
	@test -x "$(CARGO)" || (echo "error: rustup cargo not found at $(CARGO) — https://rustup.rs" >&2 && exit 1)
	"$(CARGO)" build --release -p jkr

# Build all 4 release targets, create a GitHub release, and bump the Homebrew tap.
# Usage: make publish   (uses version from crates/jkr/Cargo.toml)
publish:
	@echo "==> Publishing jkr v$(VERSION)"
	@command -v docker >/dev/null || (echo "error: docker required for linux cross-builds" >&2 && exit 1)
	@command -v gh     >/dev/null || (echo "error: gh (GitHub CLI) required" >&2 && exit 1)

	@echo "--> Building aarch64-apple-darwin"
	PATH="$(RUSTUP):$$PATH" $(CARGO) build --release -p jkr

	@echo "--> Building x86_64-apple-darwin"
	rustup target add x86_64-apple-darwin 2>/dev/null || true
	PATH="$(RUSTUP):$$PATH" $(CARGO) build --release -p jkr --target x86_64-apple-darwin

	@echo "--> Building x86_64-unknown-linux-gnu (Docker)"
	docker run --rm --platform linux/amd64 \
	  -v "$(CURDIR)":/usr/src/jkr \
	  -v /tmp/jkr-publish-x86-linux:/usr/src/jkr/target \
	  -w /usr/src/jkr rust:1.88 \
	  bash -c "apt-get update -qq && apt-get install -y libdbus-1-dev pkg-config 2>/dev/null && cargo build --release -p jkr"

	@echo "--> Building aarch64-unknown-linux-gnu (Docker)"
	docker run --rm --platform linux/arm64 \
	  -v "$(CURDIR)":/usr/src/jkr \
	  -v /tmp/jkr-publish-arm-linux:/usr/src/jkr/target \
	  -w /usr/src/jkr rust:1.88 \
	  bash -c "apt-get update -qq && apt-get install -y libdbus-1-dev pkg-config 2>/dev/null && cargo build --release -p jkr"

	@echo "--> Packaging"
	@rm -rf /tmp/jkr-release-$(VERSION) && mkdir -p /tmp/jkr-release-$(VERSION)
	tar czf /tmp/jkr-release-$(VERSION)/jkr-aarch64-apple-darwin.tar.gz      -C target/release jkr
	tar czf /tmp/jkr-release-$(VERSION)/jkr-x86_64-apple-darwin.tar.gz       -C target/x86_64-apple-darwin/release jkr
	tar czf /tmp/jkr-release-$(VERSION)/jkr-x86_64-unknown-linux-gnu.tar.gz  -C /tmp/jkr-publish-x86-linux/release jkr
	tar czf /tmp/jkr-release-$(VERSION)/jkr-aarch64-unknown-linux-gnu.tar.gz -C /tmp/jkr-publish-arm-linux/release jkr
	@cd /tmp/jkr-release-$(VERSION) && for f in *.tar.gz; do shasum -a 256 "$$f" | awk '{print $$1}' > "$$f.sha256"; done

	@echo "--> Creating GitHub release v$(VERSION)"
	gh release create v$(VERSION) /tmp/jkr-release-$(VERSION)/*.tar.gz /tmp/jkr-release-$(VERSION)/*.sha256 \
	  --repo einyx/jkr --title "v$(VERSION)" --generate-notes

	@echo "--> Bumping Homebrew tap"
	$(MAKE) _bump-tap VERSION=$(VERSION)

	@echo "==> Done. Run: brew upgrade jkr"

_bump-tap:
	$(eval SHA_ARM_MAC := $(shell cat /tmp/jkr-release-$(VERSION)/jkr-aarch64-apple-darwin.tar.gz.sha256))
	$(eval SHA_X86_MAC := $(shell cat /tmp/jkr-release-$(VERSION)/jkr-x86_64-apple-darwin.tar.gz.sha256))
	$(eval SHA_ARM_LNX := $(shell cat /tmp/jkr-release-$(VERSION)/jkr-aarch64-unknown-linux-gnu.tar.gz.sha256))
	$(eval SHA_X86_LNX := $(shell cat /tmp/jkr-release-$(VERSION)/jkr-x86_64-unknown-linux-gnu.tar.gz.sha256))
	python3 scripts/bump-tap.py "$(TAP)" "$(VERSION)" "$(SHA_ARM_MAC)" "$(SHA_X86_MAC)" "$(SHA_ARM_LNX)" "$(SHA_X86_LNX)"
	cd /opt/homebrew/Library/Taps/einyx/homebrew-tap && \
	  git add Formula/jkr.rb && \
	  git commit -m "release: jkr v$(VERSION)" && \
	  git push
