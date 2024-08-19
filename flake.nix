{
  description = "A Nix binary cache server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    nixpkgs-stable.url = "github:NixOS/nixpkgs/nixos-23.11";
    flake-utils.url = "github:numtide/flake-utils";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, nixpkgs-stable, flake-utils, crane, ... }: let
    supportedSystems = flake-utils.lib.defaultSystems ++ [ "riscv64-linux" ];

    makeCranePkgs = pkgs: let
      craneLib = crane.mkLib pkgs;
    in pkgs.callPackage ./crane.nix { inherit craneLib; };
  in flake-utils.lib.eachSystem supportedSystems (system: let
    pkgs = import nixpkgs {
      inherit system;
      overlays = [];
    };
    cranePkgs = makeCranePkgs pkgs;

    pkgsStable = import nixpkgs-stable {
      inherit system;
      overlays = [];
    };
    cranePkgsStable = makeCranePkgs pkgsStable;

    inherit (pkgs) lib;
  in rec {
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

    devShells = {
      default = pkgs.mkShell {
        inputsFrom = with packages; [ attic book ];
        nativeBuildInputs = with pkgs; [
          rustc

          rustfmt clippy
          cargo-expand
          # Temporary broken: https://github.com/NixOS/nixpkgs/pull/335152
          # cargo-outdated
          cargo-edit
          tokio-console

          sqlite-interactive

          editorconfig-checker

          flyctl

          wrk
        ] ++ (lib.optionals pkgs.stdenv.isLinux [
          linuxPackages.perf
        ]);

        NIX_PATH = "nixpkgs=${pkgs.path}";
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustcSrc}/library";

        # See comment in `attic/build.rs`
        NIX_INCLUDE_PATH = "${lib.getDev pkgs.nixVersions.nix_2_24}/include";

        ATTIC_DISTRIBUTOR = "dev";
      };

      demo = pkgs.mkShell {
        nativeBuildInputs = [
          packages.default
        ];

        shellHook = ''
          >&2 echo
          >&2 echo '🚀 Run `atticd` to get started!'
          >&2 echo
        '';
      };
    };
    devShell = devShells.default;

    internal = {
      inherit (cranePkgs) attic-tests cargoArtifacts;
    };

    checks = let
      makeIntegrationTests = pkgs: import ./integration-tests {
        pkgs = import nixpkgs {
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
}
