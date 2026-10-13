# The menu bar app (macOS). A space in the name: quote every path that holds $(APP).
TRAY        := trello-mcp-tray
TRAY_SRC    := tray/main.swift tray/Panel.swift tray/Procs.swift
APP         := Trello MCP.app
AGENT       := com.trellomcp.tray
AGENT_PLIST := $(HOME)/Library/LaunchAgents/$(AGENT).plist
APP_DIR     := $(HOME)/Applications
# Unload the login agent and wait until it has exited.
stop-agent = launchctl bootout gui/$$(id -u)/$(AGENT) 2>/dev/null; while launchctl print gui/$$(id -u)/$(AGENT) >/dev/null 2>&1; do sleep 0.2; done
SELFTEST    := $(or $(TMPDIR),/tmp)/$(TRAY)-selftest
# Login Items shows the app's name and icon only for a team-signed agent (an
# ad-hoc one shows as "exec"): sign with TEAM's Apple Development identity, as
# gws-mcp does. "-" (ad-hoc) when this Mac has none.
TEAM        ?= S36W2NCK97
SIGN_ID     ?= $(or $(shell security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: .*\)"$$/\1/p' | while read -r n; do security find-certificate -c "$$n" -p 2>/dev/null | openssl x509 -noout -subject 2>/dev/null | grep -q "OU=$(TEAM)" && { echo "$$n"; break; }; done),-)

.PHONY: install build test lint fmt check release clean tray tray-test icon tray-install tray-uninstall

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

# Everything CI-worthy: formatting, clippy, tests; the tray selftest on macOS.
check: lint test $(if $(filter Darwin,$(shell uname)),tray-test)

# Usage: make release VERSION=2.0.0
release:
	@test -n "$(VERSION)" || { echo "usage: make release VERSION=x.y.z" >&2; exit 2; }
	scripts/build-release $(VERSION)

# Drops build caches only; bin/trello-mcp stays.
clean:
	cargo clean

# No XCTest target; the tray's pure parts and libproc calls are checked behind -D SELFTEST, as in claude-usage.
tray-test: ## check the tray's rules, text, libproc calls and stop path
	swiftc -D SELFTEST -suppress-warnings $(TRAY_SRC) -o $(SELFTEST)
	$(SELFTEST)

tray: ## build dist/Trello MCP.app (macOS)
	rm -rf "dist/$(APP)"
	mkdir -p "dist/$(APP)/Contents/MacOS" "dist/$(APP)/Contents/Resources"
	swiftc -O $(TRAY_SRC) -o "dist/$(APP)/Contents/MacOS/$(TRAY)"
	cp tray/Info.plist "dist/$(APP)/Contents/Info.plist"
	cp tray/AppIcon.icns "dist/$(APP)/Contents/Resources/"
	codesign --force -s "$(SIGN_ID)" "dist/$(APP)"

tray-install: tray ## run the menu bar app now and at every login (macOS)
	sed -e 's|@HOME@|$(HOME)|g' tray/$(AGENT).plist > $(AGENT).plist.tmp
	plutil -lint $(AGENT).plist.tmp
	@$(stop-agent)
	mkdir -p "$(APP_DIR)" "$(HOME)/Library/LaunchAgents"
	rm -rf "$(APP_DIR)/$(APP)" && cp -R "dist/$(APP)" "$(APP_DIR)/"
	/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$(APP_DIR)/$(APP)"
	mv $(AGENT).plist.tmp "$(AGENT_PLIST)"
	launchctl bootstrap gui/$$(id -u) "$(AGENT_PLIST)"

tray-uninstall: ## stop the menu bar app and remove its login item and app
	@$(stop-agent)
	rm -rf "$(AGENT_PLIST)" "$(APP_DIR)/$(APP)"

icon: ## render tray/AppIcon.icns from tray/AppIcon.svg (needs rsvg-convert)
	rm -rf dist/AppIcon.iconset && mkdir -p dist/AppIcon.iconset
	for s in 16 32 128 256 512; do \
		rsvg-convert -w $$s -h $$s tray/AppIcon.svg -o dist/AppIcon.iconset/icon_$${s}x$${s}.png; \
		rsvg-convert -w $$((s * 2)) -h $$((s * 2)) tray/AppIcon.svg -o dist/AppIcon.iconset/icon_$${s}x$${s}@2x.png; \
	done
	iconutil -c icns dist/AppIcon.iconset -o tray/AppIcon.icns
