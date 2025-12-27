# === Configuration ===
BROWSER     := --firefox

.PHONY: help build-plugin build-worker build-all bench-wasm

build-plugin:
	@echo "--- Building Plugin WASM ---"
	cargo build --target wasm32-wasip1 -p test-plugin --release

build-all: build-plugin

clippy: build-all
	@echo "--- Running Clippy Lints ---"
	cargo clippy -- -D warnings

test-native: build-plugin
	@echo "--- Running Plugin Tests ---"
	cargo test

bench-native: build-plugin
	@echo "--- Running Plugin Benchmarks ---"
	cargo bench

test-wasm: build-all
	@echo "--- Running Wasm Tests ---"
	cargo test \
		-Z build-std=std,panic_abort \
		-p wasmi-plugin-hdk \
		--target wasm32-unknown-unknown

bench-wasm: build-all
	@echo "--- Running Wasm Benchmarks ---"
	cargo bench \
		-Z build-std=std,panic_abort \
		-p wasmi-plugin-hdk \
		--target wasm32-unknown-unknown