# For distribution from this repository as well as CI, we use Crane to build
# Attic.
#
# For a nixpkgs-acceptable form of the package expression, see `package.nixpkgs.nix`
# which will be submitted when the Attic API is considered stable. However, that
# expression is not tested by CI so to not slow down the hot path.

{ stdenv
, lib
, buildPackages
, craneLib
, rust
, runCommand
, writeReferencesToFile
, pkg-config
, installShellFiles
, jq

, nix
, boost
, libarchive
, darwin
, libiconv

, extraPackageArgs ? {}
}:

let
  version = "0.1.0";

  ignoredPaths = [
    ".ci"
    ".github"
    "book"
    "flake"
    "integration-tests"
    "nixos"
    "target"
  ];

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
    libarchive
  ] ++ lib.optionals stdenv.isDarwin [
    darwin.apple_sdk.frameworks.SystemConfiguration
    libiconv
  ];

  crossArgs = let
    rustTargetSpec = rust.toRustTargetSpec stdenv.hostPlatform;
    rustTargetSpecEnv = lib.toUpper (builtins.replaceStrings [ "-" ] [ "_" ] rustTargetSpec);
  in lib.optionalAttrs (stdenv.hostPlatform != stdenv.buildPlatform) {
    depsBuildBuild = [ buildPackages.stdenv.cc ];

    CARGO_BUILD_TARGET = rustTargetSpec;
    "CARGO_TARGET_${rustTargetSpecEnv}_LINKER" = "${stdenv.cc.targetPrefix}cc";
  };

  extraArgs = crossArgs // extraPackageArgs;

  cargoArtifacts = craneLib.buildDepsOnly ({
    pname = "attic";
    inherit src version nativeBuildInputs buildInputs;

    # By default it's "use-symlink", which causes Crane's `inheritCargoArtifactsHook`
    # to copy the artifacts using `cp --no-preserve=mode` which breaks the executable
    # bit of bindgen's build-script binary.
    #
    # With `use-zstd`, the cargo artifacts are archived in a `tar.zstd`. This is
    # actually set if you use `buildPackage` without passing `cargoArtifacts`.
    installCargoArtifactsMode = "use-zstd";
  } // extraArgs);

  mkAttic = {
    packages,
  }: let
    cargoPackageArgs = map (p: "-p ${p}") packages;
  in craneLib.buildPackage ({
    pname = "attic";
    inherit src version nativeBuildInputs buildInputs cargoArtifacts;

    ATTIC_DISTRIBUTOR = "attic";

    # See comment in `attic-tests`
    doCheck = false;

    cargoExtraArgs = lib.concatStringsSep " " cargoPackageArgs;

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

    passthru = {
      inherit nix;
    };
  } // extraArgs);

  attic = mkAttic {
    packages = ["attic-client" "attic-server"];
  };

  # Client-only package.
  attic-client = mkAttic {
    packages = ["attic-client"];
  };

  # Server-only package with fat LTO enabled.
  #
  # Because of Cargo's feature unification, the common `attic` crate always
  # has the `nix_store` feature enabled if the client and server are built
  # together, leading to `atticd` linking against `libnixstore` as well. This
  # package is slimmer with more optimization.
  #
  # We don't enable fat LTO in the default `attic` package since it
  # dramatically increases build time.
  attic-server = craneLib.buildPackage ({
    pname = "attic-server";

    # We don't pull in the common cargoArtifacts because the feature flags
    # and LTO configs are different
    inherit src version nativeBuildInputs buildInputs;

    # See comment in `attic-tests`
    doCheck = false;

    cargoExtraArgs = "-p attic-server";

    CARGO_PROFILE_RELEASE_LTO = "fat";
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "1";

    meta = {
      mainProgram = "atticd";
    };
  } // extraArgs);

  # Attic interacts with Nix directly and its tests require trusted-user access
  # to nix-daemon to import NARs, which is not possible in the build sandbox.
  # In the CI pipeline, we build the test executable inside the sandbox, then
  # run it outside.
  attic-tests = craneLib.mkCargoDerivation ({
    pname = "attic-tests";

    inherit src version buildInputs cargoArtifacts;

    nativeBuildInputs = nativeBuildInputs ++ [ jq ];

    doCheck = true;

    buildPhaseCargoCommand = "";
    checkPhaseCargoCommand = "cargoWithProfile test --no-run --message-format=json >cargo-test.json";
    doInstallCargoArtifacts = false;

    installPhase = ''
      runHook preInstall

      mkdir -p $out/bin
      jq -r 'select(.reason == "compiler-artifact" and .target.test and .executable) | .executable' <cargo-test.json | \
        xargs -I _ cp _ $out/bin

      runHook postInstall
    '';
  } // extraArgs);
in {
  inherit cargoArtifacts attic attic-client attic-server attic-tests;
}
