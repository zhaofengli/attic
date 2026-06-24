let
  flake = import ./flake-compat.nix;
in
flake.defaultNix.default.overrideAttrs (_: {
  passthru = {
    demo = flake.defaultNix.outputs.devShells.${builtins.currentSystem}.demo;
  }
  // flake.defaultNix.outputs.packages.${builtins.currentSystem};
})
