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
- Evaluate a `lib.nixosSystem` that imports the module and read
  `config.systemd.services.adesk.serviceConfig.ExecStart` /
  `config.services.adesk.socket` to catch type/config errors.
