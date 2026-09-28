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
          # `--bins` installs `lapis` (built with feature `tome`) and any other
          # workspace binary. `tome-eval` is picked up here once that bin exists.
          bins = rustPlatform.buildRustPackage {
            pname = "lapis";
            version = "0.4.2";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            buildFeatures = [ "tome" ];
            cargoBuildFlags = [ "--bins" ];
            doCheck = false;
          };
          # The A/B harness alone (`tome-eval run|check|validate`). No cargo feature needed.
          tomeEval = rustPlatform.buildRustPackage {
            pname = "tome-eval";
            version = "0.0.1";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "-p" "tome-eval" ];
            doCheck = false;
          };
        in
        f { inherit pkgs rust rustPlatform bins tomeEval; }
      );
    in
    {
      devShells = each ({ pkgs, rust, ... }: {
        default = pkgs.mkShell {
          packages = [
            rust
            pkgs.pkg-config
            pkgs.poppler_utils
            # tome-eval: regenerate evals/tome/data/chunk-pages.jsonl (read-only sqlite + jq).
            pkgs.sqlite
            pkgs.jq
          ];
        };
      });

      packages = each ({ bins, tomeEval, ... }: {
        default = bins;
        lapis = bins;
        tome-eval = tomeEval;
      });

      checks = each ({ rustPlatform, ... }: {
        tome-tree = rustPlatform.buildRustPackage {
          pname = "tome-tree-check";
          version = "0.0.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          buildFeatures = [ "tome" ];
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
