.PHONY: all dev build test run clean

all: build

dev:
	npm run tauri dev

build:
	npm run build
	cd src-tauri && cargo build

test:
	cd src-tauri && cargo test

run:
	npm run tauri dev

clean:
	rm -rf dist
	cd src-tauri && cargo clean
