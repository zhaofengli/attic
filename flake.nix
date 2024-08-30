{
  description = "A Nix binary cache server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    nixpkgs-stable.url = "github:NixOS/nixpkgs/nixos-24.05";
    flake-utils.url = "github:numtide/flake-utils";

    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = inputs @ { self, flake-parts, ... }: let
    supportedSystems = inputs.flake-utils.lib.defaultSystems ++ [ "riscv64-linux" ];

    inherit (inputs.nixpkgs) lib;

    makeCranePkgs = pkgs: let
      craneLib = inputs.crane.mkLib pkgs;
    in pkgs.callPackage ./crane.nix { inherit craneLib; };

    modules = builtins.foldl' (acc: f: f acc) ./flake [
      builtins.readDir
      (lib.filterAttrs (name: type:
        type == "regular" && lib.hasSuffix ".nix" name
      ))
      (lib.mapAttrsToList (name: _:
        lib.path.append ./flake name
      ))
    ];

  in flake-parts.lib.mkFlake { inherit inputs; } {
    imports = modules;
    systems = supportedSystems;

    debug = true;

    # old flake
    flake = inputs.flake-utils.lib.eachSystem supportedSystems (system: let
      pkgs = import inputs.nixpkgs {
        inherit system;
        overlays = [];
      };
      cranePkgs = makeCranePkgs pkgs;

      internalMatrix = lib.mapAttrs (_: nix: let
        cranePkgs' = cranePkgs.override { inherit nix; };
      in {
        inherit (cranePkgs') attic-tests cargoArtifacts;
      }) {
        "2.20" = pkgs.nixVersions.nix_2_20;
        "2.24" = pkgs.nixVersions.nix_2_24;
        "default" = pkgs.nix;
      };

      pkgsStable = import inputs.nixpkgs-stable {
        inherit system;
        overlays = [];
      };
      cranePkgsStable = makeCranePkgs pkgsStable;

      inherit (pkgs) lib;
    in rec {
      inherit internalMatrix;

      packages = {
        default = packages.attic;

        inherit (cranePkgs) attic attic-client attic-server;

        attic-nixpkgs = pkgs.callPackage ./package.nix { };

        attic-ci-installer = pkgs.callPackage ./ci-installer.nix {
          inherit self;
        };

        book = pkgs.callPackage ./book {
          attic = packages.attic;
        };
      } // (lib.optionalAttrs (system != "x86_64-darwin") {
        # Unfortunately, x86_64-darwin fails to evaluate static builds
        # TODO: Make this work with Crane
        attic-static = (pkgs.pkgsStatic.callPackage ./package.nix {
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

        attic-client-static = packages.attic-static.override {
          clientOnly = true;
        };
      }) // (lib.optionalAttrs pkgs.stdenv.isLinux {
        attic-server-image = pkgs.dockerTools.buildImage {
          name = "attic-server";
          tag = "main";
          copyToRoot = [
            # Debugging utilities for `fly ssh console`
            pkgs.busybox
            packages.attic-server

            # Now required by the fly.io sshd
            pkgs.dockerTools.fakeNss
          ];
          config = {
            Entrypoint = [ "${packages.attic-server}/bin/atticd" ];
            Env = [
              "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
            ];
          };
        };
      });

      checks = let
        makeIntegrationTests = pkgs: import ./integration-tests {
          pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ self.overlays.default ];
          };
          flake = self;
        };
        unstableTests = makeIntegrationTests pkgs;
        stableTests = lib.mapAttrs' (name: lib.nameValuePair "stable-${name}") (makeIntegrationTests pkgsStable);
      in lib.optionalAttrs pkgs.stdenv.isLinux (unstableTests // stableTests);
    }) // {
      overlays = {
        default = final: prev: let
          cranePkgs = makeCranePkgs final;
        in {
          inherit (cranePkgs) attic attic-client attic-server;
        };
      };

      nixosModules = {
        atticd = {
          imports = [
            ./nixos/atticd.nix
          ];

          services.atticd.useFlakeCompatOverlay = false;

          nixpkgs.overlays = [
            self.overlays.default
          ];
        };
      };
    };
  };
}
