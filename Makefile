.PHONY: all run test check clean

all: build

build:
	zig build

run:
	zig build run

test:
	zig build test

check:
	zig build check

clean:
	rm -rf .zig-cache zig-out
