{
  description = "A Nix binary cache server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";

    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, utils, ... }: let
    supportedSystems = utils.lib.defaultSystems;
  in utils.lib.eachSystem supportedSystems (system: let
    pkgs = import nixpkgs { inherit system; };

    inherit (pkgs) lib;
  in rec {
    packages = {
      default = packages.attic;

      attic = pkgs.callPackage ./package.nix { };
      attic-client = packages.attic.override { clientOnly = true; };

      attic-server = let
        attic-server = pkgs.callPackage ./package.nix {
          crates = [ "attic-server" ];
        };
      in attic-server.overrideAttrs (old: {
        pname = "attic-server";

        CARGO_PROFILE_RELEASE_LTO = "fat";
        CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "1";
      });

      attic-server-image = pkgs.dockerTools.buildImage {
        name = "attic-server";
        tag = "main";
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
