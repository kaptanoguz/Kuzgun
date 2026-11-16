{
  description = "Fast native WhatsApp client";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # rust-toolchain.toml pins the compiler so local builds and CI agree.
    # This reads that file rather than restating the version here.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      systems = [
        "aarch64-linux"
        "x86_64-linux"
        # Nixpkgs unstable no longer supports x86_64-darwin.
        "aarch64-darwin"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ (import rust-overlay) ];
            }
          )
        );
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          inherit (pkgs.stdenv.hostPlatform) isLinux;
        in
        {
          default = pkgs.mkShell (
            {
              packages =
                with pkgs;
                [
                  (rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
                  rust-analyzer
                  pkg-config
                  cmake
                  perl
                ]
                # On macOS the app links the system frameworks instead.
                ++ lib.optionals isLinux [
                  alsa-lib
                  pipewire
                  # The PipeWire output's bindings are made by bindgen,
                  # which needs libclang.
                  rustPlatform.bindgenHook
                  libxkbcommon
                  wayland
                  libGL
                  libx11
                  libxcursor
                  libxi
                  libxrandr
                ];
              ZAPFAST_TEST_RTL_FONT = "${pkgs.dejavu_fonts}/share/fonts/truetype/DejaVuSans.ttf";
            }
            // pkgs.lib.optionalAttrs isLinux {
              # The GUI dlopens its Wayland, X11 and GL libraries at run time.
              LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
                with pkgs;
                [
                  libxkbcommon
                  wayland
                  libGL
                  libx11
                  libxcursor
                  libxi
                  libxrandr
                ]
              );
            }
          );
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt-tree);
    };
}
