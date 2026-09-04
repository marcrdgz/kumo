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
	@mkdir -p ~/.config/opencode ~/.opencode ~/.claude ~/.codex
	@for d in ~/.config/opencode ~/.opencode ~/.claude ~/.codex; do \
		cp kumo-agents.md "$$d/kumo-agents.md" 2>/dev/null || true; \
	done
	@mkdir -p ~/.config/opencode/skills/kumo ~/.agents/skills/kumo ~/.claude/skills/kumo ~/.codex/skills/kumo
	@cp skills/kumo/SKILL.md ~/.config/opencode/skills/kumo/SKILL.md 2>/dev/null || true
	@cp skills/kumo/SKILL.md ~/.agents/skills/kumo/SKILL.md 2>/dev/null || true
	@cp skills/kumo/SKILL.md ~/.claude/skills/kumo/SKILL.md 2>/dev/null || true
	@cp skills/kumo/SKILL.md ~/.codex/skills/kumo/SKILL.md 2>/dev/null || true
	@echo "kumo installed — skill at ~/.config/kumo/kumo-agents.md (mirrored to opencode/claude/codex + skills/kumo/SKILL.md)"
	@echo "hint: restart agent sessions after install so opencode picks up the new skill"

docs:
	npm run dev --prefix docs

docs-build:
	npm ci --prefix docs
	DOCS_BASE_PATH=/kumo npm run build --prefix docs

clean:
	cargo clean
