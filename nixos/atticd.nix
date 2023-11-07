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

    export ATTIC_SERVER_TOKEN_RS256_SECRET="$(${pkgs.openssl}/bin/openssl genrsa -traditional -out - 1024 | ${pkgs.coreutils}/bin/base64 -w0)"
    export ATTIC_SERVER_DATABASE_URL="sqlite://:memory:"
    ${cfg.package}/bin/atticd --mode check-config -f $configFile
    cat <$configFile >$out
  '';

  atticadmShim = pkgs.writeShellScript "atticadm" ''
    if [ -n "$ATTICADM_PWD" ]; then
      cd "$ATTICADM_PWD"
      if [ "$?" != "0" ]; then
        >&2 echo "Warning: Failed to change directory to $ATTICADM_PWD"
      fi
    fi

    exec ${cfg.package}/bin/atticadm -f ${checkedConfigFile} "$@"
  '';

  atticadmWrapper = pkgs.writeShellScriptBin "atticd-atticadm" ''
    exec systemd-run \
      --quiet \
      --pty \
      --same-dir \
      --wait \
      --collect \
      --service-type=exec \
      --property=EnvironmentFile=${cfg.credentialsFile} \
      --property=DynamicUser=yes \
      --property=User=${cfg.user} \
      --property=Environment=ATTICADM_PWD=$(pwd) \
      --working-directory / \
      -- \
      ${atticadmShim} "$@"
  '';

  hasLocalPostgresDB = let
    url = cfg.settings.database.url or "";
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

          - ATTIC_SERVER_TOKEN_RS256_SECRET: The PEM-encoded version of the
            RS256 JWT secret. Generate it with `openssl genrsa -traditional -out - 4096 | base64 -w0`.
        '';
        type = types.nullOr types.path;
        default = null;
      };
      user = lib.mkOption {
        description = ''
          The group under which attic runs.
        '';
        type = types.str;
        default = "atticd";
      };
      group = lib.mkOption {
        description = ''
          The user under which attic runs.
        '';
        type = types.str;
        default = "atticd";
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
          assertion = cfg.credentialsFile != null;
          message = ''
            <option>services.atticd.credentialsFile</option> is not set.

            Run `openssl genrsa -traditional -out private_key.pem 4096 | base64 -w0` and create a file with the following contents:

            ATTIC_SERVER_TOKEN_RS256_SECRET="output from command"

            Then, set `services.atticd.credentialsFile` to the quoted absolute path of the file.
          '';
        }
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
        after = [ "network.target" ]
          ++ lib.optionals hasLocalPostgresDB [ "postgresql.service" "nss-lookup.target" ];
        serviceConfig = {
          ExecStart = "${cfg.package}/bin/atticd -f ${checkedConfigFile}";
          EnvironmentFile = cfg.credentialsFile;
          StateDirectory = "atticd"; # for usage with local storage and sqlite
          DynamicUser = true;
          User = cfg.user;
          Group = cfg.group;
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

      environment.systemPackages = [ atticadmWrapper ];
    }
    (lib.mkIf cfg.useFlakeCompatOverlay {
      nixpkgs.overlays = [ overlay ];
    })
  ]);
}
