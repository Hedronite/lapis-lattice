{
  description = "Lapis, with the tome-tree spike (dev shell and binaries)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      each = f: nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rust;
            rustc = rust;
          };
          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "ratatui-textarea-0.9.2" = "sha256-884ZPDer6zgPQFS5S8dvQpINF1D+L0+sml4rlH+U6uU=";
            };
          };
          # `--bins` installs `lapis` (built with feature `tome`) and any other
          # workspace binary. `tome-eval` is picked up here once that bin exists.
          bins = rustPlatform.buildRustPackage {
            pname = "lapis";
            version = "0.4.2";
            src = ./.;
            inherit cargoLock;
            buildFeatures = [ "tome" ];
            cargoBuildFlags = [ "--bins" ];
            doCheck = false;
          };
        in
        f { inherit pkgs rust rustPlatform bins cargoLock; }
      );
    in
    {
      devShells = each ({ pkgs, rust, ... }: {
        default = pkgs.mkShell {
          packages = [
            rust
            pkgs.pkg-config
            pkgs.poppler-utils
          ];
        };
      });

      packages = each ({ bins, ... }: {
        default = bins;
        lapis = bins;
        tome-eval = bins;
      });

      checks = each ({ rustPlatform, cargoLock, ... }: {
        tome-tree = rustPlatform.buildRustPackage {
          pname = "tome-tree-check";
          version = "0.0.0";
          src = ./.;
          inherit cargoLock;
          cargoBuildFlags = [ "-p" "tome-tree" "--all-targets" ];
          cargoTestFlags = [ "-p" "tome-tree" "--all-features" ];
          doCheck = true;
          preCheck = ''
            cargo clippy -p tome-tree --all-targets --all-features -- -D warnings
          '';
          # The check's product is the test run, not an installed binary.
          installPhase = "mkdir -p $out";
        };
      });
    };
}
