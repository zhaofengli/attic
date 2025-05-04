{
  lib,
  pkgs,
  config,
  ...
}:

let
  inherit (lib) types;

  cfg = config.services.atticd;

  # unused when the entrypoint is flake
  flake = import ../flake-compat.nix;
  overlay = flake.defaultNix.overlays.default;

  format = pkgs.formats.toml { };

  checkedConfigFile =
    pkgs.runCommand "checked-attic-server.toml"
      {
        configFile = cfg.configFile;
      }
      ''
        cat $configFile

        export ATTIC_SERVER_TOKEN_HS256_SECRET_BASE64="dGVzdCBzZWNyZXQ="
        export ATTIC_SERVER_DATABASE_URL="sqlite://:memory:"
        ${lib.getExe cfg.package} --mode check-config -f $configFile
        cat <$configFile >$out
      '';

  # See here for why we define credentials-related environment variables in
  # this wrapper rather than in the command executed in `atticd-atticadm`:
  # https://github.com/systemd/systemd/issues/5699
  # https://github.com/systemd/systemd/issues/34931
  #
  # NOTE that, for credentials specified with `ImportCredential`, this wrapper
  # (a) treats `cred.id` as a glob and (b) only recognizes the *first* file
  # found under `$CREDENTIALS_DIRECTORY` that matches this glob.
  atticadmShim = pkgs.writeShellScript "atticadm" ''
    if [ -n "$ATTICADM_PWD" ]; then
      cd "$ATTICADM_PWD"
      if [ "$?" != "0" ]; then
        >&2 echo "Warning: Failed to change directory to $ATTICADM_PWD"
      fi
    fi

    ${lib.concatMapStrings (cred:
      if cred.import
      then ''
        for cred in "$CREDENTIALS_DIRECTORY"/${cred.id}; do
          export ${cred.envVar}="$cred"
          break
        done
      ''
      else ''
        export ${cred.envVar}="''${CREDENTIALS_DIRECTORY?}"/${lib.escapeShellArg cred.id}
      '') credentialsList}

    exec ${cfg.package}/bin/atticadm -f ${checkedConfigFile} "$@"
  '';

  atticadmWrapper =
    let
      run = [
        "exec"
        "systemd-run"
        "--quiet"
        "--pipe"
        "--pty"
        "--wait"
        "--collect"
        "--service-type=exec"
        "--property=DynamicUser=yes"
        "--property=User=${cfg.user}"
        "--working-directory=/"
      ] ++ (lib.optional haveEnvironmentFile "--property=EnvironmentFile=${cfg.environmentFile}")
      ++ (map (cred: "--property=${cred.setting}=${cred.spec}") credentialsList);
    in
    pkgs.writeShellScriptBin "atticd-atticadm" ''
      ${lib.escapeShellArgs run} --property=Environment=ATTICADM_PWD=$(pwd) -- ${atticadmShim} "$@"
    '';

  credentialsList = builtins.attrValues cfg.credentials;
  credentialsSettings = builtins.foldl'
    (acc: cred: acc // {
      ${cred.setting} = (acc.${cred.setting} or [ ]) ++ [ cred.spec ];
    })
    { }
    credentialsList;

  haveEnvironmentFile = cfg.environmentFile != null;

  hasLocalPostgresDB =
    let
      url = cfg.settings.database.url or "";
      localStrings = [
        "localhost"
        "127.0.0.1"
        "/run/postgresql"
      ];
      hasLocalStrings = lib.any (lib.flip lib.hasInfix url) localStrings;
    in
    config.services.postgresql.enable && lib.hasPrefix "postgresql://" url && hasLocalStrings;

  toEnvVar = lib.flip lib.pipe [
    (lib.replaceStrings [ "-" ] [ "_" ])
    lib.toUpper
  ];

  toAtticEnvVar = name: "ATTIC_${toEnvVar name}";
