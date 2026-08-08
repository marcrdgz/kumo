.PHONY: all build test run clean

all: build

build:
	cargo build

test:
	cargo test

run:
	cargo run -p kumo

clean:
	cargo clean
