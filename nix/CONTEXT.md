# nix/ — Nix packaging for ADesk

## Intent

Nix-side distribution glue kept out of `flake.nix` so the flake stays a thin
wiring layer. The flake imports everything here (no standalone entry points).

## API Surface

- `adesk-module.nix` — `self: { config, lib, pkgs, ... }: { ... }`. A NixOS
  module (exported as `nixosModules.default` / `nixosModules.adesk`) that runs
  `adesk-server` as the `adesk.service` systemd unit and can optionally launch a
  companion agent as `adesk-agent.service`. It takes the flake's `self` so its
  `services.adesk.package` default resolves to
  `self.packages.<system>.adesk`.
- `services.adesk.socketPath` (`types.str`, default `/run/adesk/adesk.sock`) is
  the writable AGP Unix-socket path; `services.adesk.socket` is a `readOnly`
  `types.str` alias of it. Other modules consume
  `config.services.adesk.socket` (always defined, even when the service is
  disabled) instead of re-deriving the path.
- `services.adesk.agent.*` is the pluggable companion-agent submodule
  (`enable`, `package`, `command`, `extraArgs`, `execStart`, `environment`,
  `user`, `group`, `restart`, `restartSec`). The AGP socket is injected as
  `ADESK_SOCKET`; the default `command` is the flake's `adesk-agent`, which can
  be driven without network via `extraArgs = [ "--provider" "dummy" ]`.

## Constraints

- The module must stay evaluable on its own (no `import` of the flake); it is
  wired up by `import ./nix/adesk-module.nix self` in `flake.nix`.
- Option names/defaults must track the `adesk-server` CLI/env surface
  (`crates/adesk-server/src/main.rs`): `ADESK_SOCKET`, `ADESK_OUTPUT`,
  `ADESK_RENDERER`, `ADESK_LOG`, `ADESK_APPS_DIR`, `ADESK_XKB_*`,
  `ADESK_VIEWER_SOCKET`, `ADESK_VIEWER_TCP` and the `--no-viewer` flag.
- The AGP socket defaults under the service `RuntimeDirectory`
  (`/run/adesk`), which is also `XDG_RUNTIME_DIR`; `XKB_CONFIG_ROOT` points at
  `pkgs.xkeyboard-config` (libxkbcommon needs it).

## Verification

- `nix flake show`, `nix flake check`, and
  `nix eval .#packages.x86_64-linux.adesk.drvPath` must be green.
- `nix build .#adesk` installs exactly
  `bin/{adesk-server,adesk-viewer,adesk-machine,adesk-agent}`; `postInstall`
  removes the dev-only `adesk-test-app`.
- Evaluate a `lib.nixosSystem` that imports the module and read
  `config.systemd.services.adesk.serviceConfig.ExecStart` /
  `config.services.adesk.socket` (and `config.system.build.toplevel.drvPath`
  with a complete host config) to catch type/config errors.
