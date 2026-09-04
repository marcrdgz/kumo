.PHONY: all build test run install clean docs docs-build

# Xcode 26.5+ SDKs dropped the arm64-macos slice from libSystem.tbd, so zig
# 0.15 (libghostty-vt's pinned toolchain) resolves zero symbols from it and
# every native link fails. The CommandLineTools SDK still carries the slice,
# so scope macOS builds to it. Override with `make DEVELOPER_DIR=...` or an
# exported env var if your setup differs.
ifeq ($(shell uname),Darwin)
ifeq ($(shell test -d /Library/Developer/CommandLineTools && echo yes),yes)
DEVELOPER_DIR ?= /Library/Developer/CommandLineTools
export DEVELOPER_DIR
endif
endif

all: build

build:
	cargo build

test:
	cargo test

run:
	cargo run -p kumo

install:
	cargo install --path app/kumo --locked
	@mkdir -p ~/.config/kumo
	@cp kumo-agents.md ~/.config/kumo/kumo-agents.md 2>/dev/null || true
	@for d in ~/.config/opencode ~/.opencode ~/.claude ~/.codex; do \
		if [ -d "$$d" ]; then cp kumo-agents.md "$$d/kumo-agents.md" 2>/dev/null || true; fi; \
	done
	@echo "kumo installed — skill at ~/.config/kumo/kumo-agents.md (mirrored to opencode/claude/codex if present)"

docs:
	npm run dev --prefix docs

docs-build:
	npm ci --prefix docs
	DOCS_BASE_PATH=/kumo npm run build --prefix docs

clean:
	cargo clean
