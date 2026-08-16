.PHONY: all build test run install clean

all: build

build:
	cargo build

test:
	cargo test

run:
	cargo run -p kumo

install:
	cargo install --path app/cli --locked
	cargo install --path app/daemon --locked

clean:
	cargo clean
