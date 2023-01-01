{ lib, stdenv, rustPlatform
, pkg-config
, installShellFiles
, nix
, boost
, darwin

# Only build the client
, clientOnly ? false

# Only build certain crates
, crates ? if clientOnly then [ "attic-client" ] else []
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
    rustPlatform.bindgenHook
    pkg-config
    installShellFiles
  ];

  buildInputs = [
    nix boost
  ] ++ lib.optionals stdenv.isDarwin (with darwin.apple_sdk.frameworks; [
    SystemConfiguration
  ]);

  cargoHash = "sha256-9gJGY/6m6ao8srnhJ3WzDx35F56lhwZ6t0T3FSn/p7g=";
  cargoBuildFlags = lib.concatMapStrings (c: "-p ${c} ") crates;

  ATTIC_DISTRIBUTOR = "attic";

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
    license = licenses.agpl3Plus;
    maintainers = with maintainers; [ zhaofengli ];
    platforms = platforms.linux ++ platforms.darwin;
  };
}
