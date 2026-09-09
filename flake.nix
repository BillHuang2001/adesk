{
  description = "ADesk — AI-native headless Wayland runtime: development environment";

  # `nixpkgs` (indirect) resolves through the local flake registry, so this
  # works offline; `nix flake lock` pins the exact revision.
  inputs.nixpkgs.url = "nixpkgs";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f (import nixpkgs { inherit system; }));
    in
    {
      devShells = forAllSystems (pkgs:
        let
          # Runtime/build libraries needed by smithay + wayland-rs + our crates.
          libs = with pkgs; [
            libxkbcommon     # keyboard keymap handling (smithay hard dependency)
            xkeyboard-config # XKB data files used by libxkbcommon at runtime
            pixman           # software rasterizer (smithay renderer_pixman)
            wayland          # libwayland (protocol XML, optional system backend)
            wayland-protocols
            libdrm
            libgbm
            libGL
            libglvnd         # libEGL / libGLESv2 (dlopen'd by smithay backend_egl)
            libinput
            systemd          # libudev
            seatd            # libseat + seatd daemon
            dbus             # reserved for future AT-SPI integration
          ];
        in
        {
          default = pkgs.mkShell {
            name = "adesk";
            nativeBuildInputs = with pkgs; [
              pkg-config
              rustc
              cargo
              clippy
              rustfmt
              rust-analyzer
              cmake
            ];
            buildInputs = libs;
            shellHook = ''
              export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath libs}:$LD_LIBRARY_PATH"
              # libxkbcommon looks for the XKB data files here.
              export XKB_CONFIG_ROOT="${pkgs.xkeyboard-config}/share/X11/xkb"
              export XKB_CONFIG_EXTRA_PATH="${pkgs.xkeyboard-config}/share/X11/xkb"
              # Mesa software fallback (llvmpipe) so the GL renderer works in CI/VMs.
              export LIBGL_ALWAYS_SOFTWARE="''${LIBGL_ALWAYS_SOFTWARE:-1}"
              export EGL_PLATFORM="''${EGL_PLATFORM:-surfaceless}"
              export RUST_BACKTRACE="''${RUST_BACKTRACE:-1}"
              echo "adesk dev shell: $(rustc --version)"
            '';
          };
        });
    };
}
