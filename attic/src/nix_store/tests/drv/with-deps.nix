#!/bin/sh
/*/sh -c "echo Hi! I depend on $dep. > $out"; exit 0; */
let
  a = derivation {
    name = "attic-test-with-deps-a";
    builder = ./with-deps.nix;
    system = "x86_64-linux";
    dep = b;
  };
  b = derivation {
    name = "attic-test-with-deps-b";
    builder = ./with-deps.nix;
    system = "x86_64-linux";
    dep = c;
  };
  c = derivation {
    name = "attic-test-with-deps-c-final";
    builder = ./with-deps.nix;
    system = "x86_64-linux";
  };
in a
