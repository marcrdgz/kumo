.PHONY: all build test run install clean

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
