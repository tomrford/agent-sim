{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    zig-overlay.url = "github:mitchellh/zig-overlay";
    zig-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    rust-overlay,
    zig-overlay,
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay) zig-overlay.overlays.default];
        };
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = ["rust-src" "rustfmt" "clippy" "rust-analyzer"];
        };

        cargoToml = builtins.fromTOML (builtins.readFile ./runtime/Cargo.toml);

        agent-sim = pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          version = cargoToml.package.version;
          src = ./.;
          cargoRoot = "runtime";
          buildAndTestSubdir = "runtime";
          cargoLock.lockFile = ./runtime/Cargo.lock;
          buildType = "release";
          doCheck = false;
        };
      in {
        packages = {
          default = agent-sim;
          agent-sim = agent-sim;
        };

        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
            pkgs.zigpkgs."0.15.2"
          ];
        };
      }
    );
}
