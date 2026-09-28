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
        in
        f { inherit pkgs rust rustPlatform bins; }
      );
    in
    {
      devShells = each ({ pkgs, rust, ... }: {
        default = pkgs.mkShell {
          packages = [
            rust
            pkgs.pkg-config
            pkgs.poppler_utils
          ];
        };
      });

      packages = each ({ bins, ... }: {
        default = bins;
        lapis = bins;
        tome-eval = bins;
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
