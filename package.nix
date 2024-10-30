# This is an alternative package expression of Attic in a nixpkgs-acceptable
# form. It will be submitted when the Attic API is considered stable.
#
# For the expression used for CI as well as distribution from this repo, see
# `crane.nix`.

{ lib, stdenv, rustPlatform
, pkg-config
, installShellFiles
, nix
, boost
, darwin

# Only build the client
, clientOnly ? false

# Only build certain crates
, crates ? if clientOnly then [ "attic-client" ] else [ "attic-client" "attic-server" ]
}:

let
  ignoredPaths = [ ".github" "target" "book" ];

in rustPlatform.buildRustPackage rec {
  pname = "attic";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    filter = name: type: !(type == "directory" && builtins.elem (baseNameOf name) ignoredPaths);
    src = lib.cleanSource ./.;
  };

  nativeBuildInputs = [
    pkg-config
    installShellFiles
  ];

  buildInputs = [
    nix boost
  ] ++ lib.optionals stdenv.isDarwin (with darwin.apple_sdk.frameworks; [
    SystemConfiguration
  ]);

  cargoLock = {
    lockFile = ./Cargo.lock;
    allowBuiltinFetchGit = true;
  };
  cargoBuildFlags = lib.concatMapStrings (c: "-p ${c} ") crates;

  ATTIC_DISTRIBUTOR = "attic";

  # See comment in `attic/build.rs`
  NIX_INCLUDE_PATH = "${lib.getDev nix}/include";

  # Recursive Nix is not stable yet
  doCheck = false;

  postInstall = lib.optionalString (stdenv.hostPlatform == stdenv.buildPlatform) ''
    if [[ -f $out/bin/attic ]]; then
      installShellCompletion --cmd attic \
        --bash <($out/bin/attic gen-completions bash) \
        --zsh <($out/bin/attic gen-completions zsh) \
        --fish <($out/bin/attic gen-completions fish)
    fi
  '';

  meta = with lib; {
    description = "Multi-tenant Nix binary cache system";
    homepage = "https://github.com/zhaofengli/attic";
    license = licenses.asl20;
    maintainers = with maintainers; [ zhaofengli ];
    platforms = platforms.linux ++ platforms.darwin;
    mainProgram = "attic";
  };
}
