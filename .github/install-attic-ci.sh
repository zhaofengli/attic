#!/usr/bin/env bash
set -euo pipefail
expr=$(mktemp)

cleanup() {
  rm -f "$expr"
}

cat >"$expr" <<'EOF'
  { system ? builtins.currentSystem }:
let
  maybeStorePath = if builtins ? langVersion && builtins.lessThan 1 builtins.langVersion
    then builtins.storePath
    else x: x;
  mkFakeDerivation = attrs: outputs:
    let
      outputNames = builtins.attrNames outputs;
      common = attrs // outputsSet //
        { type = "derivation";
          outputs = outputNames;
          all = outputsList;
        };
      outputToAttrListElement = outputName:
        { name = outputName;
          value = common // {
            inherit outputName;
            outPath = maybeStorePath (builtins.getAttr outputName outputs);
            drvPath = maybeStorePath (builtins.getAttr outputName outputs);
          };
        };
      outputsList = map outputToAttrListElement outputNames;
      outputsSet = builtins.listToAttrs outputsList;
    in outputsSet;
in

{
  "x86_64-linux" = (mkFakeDerivation {
  name = "attic-0.1.0";
  system = "x86_64-linux";
} {
  out = "/nix/store/pl3ddk9579aqypxwjsapfvz946j4r1cv-attic-0.1.0";
}).out;

  "aarch64-linux" = (mkFakeDerivation {
  name = "attic-0.1.0";
  system = "aarch64-linux";
} {
  out = "/nix/store/5bm1mqpd0xjfhmbrz3gvznddvdzinykw-attic-0.1.0";
}).out;

  "x86_64-darwin" = (mkFakeDerivation {
  name = "attic-0.1.0";
  system = "x86_64-darwin";
} {
  out = "/nix/store/kl6v8r5z7xd8pq2nk3yfp2678m10k89v-attic-0.1.0";
}).out;

  "aarch64-darwin" = (mkFakeDerivation {
  name = "attic-0.1.0";
  system = "aarch64-darwin";
} {
  out = "/nix/store/bxbs5q50sxzpxjk1idz10xsybkgx3hr7-attic-0.1.0";
}).out;

}.${system}

EOF

nix-env --substituters "https://staging.attic.rs/attic-ci https://cache.nixos.org" --trusted-public-keys "attic-ci:U5Sey4mUxwBXM3iFapmP0/ogODXywKLRNgRPQpEXxbo= cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=" -if "$expr"
