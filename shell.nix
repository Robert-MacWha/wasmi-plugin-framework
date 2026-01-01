let
  pkgs = import <nixpkgs> {
    overlays = [
      (import (builtins.fetchTarball "https://github.com/oxalica/rust-overlay/archive/master.tar.gz"))
    ];
  };
  rustToolchain = pkgs.rust-bin.nightly.latest.default.override {
    extensions = [ "rust-src" ];
    targets = [
      "wasm32-wasip1"
      "wasm32-unknown-unknown"
    ];
  };
  wasm-bindgen-cli_0_2_106 = pkgs.callPackage ./flakes/wasm-bindgen-cli.nix { };
in
pkgs.mkShell {
  packages = with pkgs; [
    rustToolchain
    rust-analyzer
    cargo-sort
    cargo-machete
    cargo-udeps
    samply
    binaryen
    wasm-pack
    geckodriver
    wasm-bindgen-cli_0_2_106
    wasm-tools
    wabt
  ];

  shellHook = ''
    export WASM_BINDGEN_THREADS_HEADERS=1
    export WASM_BINDGEN_TEST_TIMEOUT=100
    export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library"
  '';
}
