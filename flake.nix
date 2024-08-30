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
