# === Configuration ===
HDK_PATH    := crates/wasmi-plugin-hdk
WORKER_PATH := crates/worker
PLUGIN_NAME := test-plugin
BROWSER     := --firefox

.PHONY: help build-plugin build-worker build-all bench-wasm

help:
	@echo "Usage:"
	@echo "  make bench-wasm    - Build all and run Wasm benchmarks"
	@echo "  make build-all     - Build plugin and worker WASM"

build-plugin:
	@echo "--- Building Plugin WASM ---"
	cargo build -p $(PLUGIN_NAME) --target wasm32-wasip1 --release

build-worker:
	@echo "--- Building Worker Assets ---"
	cd $(WORKER_PATH) && wasm-pack build --target no-modules --out-dir ./pkg --release


build-all: build-plugin build-worker

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
	@cd $(HDK_PATH) && \
	cargo test -p wasmi-plugin-hdk --target wasm32-unknown-unknown

bench-wasm: build-all
	@echo "--- Running Wasm Benchmarks ---"
	@cd $(HDK_PATH) && \
	cargo bench -p wasmi-plugin-hdk --target wasm32-unknown-unknown