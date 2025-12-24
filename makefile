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

bench-wasm: build-all
	@echo "--- Running Wasm Benchmarks ---"
	@cd $(HDK_PATH) && \
	export WASM_BINDGEN_TEST_TIMEOUT=300 && \
	CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner && \
	cargo bench -p wasmi-plugin-hdk --target wasm32-unknown-unknown