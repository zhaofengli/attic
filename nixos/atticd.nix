{ lib, pkgs, config, ... }:

let
  inherit (lib) types;

  cfg = config.services.atticd;

  # unused when the entrypoint is flake
  flake = import ../flake-compat.nix;
  overlay = flake.defaultNix.overlays.default;

  format = pkgs.formats.toml { };

  checkedConfigFile = pkgs.runCommand "checked-attic-server.toml" {
    configFile = cfg.configFile;
  } ''
    cat $configFile

    export ATTIC_SERVER_TOKEN_HS256_SECRET_BASE64="dGVzdCBzZWNyZXQ="
    ${cfg.package}/bin/atticd --mode check-config -f $configFile
    cat <$configFile >$out
  '';

  hasLocalPostgresDB = let
    url = cfg.settings.database.url;
    localStrings = [ "localhost" "127.0.0.1" "/run/postgresql" ];
    hasLocalStrings = lib.any (lib.flip lib.hasInfix url) localStrings;
  in config.services.postgresql.enable && lib.hasPrefix "postgresql://" url && hasLocalStrings;
in
{
  options = {
    services.atticd = {
      enable = lib.mkOption {
        description = ''
          Whether to enable the atticd, the Nix Binary Cache server.
        '';
        type = types.bool;
        default = false;
      };
      package = lib.mkOption {
        description = ''
          The package to use.
        '';
        type = types.package;
        default = pkgs.attic-server;
      };
      credentialsFile = lib.mkOption {
        description = ''
          Path to an EnvironmentFile containing required environment
          variables:

          - ATTIC_SERVER_TOKEN_HS256_SECRET_BASE64: The Base64-encoded version of the
            HS256 JWT secret.
        '';
        type = types.path;
      };
      settings = lib.mkOption {
        description = ''
          Structured configurations of atticd.
        '';
        type = format.type;
        default = {}; # setting defaults here does not compose well
      };
      configFile = lib.mkOption {
        description = ''
          Path to an existing atticd configuration file.

          By default, it's generated from `services.atticd.settings`.
        '';
        type = types.path;
        default = format.generate "server.toml" cfg.settings;
        defaultText = "generated from `services.atticd.settings`";
      };

      # Internal flags
      useFlakeCompatOverlay = lib.mkOption {
        description = ''
          Whether to insert the overlay with flake-compat.
        '';
        type = types.bool;
        internal = true;
        default = true;
      };
    };
  };
  config = lib.mkIf (cfg.enable) (lib.mkMerge [
    {
      assertions = [
        {
          assertion = !lib.isStorePath cfg.credentialsFile;
          message = ''
            <option>services.atticd.credentialsFile</option> points to a path in the Nix store. The Nix store is globally readable.

            You should use a quoted absolute path to prevent this.
          '';
        }
      ];

      services.atticd.settings = {
        database.url = lib.mkDefault "sqlite:///var/lib/atticd/server.db?mode=rwc";

        # "storage" is internally tagged
        # if the user sets something the whole thing must be replaced
        storage = lib.mkDefault {
          type = "local";
          path = "/var/lib/atticd/storage";
        };
      };

      systemd.services.atticd = {
        wantedBy = [ "multi-user.target" ];
        after = [ "network.target" ] ++ lib.optional hasLocalPostgresDB "postgresql.service";
        serviceConfig = {
          ExecStart = "${cfg.package}/bin/atticd -f ${checkedConfigFile}";
          EnvironmentFile = cfg.credentialsFile;
          StateDirectory = "atticd"; # for usage with local storage and sqlite
          DynamicUser = true;
          ProtectHome = true;
          ProtectHostname = true;
          ProtectKernelLogs = true;
          ProtectKernelModules = true;
          ProtectKernelTunables = true;
          ProtectProc = "invisible";
          ProtectSystem = "strict";
          RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
          RestrictNamespaces = true;
          RestrictRealtime = true;
          RestrictSUIDSGID = true;
        };
      };

      environment.systemPackages = [ cfg.package ];
    }
    (lib.mkIf cfg.useFlakeCompatOverlay {
      nixpkgs.overlays = [ overlay ];
    })
  ]);
}
