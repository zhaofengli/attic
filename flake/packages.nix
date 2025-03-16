{ self
, lib
, flake-parts-lib
, inputs
, config
, makeCranePkgs
, getSystem
, ...
}:

let
  inherit (lib)
    mkOption
    types
    ;
  inherit (flake-parts-lib)
    mkPerSystemOption
    ;

  # Re-evaluate perSystem with cross nixpkgs
  # HACK before https://github.com/hercules-ci/flake-parts/issues/95 is solved
  evalCross = { system, pkgs }: config.allSystems.${system}.debug.extendModules {
    modules = [
      ({ config, lib, ... }: {
        _module.args.pkgs = pkgs;
        _module.args.self' = lib.mkForce config;
      })
    ];
  };
in
{
  options = {
    perSystem = mkPerSystemOption {
      options.attic = {
        toolchain = mkOption {
          type = types.nullOr types.package;
          default = null;
        };
        extraPackageArgs = mkOption {
          type = types.attrsOf types.anything;
          default = {};
        };
      };
    };
  };

  config = {
    _module.args.makeCranePkgs = lib.mkDefault (pkgs: let
      perSystemConfig = getSystem pkgs.system;
      craneLib = builtins.foldl' (acc: f: f acc) pkgs [
        inputs.crane.mkLib
        (craneLib:
          if perSystemConfig.attic.toolchain == null then craneLib
          else craneLib.overrideToolchain config.attic.toolchain
        )
      ];
    in pkgs.callPackage ../crane.nix {
      inherit craneLib;
      inherit (perSystemConfig.attic) extraPackageArgs;
    });

    perSystem = {
      self',
      pkgs,
      config,
      cranePkgs,
      cranePkgsStatic,
      ...
    }: (lib.mkMerge [
      {
        _module.args = {
          cranePkgs = makeCranePkgs pkgs;
          cranePkgsStatic = makeCranePkgs pkgs.pkgsStatic;
        };

        packages = {
          default = self'.packages.attic;

          inherit (cranePkgs)
            attic
            attic-client
            attic-server
          ;

          attic-lix = (cranePkgs.override {
            nix = config.attic.nix-versions.versions."lix-2.91";
          }).attic;

          attic-static = cranePkgsStatic.attic;
          attic-client-static = cranePkgsStatic.attic-client;
          attic-server-static = cranePkgsStatic.attic-server;

          attic-ci-installer = pkgs.callPackage ../ci-installer.nix {
            inherit self;
          };

          book = pkgs.callPackage ../book {
            attic = self'.packages.attic;
          };
        };
      }

      (lib.mkIf pkgs.stdenv.isLinux {
        packages = {
          attic-server-image = pkgs.dockerTools.buildImage {
            name = "attic-server";
            tag = "main";
            copyToRoot = [
              self'.packages.attic-server

              # Debugging utilities for `fly ssh console`
              pkgs.busybox

              # Now required by the fly.io sshd
              pkgs.dockerTools.fakeNss
            ];
            config = {
              Entrypoint = [ "${self'.packages.attic-server}/bin/atticd" ];
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              ];
            };
          };
        };
      })

      (lib.mkIf (pkgs.system == "x86_64-linux") {
        packages = {
          attic-server-image-aarch64 = let
            eval = evalCross {
              system = "aarch64-linux";
              pkgs = pkgs.pkgsCross.aarch64-multiplatform;
            };

          in eval.config.packages.attic-server-image;
        };
      })
    ]);
  };
}
