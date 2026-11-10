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
        # `wayland-sys` uses its `dlopen` backend (no pkg-config probing, which
        # is also what lets plain `cargo build` work on NixOS), so nothing is
        # linked at build time and the runtime loader must find libwayland,
        # libvulkan and the GL driver: wrap the binary with them.
        runtimeLibs = pkgs.lib.makeLibraryPath [
          pkgs.wayland
          pkgs.vulkan-loader
          pkgs.libglvnd
        ];

        mkHandwriting = pkgs.rustPlatform.buildRustPackage {
          pname = "handwriting-input";
          version = "0.1.0";
          src = pkgs.lib.cleanSource ./.;

          cargoLock.lockFile = ./Cargo.lock;
          # Hash of the vendored crate tarball from Cargo.lock. After changing
          # dependencies, rebuild with a throwaway value (pkgs.lib.fakeHash) and
          # paste the "got:" hash the build error prints.
          cargoHash = "sha256-JZWqYCOyFtDQzBrM2Ui1pV2I6rojSrMUBXpyBw+J0ZE=";

          nativeBuildInputs = [ pkgs.makeWrapper ];

          doCheck = true; # offline unit tests only

          postInstall = ''
            wrapProgram $out/bin/handwriting-input \
              --prefix LD_LIBRARY_PATH : "${runtimeLibs}:/run/opengl-driver/lib"
          '';

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
            pkgs.vulkan-loader
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
