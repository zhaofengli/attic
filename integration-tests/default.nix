{ pkgs ? import ./nixpkgs.nix
, flake ? (import ../flake-compat.nix).defaultNix
}:

let
  inherit (pkgs) lib;

  nixosLib = import (pkgs.path + "/nixos/lib") { };

  runTest = module: (nixosLib.evalTest ({ config, ... }: {
    imports = [
      module
      {
        hostPkgs = pkgs;
        _module.args.flake = flake;
      }
    ];
    result = config.test;
  })).config.result;

  basicTests = let
    matrix = {
      database = [ "sqlite" "postgres" ];
      storage = [ "local" "minio" ];
    };
  in builtins.listToAttrs (map (e: {
    name = "basic-${e.database}-${e.storage}";
    value = runTest {
      imports = [
        ./basic
        {
          inherit (e) database storage;
        }
      ];
    };
  }) (lib.cartesianProduct matrix));
in {
} // basicTests
