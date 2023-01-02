{
  description = "A Nix binary cache server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-compat.follows = "flake-compat";
      inputs.flake-utils.follows = "flake-utils";
    };

    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, crane, ... }: let
    supportedSystems = flake-utils.lib.defaultSystems;
  in flake-utils.lib.eachSystem supportedSystems (system: let
    pkgs = import nixpkgs { inherit system; };

    craneLib = crane.lib.${system};
    cranePkgs = pkgs.callPackage ./crane.nix { inherit craneLib; };

    inherit (pkgs) lib;
  in rec {
    packages = {
      default = packages.attic;

      inherit (cranePkgs) attic attic-client attic-server attic-tests;

      attic-nixpkgs = pkgs.callPackage ./package.nix { };

      attic-server-image = pkgs.dockerTools.buildImage {
        name = "attic-server";
        tag = "main";
        copyToRoot = [
          # Minimal `/bin/sh` for `fly ssh console`
          pkgs.busybox-sandbox-shell
          packages.attic-server
        ];
        config = {
          Entrypoint = [ "${packages.attic-server}/bin/atticd" ];
          Cmd = [ "--mode" "api-server" ];
          Env = [
            "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
          ];
        };
      };

      book = pkgs.callPackage ./book {
        attic = packages.attic;
      };
    };

    devShells = {
      default = pkgs.mkShell {
        inputsFrom = with packages; [ attic book ];
        nativeBuildInputs = with pkgs; [
          rustc

          rustfmt clippy
          cargo-expand cargo-outdated cargo-edit

          sqlite-interactive

          editorconfig-checker

          flyctl
        ] ++ (lib.optionals pkgs.stdenv.isLinux [
          linuxPackages.perf
        ]);

        NIX_PATH = "nixpkgs=${pkgs.path}";
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustcSrc}/library";

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
  });
}
