# === Configuration ===
BROWSER     := --firefox
WASM_IN = target/wasm32-wasip1/release/test-plugin.wasm
WASM_OUT = target/wasm32-wasip1/release/test-plugin-async.wasm

.PHONY: help build-plugin build-worker build-all bench-wasm

build-plugin:
	@echo "--- Building Plugin WASM ---"
	cargo build --target wasm32-wasip1 -p test-plugin --release
	wasm-opt $(WASM_IN) -o $(WASM_OUT) --asyncify -O3

build-all: build-plugin

clippy-native: build-all
	@echo "--- Running Clippy Lints ---"
	cargo clippy -- -D warnings

clippy-wasm: build-all
	@echo "--- Running Clippy Lints for Wasm ---"
	cargo clippy \
		-Z build-std=std,panic_abort \
		-p wasmi-plugin-hdk \
		--target wasm32-unknown-unknown \
		-- -D warnings

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