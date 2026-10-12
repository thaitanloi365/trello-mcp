.PHONY: install

# Build and replace bin/trello-mcp. install(1) writes a new inode, so a
# running server and macOS code signing stay intact.
install:
	cargo build --release --locked
	mkdir -p bin
	install -m 0755 target/release/trello-mcp bin/trello-mcp
	bin/trello-mcp version
