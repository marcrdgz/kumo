.PHONY: all build test run install clean

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

clean:
	cargo clean
