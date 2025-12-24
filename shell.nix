let
  pkgs = import <nixpkgs> {
    overlays = [
      (import (builtins.fetchTarball "https://github.com/oxalica/rust-overlay/archive/master.tar.gz"))
    ];
  };
  unstable = import <nixpkgs-unstable> { };
  wasm-bindgen-cli_0_2_106 = pkgs.callPackage ./flakes/wasm-bindgen-cli.nix { };
in
pkgs.mkShell {
  packages = with pkgs; [
    # Rust toolchain with WASI target
    (rust-bin.stable.latest.default.override {
      extensions = [ "rust-src" ];
      targets = [
        "wasm32-wasip1"
        "wasm32-unknown-unknown"
      ];
    })

    cargo
    cargo-sort
    cargo-machete
    samply
    rustfmt
    binaryen
    rust-bin.stable.latest.rust-analyzer
    wasm-pack
    geckodriver
    wasm-bindgen-cli_0_2_106
  ];
}
