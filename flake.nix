{
  description = "Blocs — générateur de blocs diagrammes";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Loaded at runtime by winit / wgpu (eframe) and rfd's file dialog (dbus), not linked.
      runtimeLibs = pkgs: pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (with pkgs; [
        dbus
        wayland
        libxkbcommon
        vulkan-loader
        libGL
        libx11
        libxcursor
        libxi
        libxrandr
      ]);
    in
    {
      # `nix run github:aualbert/blocs`
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "blocs";
          version = "0.1.0";
          # The program's files only: editing the README or docs does not rebuild it.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./.cargo ./src ./assets ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.makeWrapper pkgs.pkg-config ];
          buildInputs = [ pkgs.aws-lc ]; # rustls's crypto, from nixpkgs rather than built
          doCheck = false; # some tests need a GPU, absent from the build sandbox
          postFixup = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
            wrapProgram $out/bin/blocs --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)}
          '';
          meta.mainProgram = "blocs";
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            pkg-config
          ];

          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (runtimeLibs pkgs);
        };
      });
    };
}
