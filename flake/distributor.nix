{ lib, flake-parts-lib, ... }:
let
  inherit (lib)
    mkOption
    types
    ;
in
{
  options = {
    attic.distributor = mkOption {
      type = types.str;
      default = "dev";
    };
  };
}
