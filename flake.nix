{
  description = "ADesk — AI-native headless Wayland runtime: packages, NixOS module and development environment";

  # `nixpkgs` (indirect) resolves through the local flake registry, so this
  # works offline; `nix flake lock` pins the exact revision.
  inputs.nixpkgs.url = "nixpkgs";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f (import nixpkgs { inherit system; }));

      # Native libraries the runtime links against (or dlopen's at runtime),
      # plus the `dbus` tooling package (see below). Shared by the package and
      # the dev shell; keep in sync with the "Running in a container" section
      # of README.md.
      adeskLibraries = pkgs: with pkgs; [
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
        dbus             # tooling only: provides dbus-daemon for a private session bus
                         # (the adesk-a11y AT-SPI2 client uses the pure-Rust atspi/zbus
                         # crates and never links libdbus; the accessibility integration
                         # test starts a private bus, and a real AT-SPI session can too)
      ];

      # Native libraries only the GTK4/libadwaita viewer front-end
      # (`adesk-viewer-gui`) needs. Kept separate from `adeskLibraries` so the
      # headless viewer/server/machine/agent binaries never gain a GTK
      # dependency. The package includes `adesk-viewer-gui` (the workspace root
      # is a virtual manifest, so `cargo build` builds every member) and thus
      # needs these on PKG_CONFIG_PATH; the dev shell needs them to build and
      # test the workspace. `gtk4`/`libadwaita` propagate their own pkg-config
      # deps (pango, cairo, gdk-pixbuf, ...).
      adeskGuiLibraries = pkgs: with pkgs; [
        gtk4
        libadwaita
      ];
    in
    {
      # `nix build .#adesk` (or `.#default`) builds every ADesk binary —
      # adesk-server, adesk-viewer, adesk-viewer-gui, adesk-machine and
      # adesk-agent — from the workspace's pinned Cargo.lock.
      packages = forAllSystems (pkgs:
        let
          adesk = pkgs.rustPlatform.buildRustPackage {
            pname = "adesk";
            version = "0.1.0";

            src = ./.;

            # Use the workspace lockfile directly; no hand-maintained vendorHash.
            cargoLock.lockFile = ./Cargo.lock;

            # `.cargo/config.toml` requests `-fuse-ld=mold`, so `mold` must be
            # on PATH wherever cargo links — the package build and the dev shell.
            nativeBuildInputs = with pkgs; [ pkg-config cmake mold ];
            # Wayland-sys/xkbcommon build scripts find the libraries below via
            # the `pkg-config` setup hook's PKG_CONFIG_PATH; the Nix linker
            # wrapper records them in the binaries' RUNPATH (several are also
            # dlopen'd at runtime). `adeskGuiLibraries` is required because the
            # workspace root is a virtual manifest, so building the package
            # builds `adesk-viewer-gui` too.
            buildInputs = (adeskLibraries pkgs) ++ (adeskGuiLibraries pkgs);

            # The test suites start a real compositor and need a runtime
            # environment (XDG_RUNTIME_DIR, xkb data, sockets). Building the
            # package must not require one; run them via ./scripts/dev.sh.
            doCheck = false;

            # Ship the five user-facing binaries only; `adesk-testkit`'s
            # dev-only fixture binary is not part of the runtime.
            postInstall = ''
              rm -f $out/bin/adesk-test-app
            '';

            meta = with pkgs.lib; {
              description = "AI-native headless Wayland runtime (AGP server, viewer + GTK front-end, machine and agent)";
              homepage = "https://example.invalid/adesk";
              license = with licenses; [ mit asl20 ];
              platforms = platforms.linux;
              mainProgram = "adesk-server";
            };
          };
        in
        {
          adesk = adesk;
          default = adesk;
        });

      # NixOS module: run adesk-server as a systemd service and optionally a
      # companion agent. `default` and `adesk` are the same module.
      nixosModules = {
        default = import ./nix/adesk-module.nix self;
        adesk = import ./nix/adesk-module.nix self;
      };

      devShells = forAllSystems (pkgs:
        let
          libs = adeskLibraries pkgs;
          guiLibs = adeskGuiLibraries pkgs;
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
              mold
            ];
            buildInputs = libs ++ guiLibs;
            shellHook = ''
              export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath (libs ++ guiLibs)}:$LD_LIBRARY_PATH"
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
