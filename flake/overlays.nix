{ makeCranePkgs, ... }:
{
  flake.overlays = {
    default = final: prev: let
      cranePkgs = makeCranePkgs final;
    in {
      inherit (cranePkgs)
        attic
        attic-client
        attic-server
        ;
    };
  };
}
