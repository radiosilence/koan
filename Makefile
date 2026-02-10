.PHONY: build build-rust generate-bindings build-xcframework build-swift run clean test

build:
	./scripts/build.sh

build-rust:
	./scripts/build-rust.sh

generate-bindings:
	./scripts/generate-bindings.sh

build-xcframework:
	./scripts/build-xcframework.sh

build-swift:
	cd swift && swift build

run: build
	cd swift && swift run

clean:
	cargo clean
	rm -rf swift/.build
	rm -rf swift/KoanRust.xcframework
	rm -rf swift/Generated

test:
	cargo test --workspace
	cargo clippy --workspace -- -D warnings
