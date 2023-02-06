# For distribution from this repository as well as CI, we use Crane to build
# Attic.
#
# For a nixpkgs-acceptable form of the package expression, see `package.nixpkgs.nix`
# which will be submitted when the Attic API is considered stable. However, that
# expression is not tested by CI so to not slow down the hot path.

{ stdenv
, lib
, craneLib
, llvmPackages
, rustPlatform
, runCommand
, writeReferencesToFile
, pkg-config
, installShellFiles
, jq

, nix
, boost
, darwin
, libiconv
}:

let
  version = "0.1.0";

  ignoredPaths = [ ".github" "target" "book" "nixos" "integration-tests" ];

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
  ] ++ lib.optionals stdenv.isDarwin [
    darwin.apple_sdk.frameworks.SystemConfiguration
    libiconv
  ];

  cargoArtifacts = craneLib.buildDepsOnly {
    pname = "attic";
    inherit src nativeBuildInputs buildInputs;

    # By default it's "use-symlink", which causes Crane's `inheritCargoArtifactsHook`
    # to copy the artifacts using `cp --no-preserve=mode` which breaks the executable
    # bit of bindgen's build-script binary.
    #
    # With `use-zstd`, the cargo artifacts are archived in a `tar.zstd`. This is
    # actually set if you use `buildPackage` without passing `cargoArtifacts`.
    installCargoArtifactsMode = "use-zstd";
  };

  attic = craneLib.buildPackage {
    pname = "attic";
    inherit src version nativeBuildInputs buildInputs cargoArtifacts;

    ATTIC_DISTRIBUTOR = "attic";

    # See comment in `attic-tests`
    doCheck = false;

    cargoExtraArgs = "-p attic-client -p attic-server";

    # Temporary workaround for https://github.com/NixOS/nixpkgs/pull/207352#issuecomment-1418363441
    preBuild = ''
      export LIBCLANG_PATH="${llvmPackages.libclang.lib}/lib"
      export BINDGEN_EXTRA_CLANG_ARGS="$(< ${llvmPackages.clang}/nix-support/cc-cflags) $(< ${llvmPackages.clang}/nix-support/libc-cflags) $(< ${llvmPackages.clang}/nix-support/libcxx-cxxflags) $NIX_CFLAGS_COMPILE"
    '';

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
    };
  };

  # Client-only package.
  attic-client = attic.overrideAttrs (old: {
    cargoExtraArgs = " -p attic-client";
  });

  # Server-only package with fat LTO enabled.
  #
  # Because of Cargo's feature unification, the common `attic` crate always
  # has the `nix_store` feature enabled if the client and server are built
  # together, leading to `atticd` linking against `libnixstore` as well. This
  # package is slimmer with more optimization.
  #
  # We don't enable fat LTO in the default `attic` package since it
  # dramatically increases build time.
  attic-server = craneLib.buildPackage {
    pname = "attic-server";

    # We don't pull in the common cargoArtifacts because the feature flags
    # and LTO configs are different
    inherit src version nativeBuildInputs buildInputs;

    # See comment in `attic-tests`
    doCheck = false;

    cargoExtraArgs = "-p attic-server";

    CARGO_PROFILE_RELEASE_LTO = "fat";
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "1";
  };

  # Attic interacts with Nix directly and its tests require trusted-user access
  # to nix-daemon to import NARs, which is not possible in the build sandbox.
  # In the CI pipeline, we build the test executable inside the sandbox, then
  # run it outside.
  attic-tests = craneLib.mkCargoDerivation {
    pname = "attic-tests";

    inherit src version buildInputs cargoArtifacts;

    nativeBuildInputs = nativeBuildInputs ++ [ jq ];

    doCheck = true;

    # Temporary workaround for https://github.com/NixOS/nixpkgs/pull/207352#issuecomment-1418363441
    preBuild = ''
      export LIBCLANG_PATH="${llvmPackages.libclang.lib}/lib"
      export BINDGEN_EXTRA_CLANG_ARGS="$(< ${llvmPackages.clang}/nix-support/cc-cflags) $(< ${llvmPackages.clang}/nix-support/libc-cflags) $(< ${llvmPackages.clang}/nix-support/libcxx-cxxflags) $NIX_CFLAGS_COMPILE"
    '';

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
  };
in {
  inherit cargoArtifacts attic attic-client attic-server attic-tests;
}
