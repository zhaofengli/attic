# Generates a script to bootstrap the Attic client with a substituter for
# CI usage. Will be simplier when Attic is in cache.nixos.org.

{ self
, writeText
, writeScript

, substituter ? "https://staging.attic.rs/attic-ci"
, trustedPublicKey ? "attic-ci:U5Sey4mUxwBXM3iFapmP0/ogODXywKLRNgRPQpEXxbo="
}:

let
  cacheNixosOrg = "https://cache.nixos.org";
  cacheNixosOrgKey = "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";

  bootstrapHeader = ''
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
  '';

  makeBootstrap = system: let
    package = self.packages.${system}.attic;
  in ''
    "${system}" = (mkFakeDerivation {
      name = "${package.name}";
      system = "${system}";
    } {
      out = "${package.out}";
    }).out;
  '';

  bootstrapExpr = ''
    { system ? builtins.currentSystem }:
    ${bootstrapHeader}
    {
      ${makeBootstrap "x86_64-linux"}
      ${makeBootstrap "aarch64-linux"}
      ${makeBootstrap "x86_64-darwin"}
      ${makeBootstrap "aarch64-darwin"}
    }.''${system}
  '';

  bootstrapScript = writeScript "install-attic-ci.sh" ''
    #!/usr/bin/env bash
    set -euo pipefail
    expr=$(mktemp)

    cleanup() {
      rm -f "$expr"
    }

    cat >"$expr" <<'EOF'
      ${bootstrapExpr}
    EOF

    nix-env --substituters "${substituter} ${cacheNixosOrg}" --trusted-public-keys "${trustedPublicKey} ${cacheNixosOrgKey}" -if "$expr"
  '';
in bootstrapScript
