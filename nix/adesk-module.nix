# NixOS module for ADesk — the AI-native headless Wayland runtime.
#
# This module installs and runs `adesk-server` as a systemd service, creates a
# dedicated system user plus a writable runtime directory for the AGP Unix
# socket, and can optionally launch a companion *agent* process (any program
# that speaks the Agent GUI Protocol) which is automatically pointed at the
# runtime's socket.
#
# Every option has a `description`; `nixos-help` / the generated
# `configuration.nix(5)` manual render them as documentation.
#
# ---------------------------------------------------------------------------
# Usage (flake consumer):
#
#   {
#     inputs.adesk.url = "github:example/adesk";
#     outputs = { self, nixpkgs, adesk }: {
#       nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
#         system = "x86_64-linux";
#         modules = [
#           adesk.nixosModules.default
#           {
#             services.adesk = {
#               enable = true;
#               renderer = "pixman";         # no GPU required
#               output = "1280x800";
#               viewer.enable = true;
#
#               agent = {
#                 enable = true;
#                 # The default command is the flake's `adesk-agent` binary; point
#                 # it at your own AGP client instead if you prefer.
#                 command = [ "''${config.services.adesk.package}/bin/adesk-agent" "--task" "open the editor" ];
#               };
#             };
#
#             # `services.adesk.socket` is a stable, read-only alias for
#             # `services.adesk.socketPath`: reference it from any other unit.
#             systemd.services.my-sidecar.environment.ADESK_SOCKET =
#               config.services.adesk.socket;
#           }
#         ];
#       };
#     };
#   }
# ---------------------------------------------------------------------------
self:
{ config, lib, pkgs, ... }:

let
  cfg = config.services.adesk;

  inherit (lib)
    mkEnableOption mkIf mkMerge mkOption types optionalAttrs optionals
    concatStringsSep escapeShellArgs literalExpression toString;

  # The flake's `adesk` package for the host platform. Used as the default for
  # `services.adesk.package`; a clear evaluation error is produced on platforms
  # the flake does not build for.
  defaultPackage =
    self.packages.${pkgs.stdenv.hostPlatform.system}.adesk or
    (throw "services.adesk: this flake publishes no `adesk` package for ${pkgs.stdenv.hostPlatform.system}");

  # Where libxkbcommon finds the XKB data files (smithay's keyboard setup needs
  # it); the dev shell and README use the same convention.
  xkbDir = "${pkgs.xkeyboard-config}/share/X11/xkb";

  # `--no-viewer` is a flag, the other viewer knobs are environment variables.
  viewerArgs = optionals (!cfg.viewer.enable) [ "--no-viewer" ];

  viewerEnv = optionalAttrs (cfg.viewer.socket != null) {
    ADESK_VIEWER_SOCKET = cfg.viewer.socket;
  } // optionalAttrs (cfg.viewer.tcp != null) {
    ADESK_VIEWER_TCP = cfg.viewer.tcp;
  };

  appsDirEnv = optionalAttrs (cfg.appsDir != [ ]) {
    ADESK_APPS_DIR = concatStringsSep ":" (map toString cfg.appsDir);
  };

  accessibilityEnv = {
    ADESK_ACCESSIBILITY = cfg.accessibility;
  };

  recordingsDirEnv = optionalAttrs (cfg.recordingsDir != null) {
    ADESK_RECORDINGS_DIR = toString cfg.recordingsDir;
  };

  xkbEnv =
    optionalAttrs (cfg.xkb.layout != null) { ADESK_XKB_LAYOUT = cfg.xkb.layout; }
    // optionalAttrs (cfg.xkb.variant != null) { ADESK_XKB_VARIANT = cfg.xkb.variant; }
    // optionalAttrs (cfg.xkb.model != null) { ADESK_XKB_MODEL = cfg.xkb.model; }
    // optionalAttrs (cfg.xkb.rules != null) { ADESK_XKB_RULES = cfg.xkb.rules; };

  # Environment shared by the runtime and its companion agent.
  runtimeEnv = {
    XDG_RUNTIME_DIR = "/run/${cfg.runtimeDirectory}";
    ADESK_SOCKET = cfg.socketPath;
    XKB_CONFIG_ROOT = xkbDir;
    XKB_CONFIG_EXTRA_PATH = xkbDir;
  };