in
{
  imports = [
    (lib.mkRenamedOptionModule [ "services" "atticd" "credentialsFile" ] [ "services" "atticd" "environmentFile" ])
  ];

  options = {
    services.atticd = {
      enable = lib.mkEnableOption "the atticd, the Nix Binary Cache server";

      package = lib.mkPackageOption pkgs "attic-server" { };

      environmentFile = lib.mkOption {
        description = ''
          Path to an EnvironmentFile containing required environment
          variables:

          - ATTIC_SERVER_TOKEN_RS256_SECRET_BASE64: The base64-encoded RSA PEM PKCS1 of the
            RS256 JWT secret. Generate it with `openssl genrsa -traditional 4096 | base64 -w0`.
        '';
        type = types.nullOr types.path;
        default = null;
      };

      credentials = lib.mkOption {
        description = ''
          Attribute set naming files to provide to the `atticd` service via
          systemd's `LoadCredential` option.

          See {manpage}`systemd.exec(5)`.
        '';

        default = { };

        type =
          let
            credentialModule = types.submodule ({ name, config, ... }: {
              options = {
                name = lib.mkOption {
                  description = ''
                '';
                  type = types.nonEmptyStr;
                  default = name;
                };

                # Read-only to avoid collisions: this way, there is a one-to-one
                # mapping between entries in the `credentials` attrset and their
                # paths under `$CREDENTIALS_DIRECTORY`.
                id = lib.mkOption {
                  description = ''
                    ID of the credential.  The credential will be located at
                    `$CREDENTIALS_DIRECTORY/<id>`.

                    Also used for constructing the default value of
                    {option}`envVar`.
                  '';
                  type = types.nonEmptyStr;
                  default = name;
                  readOnly = true;
                };

                value = lib.mkOption {
                  description = ''
                    If {option}`set` is enabled, then this is the value of the
                    credential; otherwise, it is the path to the file containing
                    the credential.

                    If set to `null` (the default), then it is assumed that this
                    credential is a so-called "system credential" (a credential
                    already loaded into the system manager, possibly with
                    {command}`systemd-creds`), or a file to be loaded from the
                    locations specified in {manpage}`systemd.exec(5)`'s
                    documentation of `LoadCredential` and
                    `LoadCredentialEncrypted`.
                  '';
                  default = null;
                  type = types.nullOr (types.oneOf [ types.path types.str ]);
                };

                encrypted = lib.mkOption {
                  description = ''
                    Set this to `true` if the credential should be loaded with
                    `SetCredentialEncrypted` rather than `SetCredential` (when
                    {option}`set` is enabled), or with `LoadCredentialEncrypted`
                    rather than `LoadCredential`.
                  '';
                  type = types.bool;
                  default = false;
                };

                import = lib.mkOption {
                  description = ''
                    Set this to `true` if the credential should be imported with
                    `ImportCredential` (see {manpage}`systemd.exec(5)`).
                  '';
                  type = types.bool;
                  default = false;
                };

                set = lib.mkOption {
                  description = ''
                    Set this to `true` if the credential should be loaded with
                    `SetCredential` or `SetCredentialEncrypted` (see
                    {manpage}`systemd.exec(5)`).
                  '';
                  type = types.bool;
                  default = false;
                };

                envVar = lib.mkOption {
                  description = ''
                    Set the value of this environment variable to the path of the
                    credential file within the `atticd` systemd service unit.
                  '';
                  type = types.strMatching "(_[[:alnum:]]|[A-Za-z])[_[:alnum:]]*";
                  default =
                    let
                      base = toAtticEnvVar config.id;
                    in
                    if config.import then lib.removeSuffix "*" base else base;
                  defaultText = ''"ATTIC_''${lib.toUpper (lib.replaceStrings ["-"] ["_"] credential.id)}"'';
                };

                setting = lib.mkOption {
                  description = ''
                    systemd setting for use with this credential.
                  '';
                  type = types.nonEmptyStr;
                  default =
                    if config.import
                    then "ImportCredential"
                    else "${if config.set then "Set" else "Load"}Credential${lib.optionalString config.encrypted "Encrypted"}";
                  readOnly = true;
                  internal = true;
                };

                spec = lib.mkOption {
                  description = ''
                    Representation of this credential for use with
                    `LoadCredential` or `LoadCredentialEncrypted`.
                  '';
                  type = types.str;
                  default = "${config.id}${lib.optionalString (config.value != null) ":${config.value}"}";
                  readOnly = true;
                  internal = true;
                };
              };
            });

            credentialType = types.coercedTo types.path (value: { inherit value; }) credentialModule;
          in
          types.attrsOf credentialType;
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
        default = { }; # setting defaults here does not compose well
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

      mode = lib.mkOption {
        description = ''
          Mode in which to run the server.

          'monolithic' runs all components, and is suitable for single-node deployments.

          'api-server' runs only the API server, and is suitable for clustering.

          'garbage-collector' only runs the garbage collector periodically.

          A simple NixOS-based Attic deployment will typically have one 'monolithic' and any number of 'api-server' nodes.

          There are several other supported modes that perform one-off operations, but these are the only ones that make sense to run via the NixOS module.
        '';
        type = lib.types.enum [
          "monolithic"
          "api-server"
          "garbage-collector"
        ];
        default = "monolithic";
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

  config = lib.mkIf cfg.enable {
    assertions =
      let
        keyEnvVars = map (key: toAtticEnvVar "${key}-base64-file") [
          "server-token-hs256-secret"
          "server-token-rs256-secret"
          "server-token-rs256-public"
        ];
        haveCredentials = lib.any (lib.flip lib.elem keyEnvVars) (lib.catAttrs "envVar" credentialsList);
      in
      [
        {
          assertion = haveEnvironmentFile || haveCredentials;
          message = ''
            `services.atticd.environmentFile` is not set, and no entry in `service.atticd.credentials` defines any of these environment variables: ${lib.concatMapStringsSep ", " (var: "`${var}`") keyEnvVars}.

            Run `openssl genrsa -traditional -out private_key.pem 4096 | base64 -w0` and create a file with the following contents:

            ATTIC_SERVER_TOKEN_RS256_SECRET="output from command"

            Then, set `services.atticd.environmentFile` to the quoted absolute path of the file.
          '';
        }
        {
          assertion = (cfg.environmentFile != null) -> (!lib.isStorePath cfg.environmentFile);
          message = ''
            <option>services.atticd.environmentFile</option> points to a path in the Nix store. The Nix store is globally readable.

            You should use a quoted absolute path to prevent leaking secrets in the Nix store.
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
      after = [ "network-online.target" ] ++ lib.optionals hasLocalPostgresDB [ "postgresql.service" ];
      requires = lib.optionals hasLocalPostgresDB [ "postgresql.service" ];
      wants = [ "network-online.target" ];

      environment = lib.mapAttrs' (_: cred: { name = cred.envVar; value = "%d/${cred.id}"; }) cfg.credentials;

      serviceConfig = {
        ExecStart = "${lib.getExe cfg.package} -f ${checkedConfigFile} --mode ${cfg.mode}";
        StateDirectory = "atticd"; # for usage with local storage and sqlite
        DynamicUser = true;
        User = cfg.user;
        Group = cfg.group;
        Restart = "on-failure";
        RestartSec = 10;

        CapabilityBoundingSet = [ "" ];
        DeviceAllow = "";
        DevicePolicy = "closed";
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateTmp = true;
        PrivateUsers = true;
        ProcSubset = "pid";
        ProtectClock = true;
        ProtectControlGroups = true;
        ProtectHome = true;
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        ProtectProc = "invisible";
        ProtectSystem = "strict";
        ReadWritePaths =
          let
            path = cfg.settings.storage.path;
            isDefaultStateDirectory = path == "/var/lib/atticd" || lib.hasPrefix "/var/lib/atticd/" path;
          in
          lib.optionals (cfg.settings.storage.type or "" == "local" && !isDefaultStateDirectory) [ path ];
        RemoveIPC = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [
          "@system-service"
          "~@resources"
          "~@privileged"
        ];
        UMask = "0077";
      }
      // credentialsSettings
      // lib.optionalAttrs haveEnvironmentFile {
        EnvironmentFile = cfg.environmentFile;
      };
    };

    environment.systemPackages = [
      atticadmWrapper
    ];

    nixpkgs.overlays = lib.mkIf cfg.useFlakeCompatOverlay [
      overlay
    ];
  };
}
