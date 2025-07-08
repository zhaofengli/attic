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

        extraBaseModules = {
          # Temporary workaround for <https://github.com/zhaofengli/colmena/issues/281>
          # References:
          # - LKML discussion: <https://lore.kernel.org/all/w5ap2zcsatkx4dmakrkjmaexwh3mnmgc5vhavb2miaj6grrzat@7kzr5vlsrmh5/>
          # - Lix discussion: <https://matrix.to/#/!lymvtcwDJ7ZA9Npq:lix.systems/$wLqRlm7-iNmrkN2Tcn--Tmi92id4wgvKC5APwiEYYgw?via=lix.systems&via=matrix.org>
          # - Proposed fix: <https://lkml.org/lkml/2024/10/21/1621>
          boot.kernelPackages = pkgs.linuxPackages_6_6;
        };
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