in
{
  options.services.adesk = {
    enable = mkEnableOption "ADesk, the AI-native headless Wayland runtime (adesk-server)";

    package = mkOption {
      type = types.package;
      default = defaultPackage;
      defaultText = literalExpression "self.packages.\${system}.adesk";
      description = ''
        The ADesk runtime package. It must provide `bin/adesk-server` (and, for
        the companion agent, `bin/adesk-agent`).
      '';
    };

    user = mkOption {
      type = types.str;
      default = "adesk";
      description = ''
        User the runtime (and, by default, the companion agent) runs as. The
        user is created automatically unless it is `root`.
      '';
    };

    group = mkOption {
      type = types.str;
      default = "adesk";
      description = ''
        Group the runtime runs as. Created automatically unless it is `root`.
      '';
    };

    runtimeDirectory = mkOption {
      type = types.str;
      default = "adesk";
      example = "adesk";
      description = ''
        Name of the systemd `RuntimeDirectory` under `/run` that hosts the AGP
        socket and the compositor's Wayland socket. It is created (and owned by
        `services.adesk.user`) on service start and exposed to the process as
        `XDG_RUNTIME_DIR`.

        It must match the parent directory of
        [](#opt-services.adesk.socketPath).
      '';
    };

    socketPath = mkOption {
      type = types.str;
      default = "/run/adesk/adesk.sock";
      example = "/run/adesk/adesk.sock";
      description = ''
        Absolute path of the AGP (Agent GUI Protocol) Unix socket the runtime
        listens on. It is exported to clients as `ADESK_SOCKET`.

        The default lives inside the service's
        [](#opt-services.adesk.runtimeDirectory), which systemd creates and
        chowns to `services.adesk.user`. If you relocate the socket, make sure
        its parent directory exists and is writable by that user (for example
        with `systemd.tmpfiles.rules`).

        Other units should not re-derive this value; reference the read-only
        [](#opt-services.adesk.socket) alias instead.
      '';
    };

    socket = mkOption {
      type = types.str;
      readOnly = true;
      description = ''
        Read-only alias for [](#opt-services.adesk.socketPath) — the AGP socket
        path as a plain string, convenient to thread into another service, e.g.

        ```nix
        systemd.services.my-agent.environment.ADESK_SOCKET = config.services.adesk.socket;
        systemd.services.my-agent.environment.ADESK_VIEWER_SOCKET = config.services.adesk.socket; # see viewer.socket
        ```
      '';
    };

    output = mkOption {
      type = types.str;
      default = "1280x800";
      example = "1920x1080";
      description = ''
        Size of the single virtual output as `WxH`. Maps to `ADESK_OUTPUT`.
      '';
    };

    renderer = mkOption {
      type = types.enum [ "auto" "gl" "pixman" ];
      default = "auto";
      description = ''
        Renderer selection. Maps to `ADESK_RENDERER`. `auto` tries a
        surfaceless EGL/GL context and falls back to pixman; use `pixman` on
        machines without a GPU (e.g. most servers and VMs).
      '';
    };

    accessibility = mkOption {
      type = types.enum [ "auto" "off" ];
      default = "auto";
      description = ''
        Accessibility (AT-SPI2) backend selection, in the same spirit as
        [](#opt-services.adesk.renderer). Maps to `ADESK_ACCESSIBILITY`.
        `auto` connects lazily to the session D-Bus on first use and degrades
        to `not_supported` when no bus is reachable, whereas `off` never touches
        D-Bus. `auto` therefore needs a reachable session bus (and accessible
        applications) to answer the AGP §5.11 methods.
      '';
    };

    log = mkOption {
      type = types.str;
      default = "info";
      example = "adesk_server=debug,info";
      description = ''
        `tracing-subscriber` env-filter directive. Maps to `ADESK_LOG`.
      '';
    };

    appsDir = mkOption {
      type = types.listOf types.path;
      default = [ ];
      example = literalExpression ''[ "/run/current-system/sw/share/applications" ]'';
      description = ''
        Extra `.desktop` application directories, colon-joined into
        `ADESK_APPS_DIR`. Leave empty to use the XDG default search set.
      '';
    };

    recordingsDir = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = "/var/lib/adesk/recordings";
      description = ''
        Directory recordings started without an explicit path are written to.
        Maps to `ADESK_RECORDINGS_DIR`. When `null` the runtime derives
        `<AGP socket dir>/adesk-recordings` from
        [](#opt-services.adesk.socketPath).
      '';
    };

    xkb = {
      layout = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "us";
        description = "xkb layout list (`ADESK_XKB_LAYOUT`), e.g. `us` or `de,us`.";
      };
      variant = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "nodeadkeys";
        description = "xkb variant list (`ADESK_XKB_VARIANT`).";
      };
      model = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "pc105";
        description = "xkb model (`ADESK_XKB_MODEL`).";
      };
      rules = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "evdev";
        description = "xkb rules file (`ADESK_XKB_RULES`).";
      };
    };

    viewer = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Serve the VAP viewer endpoint (desktop streaming + limited input).
          When disabled, `--no-viewer` is passed to `adesk-server`.
        '';
      };
      socket = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "/run/adesk/adesk-viewer.sock";
        description = ''
          Explicit viewer Unix socket path (`ADESK_VIEWER_SOCKET`). When
          `null` the runtime derives it as the AGP socket's sibling
          (`<socketPath dir>/adesk-viewer.sock`).
        '';
      };
      tcp = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "127.0.0.1:7100";
        description = ''
          Optional TCP viewer listener as `HOST:PORT` (`ADESK_VIEWER_TCP`), for
          a viewer that is not on the same host.
        '';
      };
    };

    environment = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example = literalExpression ''{ RUST_BACKTRACE = "1"; }'';
      description = ''
        Extra environment variables for the `adesk.service` unit, merged over
        the module's own `ADESK_*` / `XDG_RUNTIME_DIR` settings.
      '';
    };

    extraArgs = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [ "--renderer" "pixman" ];
      description = ''
        Extra command-line arguments appended to `adesk-server`'s `ExecStart`.
      '';
    };

    agent = {
      enable = mkEnableOption "a companion ADesk agent process (any AGP client)";

      package = mkOption {
        type = types.package;
        default = cfg.package;
        defaultText = literalExpression "config.services.adesk.package";
        description = ''
          Package providing the agent binary used by the default
          [](#opt-services.adesk.agent.command).
        '';
      };

      command = mkOption {
        type = types.listOf types.str;
        default = [ "${cfg.agent.package}/bin/adesk-agent" ];
        defaultText = literalExpression ''[ "''${config.services.adesk.package}/bin/adesk-agent" ]'';
        example = literalExpression ''[ "''${pkgs.my-agent}/bin/my-agent" "--task" "open the editor" ]'';
        description = ''
          Argument vector of the agent process. The runtime socket is always
          available to it as `ADESK_SOCKET`; you may also interpolate
          `config.services.adesk.socket` directly into the command.
        '';
      };

      execStart = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = ''
          Fully-formed `ExecStart` string. When set it overrides
          [](#opt-services.adesk.agent.command) (and
          [](#opt-services.adesk.agent.extraArgs)).
        '';
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [ ];
        description = ''
          Extra arguments appended after
          [](#opt-services.adesk.agent.command).
        '';
      };

      environment = mkOption {
        type = types.attrsOf types.str;
        default = { };
        example = literalExpression ''{ OPENAI_API_KEY = "..."; }'';
        description = ''
          Extra environment variables for the agent unit. `ADESK_SOCKET` (the
          runtime socket) and `XDG_RUNTIME_DIR` are always set.
        '';
      };

      user = mkOption {
        type = types.str;
        default = cfg.user;
        defaultText = literalExpression "config.services.adesk.user";
        description = "User the agent runs as. Not created automatically.";
      };

      group = mkOption {
        type = types.str;
        default = cfg.group;
        defaultText = literalExpression "config.services.adesk.group";
        description = "Group the agent runs as. Not created automatically.";
      };

      restart = mkOption {
        type = types.enum [
          "no" "on-success" "on-failure" "on-abnormal" "on-watchdog" "on-abort" "always"
        ];
        default = "on-failure";
        description = ''
          systemd `Restart=` policy for the agent. `on-failure` lets an agent
          that exits cleanly after finishing its task stay down, while a crash
          (including a connect failure while the runtime is still starting up)
          is retried.
        '';
      };

      restartSec = mkOption {
        type = types.int;
        default = 3;
        description = "systemd `RestartSec=` delay (seconds) between agent restarts.";
      };
    };
  };

  config = mkMerge [
    # Stable, read-only alias so other services/agents can reference the socket
    # path without re-deriving it.
    { services.adesk.socket = cfg.socketPath; }

    (mkIf cfg.enable {
      users.groups = optionalAttrs (cfg.group != "root") {
        ${cfg.group} = { };
      };

      users.users = optionalAttrs (cfg.user != "root") {
        ${cfg.user} = {
          isSystemUser = true;
          group = cfg.group;
          description = "ADesk runtime user";
        };
      };

      systemd.services.adesk = {
        description = "ADesk — AI-native headless Wayland runtime";
        wantedBy = [ "multi-user.target" ];
        after = [ "network.target" ];
        environment =
          runtimeEnv // appsDirEnv // accessibilityEnv // recordingsDirEnv // xkbEnv // viewerEnv
          // cfg.environment;
        serviceConfig = {
          Type = "simple";
          ExecStart = escapeShellArgs (
            [ "${cfg.package}/bin/adesk-server" ]
            ++ viewerArgs
            ++ cfg.extraArgs
          );
          User = cfg.user;
          Group = cfg.group;
          # /run/<runtimeDirectory> is created, chowned to User and exposed as
          # XDG_RUNTIME_DIR; both the AGP and Wayland sockets live there.
          RuntimeDirectory = cfg.runtimeDirectory;
          RuntimeDirectoryMode = "0750";
          Restart = "on-failure";
          RestartSec = 2;
          NoNewPrivileges = true;
        };
      };

      systemd.services."adesk-agent" = mkIf cfg.agent.enable {
        description = "ADesk companion agent";
        wantedBy = [ "multi-user.target" ];
        after = [ "adesk.service" ];
        requires = [ "adesk.service" ];
        environment = {
          XDG_RUNTIME_DIR = "/run/${cfg.runtimeDirectory}";
          ADESK_SOCKET = cfg.socketPath;
        } // cfg.agent.environment;
        serviceConfig = {
          Type = "simple";
          ExecStart =
            if cfg.agent.execStart != null then
              cfg.agent.execStart
            else
              escapeShellArgs (cfg.agent.command ++ cfg.agent.extraArgs);
          User = cfg.agent.user;
          Group = cfg.agent.group;
          Restart = cfg.agent.restart;
          RestartSec = cfg.agent.restartSec;
          NoNewPrivileges = true;
        };
      };
    })
  ];
}
