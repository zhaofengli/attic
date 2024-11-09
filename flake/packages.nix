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

    perSystem = { self', pkgs, config, cranePkgs, ... }: let
      inherit (pkgs) pkgsStatic;
      inherit (pkgs.stdenv.hostPlatform) isDarwin;

      nix-static = pkgsStatic.nixVersions.nix_2_18.overrideAttrs (old: {
        buildInputs = lib.optionals isDarwin [
          # HACK for Darwin/macOS: Remove dependency on non-existent iconv.pc
          (pkgs.runCommand "libarchive-pkg-config-hack" {} ''
            mkdir -p $out/lib/pkgconfig
            cat "${pkgsStatic.libarchive.dev}/lib/pkgconfig/libarchive.pc" >$out/lib/pkgconfig/libarchive.pc
            sed -i '/Requires.private: iconv/d' $out/lib/pkgconfig/libarchive.pc
          '')
        ] ++ (old.buildInputs or []);

        patches = (old.patches or []) ++ [
          # Diff: https://github.com/zhaofengli/nix/compare/501a805fcd4a90e2bc112e9547417cfc4e04ca66...1dbe9899a8acb695f5f08197f1ff51c14bcc7f42
          (pkgs.fetchpatch {
            url = "https://github.com/zhaofengli/nix/compare/501a805fcd4a90e2bc112e9547417cfc4e04ca66...1dbe9899a8acb695f5f08197f1ff51c14bcc7f42.diff";
            hash = "sha256-bxBZDUUNTBUz6F4pwxx1ZnPcOKG3EhV+kDBt8BrFh6k=";
          })
        ];

        NIX_LDFLAGS = (old.NIX_LDFLAGS or "")
          + lib.optionalString isDarwin " -framework CoreFoundation -framework SystemConfiguration";

        preInstallCheck = (old.preInstallCheck or "")
          # FIXME
          + lib.optionalString isDarwin ''
            echo "exit 99" >tests/functional/gc-non-blocking.sh
          '';
      });
    in (lib.mkMerge [
      {
        _module.args.cranePkgs = makeCranePkgs pkgs;

        packages = {
          default = self'.packages.attic;

          inherit (cranePkgs)
            attic
            attic-client
            attic-server
          ;

          inherit nix-static;

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

      # Unfortunately, x86_64-darwin fails to evaluate static builds
      (lib.mkIf (pkgs.system != "x86_64-darwin") {
        packages = {
          # TODO: Make this work with Crane
          attic-static = (pkgsStatic.callPackage ../package.nix {
            nix = nix-static;
          }).overrideAttrs (old: {
            nativeBuildInputs = (old.nativeBuildInputs or []) ++ [
              pkgs.nukeReferences
            ];

            # Read by pkg_config crate (do some autodetection in build.rs?)
            PKG_CONFIG_ALL_STATIC = "1";

            "NIX_CFLAGS_LINK_${pkgsStatic.stdenv.cc.suffixSalt}" = "-lc";
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
