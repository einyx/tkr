# Always use rustup's Cargo (~/.cargo/bin). Homebrew's `cargo` ignores
# rust-toolchain.toml and stays on an older rustc — plain `cargo install` then fails MSRV.
CARGO   := $(HOME)/.cargo/bin/cargo
RUSTUP  := $(HOME)/.rustup/toolchains/1.88.0-aarch64-apple-darwin/bin
VERSION := $(shell grep -m1 '^version' crates/tkr/Cargo.toml | sed 's/.*= "\(.*\)"/\1/')
TAP     := /opt/homebrew/Library/Taps/einyx/homebrew-tap/Formula/tkr.rb

.PHONY: install build publish _bump-tap

install:
	@test -x "$(CARGO)" || (echo "error: rustup cargo not found at $(CARGO) — https://rustup.rs" >&2 && exit 1)
	"$(CARGO)" install --path crates/tkr --locked --force

build:
	@test -x "$(CARGO)" || (echo "error: rustup cargo not found at $(CARGO) — https://rustup.rs" >&2 && exit 1)
	"$(CARGO)" build --release -p tkr

# Build all 4 release targets, create a GitHub release, and bump the Homebrew tap.
# Usage: make publish   (uses version from crates/tkr/Cargo.toml)
publish:
	@echo "==> Publishing tkr v$(VERSION)"
	@command -v docker >/dev/null || (echo "error: docker required for linux cross-builds" >&2 && exit 1)
	@command -v gh     >/dev/null || (echo "error: gh (GitHub CLI) required" >&2 && exit 1)

	@echo "--> Building aarch64-apple-darwin"
	PATH="$(RUSTUP):$$PATH" $(CARGO) build --release -p tkr

	@echo "--> Building x86_64-apple-darwin"
	rustup target add x86_64-apple-darwin 2>/dev/null || true
	PATH="$(RUSTUP):$$PATH" $(CARGO) build --release -p tkr --target x86_64-apple-darwin

	@echo "--> Building x86_64-unknown-linux-gnu (Docker)"
	docker run --rm \
	  -v "$(CURDIR)":/usr/src/tkr \
	  -v /tmp/tkr-publish-x86-linux:/usr/src/tkr/target \
	  -w /usr/src/tkr rust:1.88 \
	  bash -c "apt-get update -qq && apt-get install -y libdbus-1-dev pkg-config 2>/dev/null && cargo build --release -p tkr"

	@echo "--> Building aarch64-unknown-linux-gnu (Docker)"
	docker run --rm \
	  -v "$(CURDIR)":/usr/src/tkr \
	  -v /tmp/tkr-publish-arm-linux:/usr/src/tkr/target \
	  -w /usr/src/tkr --platform linux/arm64 rust:1.88 \
	  bash -c "apt-get update -qq && apt-get install -y libdbus-1-dev pkg-config 2>/dev/null && cargo build --release -p tkr"

	@echo "--> Packaging"
	@rm -rf /tmp/tkr-release-$(VERSION) && mkdir -p /tmp/tkr-release-$(VERSION)
	tar czf /tmp/tkr-release-$(VERSION)/tkr-aarch64-apple-darwin.tar.gz      -C target/release tkr
	tar czf /tmp/tkr-release-$(VERSION)/tkr-x86_64-apple-darwin.tar.gz       -C target/x86_64-apple-darwin/release tkr
	tar czf /tmp/tkr-release-$(VERSION)/tkr-x86_64-unknown-linux-gnu.tar.gz  -C /tmp/tkr-publish-x86-linux/release tkr
	tar czf /tmp/tkr-release-$(VERSION)/tkr-aarch64-unknown-linux-gnu.tar.gz -C /tmp/tkr-publish-arm-linux/release tkr
	@cd /tmp/tkr-release-$(VERSION) && for f in *.tar.gz; do shasum -a 256 "$$f" | awk '{print $$1}' > "$$f.sha256"; done

	@echo "--> Creating GitHub release v$(VERSION)"
	gh release create v$(VERSION) /tmp/tkr-release-$(VERSION)/*.tar.gz /tmp/tkr-release-$(VERSION)/*.sha256 \
	  --repo einyx/tkr --title "v$(VERSION)" --generate-notes

	@echo "--> Bumping Homebrew tap"
	$(MAKE) _bump-tap VERSION=$(VERSION)

	@echo "==> Done. Run: brew upgrade tkr"

_bump-tap:
	$(eval SHA_ARM_MAC := $(shell cat /tmp/tkr-release-$(VERSION)/tkr-aarch64-apple-darwin.tar.gz.sha256))
	$(eval SHA_X86_MAC := $(shell cat /tmp/tkr-release-$(VERSION)/tkr-x86_64-apple-darwin.tar.gz.sha256))
	$(eval SHA_ARM_LNX := $(shell cat /tmp/tkr-release-$(VERSION)/tkr-aarch64-unknown-linux-gnu.tar.gz.sha256))
	$(eval SHA_X86_LNX := $(shell cat /tmp/tkr-release-$(VERSION)/tkr-x86_64-unknown-linux-gnu.tar.gz.sha256))
	python3 scripts/bump-tap.py "$(TAP)" "$(VERSION)" "$(SHA_ARM_MAC)" "$(SHA_X86_MAC)" "$(SHA_ARM_LNX)" "$(SHA_X86_LNX)"
	cd /opt/homebrew/Library/Taps/einyx/homebrew-tap && \
	  git add Formula/tkr.rb && \
	  git commit -m "release: tkr v$(VERSION)" && \
	  git push
