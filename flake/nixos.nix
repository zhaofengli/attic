{ config, ... }:
{
  flake.nixosModules = {
    atticd = {
      imports = [
        ../nixos/atticd.nix
      ];

      services.atticd.useFlakeCompatOverlay = false;

      nixpkgs.overlays = [
        config.flake.overlays.default
      ];
    };
  };
}
