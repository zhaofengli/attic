let
  flake = import ./flake-compat.nix;
in flake.defaultNix.default.overrideAttrs (_: {
  passthru = {
    attic-client = flake.defaultNix.outputs.packages.${builtins.currentSystem}.attic-client;
    demo = flake.defaultNix.outputs.devShells.${builtins.currentSystem}.demo;
  };
})
