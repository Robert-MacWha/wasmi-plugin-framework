# === Configuration ===
BROWSER     := --firefox

.PHONY: help build-plugin build-coordinator build-all bench-wasm

build-plugin:
	@echo "--- Building Plugin WASM ---"
	cargo build --target wasm32-wasip1 -p test-plugin --release

build-coordinator:
	@echo "--- Building Coordinator WASM ---"
	cd crates/wasmi-plugin-coordinator && wasm-pack build \
        --target web \
        --out-dir pkg \
		--release \

build-all: build-plugin build-coordinator

clippy-native: build-all
	@echo "--- Running Clippy Lints ---"
	cargo clippy -- -D warnings

clippy-wasm: build-all
	@echo "--- Running Clippy Lints for Wasm ---"
	cargo clippy \
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
		-p wasmi-plugin-hdk \
		--target wasm32-unknown-unknown

bench-wasm: build-all
	@echo "--- Running Wasm Benchmarks ---"
	cargo bench \
		-p wasmi-plugin-hdk \
		--target wasm32-unknown-unknown