{ lib, stdenv, nix-gitignore, mdbook, mdbook-linkcheck, python3, callPackage, writeScript
, attic ? null
}:

let
  colorizedHelp = let
    help = callPackage ./colorized-help.nix {
      inherit attic;
    };
  in if attic != null then help else null;
in stdenv.mkDerivation {
  inherit colorizedHelp;

  name = "attic-book";

  src = nix-gitignore.gitignoreSource [] ./.;

  nativeBuildInputs = [ mdbook ];

  buildPhase = ''
    emitColorizedHelp() {
      command=$1

      if [[ -n "$colorizedHelp" ]]; then
          cat "$colorizedHelp/$command.md" >> src/reference/$command-cli.md
      else
          echo "Error: No attic executable passed to the builder" >> src/reference/$command-cli.md
      fi
    }

    emitColorizedHelp attic
    emitColorizedHelp atticd
    emitColorizedHelp atticadm

    mdbook build -d ./build
    cp -r ./build $out
  '';

  installPhase = "true";
}
