.PHONY: install build test lint fmt check release clean

# Build and replace bin/trello-mcp. install(1) writes a new inode, so a
# running server and macOS code signing stay intact.
install: build
	mkdir -p bin
	install -m 0755 target/release/trello-mcp bin/trello-mcp
	bin/trello-mcp version

build:
	cargo build --release --locked

test:
	cargo test --locked

lint:
	cargo fmt --check
	cargo clippy --all-targets --locked -- -D warnings

fmt:
	cargo fmt

# Everything CI-worthy: formatting, clippy, tests.
check: lint test

# Usage: make release VERSION=2.0.0
release:
	@test -n "$(VERSION)" || { echo "usage: make release VERSION=x.y.z" >&2; exit 2; }
	scripts/build-release $(VERSION)

# Drops build caches only; bin/trello-mcp stays.
clean:
	cargo clean
