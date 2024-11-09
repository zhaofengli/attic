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

      # HACK to enable almost-static builds on macOS/Darwin
      #
      # Currently on macOS, pkgsStatic uses the same Rust target
      # as the regular platform (e.g., aarch64-apple-darwin).
      #
      # In `pkgs/build-support/rust/lib/default.nix`, the following
      # environment variables are forced:
      #
      # - CC_AARCH64_APPLE_DARWIN=${ccForHost} (static cc)
      # - CXX_AARCH64_APPLE_DARWIN=${cxxForHost} (static cxx)
      # - CC_AARCH64_APPLE_DARWIN=${ccForBuild} (regular cc - incorrect)
      # - CXX_AARCH64_APPLE_DARWIN=${cxxForBuild} (regular cxx - incorrect)
      #
      # As a result, the correct static CC/CXX variables are clobbered.
      darwinStaticRustPlatform = let
        inherit (pkgsStatic) stdenv rustPlatform;

        stdenv' = stdenv.override {
          buildPlatform = stdenv.buildPlatform // {
            rust = stdenv.buildPlatform.rust // {
              cargoEnvVarTarget = stdenv.hostPlatform.rust.cargoEnvVarTarget + "_BUILD";
            };
          };
        };

        rustLib' = import (pkgs.path + "/pkgs/build-support/rust/lib") {
          stdenv = stdenv';

          inherit (pkgsStatic)
            lib pkgsBuildHost pkgsBuildTarget pkgsTargetTarget;
        };

        rust' = pkgsStatic.rust // {
          inherit (rustLib') envVars;
        };

        hooks' = pkgsStatic.buildPackages.callPackages (pkgs.path + "/pkgs/build-support/rust/hooks") {
          rust = rust';
        };

        rustPlatform' = rustPlatform // hooks' // {
          buildRustPackage = rustPlatform.buildRustPackage.override {
            inherit (hooks')
              cargoBuildHook cargoCheckHook cargoInstallHook cargoNextestHook cargoSetupHook;
          };
        };
      in rustPlatform';
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

      # Static builds
      {
        packages = {
          # TODO: Make this work with Crane
          attic-static = let
            inputOverrides = {
              nix = nix-static;
            } // lib.optionalAttrs isDarwin {
              rustPlatform = darwinStaticRustPlatform;
            };
          in (pkgsStatic.callPackage ../package.nix inputOverrides).overrideAttrs (old: {
            nativeBuildInputs = (old.nativeBuildInputs or []) ++ [
              pkgs.nukeReferences
            ];

            preBuild = (old.preBuild or "")
              # HACK for Darwin/macOS:
              #
              # The other half of the mess (see darwinStaticRustPlatform above).
              # Logic of cc-rs:
              # - getenv_with_target_prefixes()
              #   - get_is_cross_compile() -> false
              #   - therefore, kind = "HOST"
              #   - tries "$CXX_aarch64_apple_darwin" (lowercase) -> not set
              #   - tries "$HOST_CXX" -> set to the incorrect CXX
              + lib.optionalString isDarwin ''
                export CXX_${lib.toLower pkgsStatic.stdenv.hostPlatform.rust.cargoEnvVarTarget}="$CXX"
              '';

            # Read by pkg_config crate (do some autodetection in build.rs?)
            PKG_CONFIG_ALL_STATIC = "1";

            "NIX_CFLAGS_LINK_${pkgsStatic.stdenv.cc.suffixSalt}" = "-lc";
            RUSTFLAGS = "-C relocation-model=static";

            NIX_LDFLAGS = (old.NIX_LDFLAGS or "")
              + lib.optionalString isDarwin " -framework CoreFoundation -framework SystemConfiguration";

            postFixup = (old.postFixup or "") + ''
              rm -f $out/nix-support/propagated-build-inputs
              nuke-refs $out/bin/attic
            '';
          });

          attic-client-static = self'.packages.attic-static.override {
            clientOnly = true;
          };
        };
      }
    ]);
  };
}
