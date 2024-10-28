{ lib, flake-parts-lib, inputs, self, ... }:
let
  inherit (lib)
    mkOption
    types
    ;
  inherit (flake-parts-lib)
    mkPerSystemOption
    ;
in
{
  options = {
    perSystem = mkPerSystemOption {
      options.attic.integration-tests = {
        nixpkgsArgs = mkOption {
          type = types.attrsOf types.anything;
          default = {};
        };
        tests = mkOption {
          type = types.attrsOf types.package;
          default = {};
        };
        stableTests = mkOption {
          type = types.attrsOf types.package;
          default = {};
        };
      };
    };
  };

  config = {
    flake.githubActions = inputs.nix-github-actions.lib.mkGithubMatrix {
      checks = {
        inherit (self.checks) x86_64-linux;
      };
    };

    perSystem = { self', pkgs, config, system, ... }: let
      cfg = config.attic.integration-tests;

      vmPkgs = import inputs.nixpkgs ({
        inherit system;
        overlays = [ self.overlays.default ];
      } // cfg.nixpkgsArgs);
      vmPkgsStable = import inputs.nixpkgs-stable ({
        inherit system;
        overlays = [ self.overlays.default ];
      } // cfg.nixpkgsArgs);

      makeIntegrationTests = pkgs: import ../integration-tests {
        inherit pkgs;
        flake = self;
      };
    in {
      attic.integration-tests = {
        tests = makeIntegrationTests vmPkgs;
        stableTests = makeIntegrationTests vmPkgsStable;
      };

      checks = let
        tests = cfg.tests;
        stableTests = lib.mapAttrs' (name: lib.nameValuePair "stable-${name}") cfg.stableTests;
      in lib.optionalAttrs pkgs.stdenv.isLinux (tests // stableTests);
    };
  };
}
