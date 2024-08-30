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
  };
}
