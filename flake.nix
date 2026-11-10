{
  description = "Handwriting input keyboard for Linux (Wayland, egui)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        # Rust pinned per rustPlatform of this nixpkgs rev
        mkHandwriting = pkgs.rustPlatform.buildRustPackage {
          pname = "handwriting-input";
          version = "0.1.0";
          src = ./.; # clean source filter via lib.cleanSource

          cargoLock.lockFile = ./Cargo.lock;
          # only needed if we add git deps:
          # cargoLock.outputHashes = { ... };

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [
            pkgs.wayland # libwayland (wayland-sys "system" or dlopen)
            pkgs.libxkbcommon # xkbcommon for keymap generation
          ];

          # wgpu loads Vulkan/EGL at runtime via dlopen — no build-time need,
          # but document runtime deps in meta + README.
          doCheck = true; # cargo test (offline api tests)

          meta = {
            description = "Handwriting input keyboard using Google handwriting API on Wayland";
            license = pkgs.lib.licenses.mit;
            mainProgram = "handwriting-input";
          };
        };
      in
      {
        packages = {
          default = mkHandwriting;
          handwriting-input = mkHandwriting;
        };

        apps.default = {
          type = "app";
          program = "${mkHandwriting}/bin/handwriting-input";
        };

        devShells.default = pkgs.mkShell {
          packages = [
            pkgs.rustc
            pkgs.cargo
            pkgs.rustfmt
            pkgs.clippy
            pkgs.pkg-config
            pkgs.wayland
            pkgs.libxkbcommon
            pkgs.rust-analyzer
          ];
          env.RUST_LOG = "info";
        };

        checks = {
          build = self.packages.${system}.default;
          clippy =
            pkgs.runCommand "clippy"
              {
                buildInputs = [
                  pkgs.cargo
                  pkgs.rustc
                ];
              }
              ''
                cd ${./.} && cargo clippy --all-targets -- -D warnings
              '';
        };
      }
    );
}
