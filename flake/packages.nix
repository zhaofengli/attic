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
in
{
  options = {
    perSystem = mkPerSystemOption {
      options.attic = {
        toolchain = mkOption {
          type = types.nullOr types.package;
          default = null;
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
    });

    perSystem = { self', pkgs, config, cranePkgs, ... }: (lib.mkMerge [
      {
        _module.args.cranePkgs = makeCranePkgs pkgs;

        packages = {
          default = self'.packages.attic;

          inherit (cranePkgs)
            attic
            attic-client
            attic-server
          ;

          attic-nixpkgs = pkgs.callPackage ../package.nix { };

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

      # Unfortunately, x86_64-darwin fails to evaluate static builds
      (lib.mkIf (pkgs.system != "x86_64-darwin") {
        packages = {
          # TODO: Make this work with Crane
          attic-static = (pkgs.pkgsStatic.callPackage ../package.nix {
            nix = pkgs.pkgsStatic.nix.overrideAttrs (old: {
              patches = (old.patches or []) ++ [
                # To be submitted
                (pkgs.fetchpatch {
                  url = "https://github.com/NixOS/nix/compare/3172c51baff5c81362fcdafa2e28773c2949c660...6b09a02536d5946458b537dfc36b7d268c9ce823.diff";
                  hash = "sha256-LFLq++J2XitEWQ0o57ihuuUlYk2PgUr11h7mMMAEe3c=";
                })
              ];
            });
          }).overrideAttrs (old: {
            nativeBuildInputs = (old.nativeBuildInputs or []) ++ [
              pkgs.nukeReferences
            ];

            # Read by pkg_config crate (do some autodetection in build.rs?)
            PKG_CONFIG_ALL_STATIC = "1";

            "NIX_CFLAGS_LINK_${pkgs.pkgsStatic.stdenv.cc.suffixSalt}" = "-lc";
            RUSTFLAGS = "-C relocation-model=static";

            postFixup = (old.postFixup or "") + ''
              rm -f $out/nix-support/propagated-build-inputs
              nuke-refs $out/bin/attic
            '';
          });

          attic-client-static = self'.packages.attic-static.override {
            clientOnly = true;
          };
        };
      })
    ]);
  };
}
