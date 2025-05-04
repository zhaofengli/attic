{ lib, stdenv, nix-gitignore, mdbook, mdbook-linkcheck, python3, callPackage, writeScript, nixosOptionsDoc
, eval
, attic ? null
}:

let
  colorizedHelp = let
    help = callPackage ./colorized-help.nix {
      inherit attic;
    };
  in if attic != null then help else null;

  optionsDoc = nixosOptionsDoc {
    inherit (eval) options;

    # Default is currently "appendix".
    documentType = "none";

    # Only produce Markdown
    allowDocBook = false;
    markdownByDefault = true;

    warningsAreErrors = false;

    transformOptions = let
      ourPrefix = "${toString ../.}/";
    in
      opt:
        opt
        // {
          # Disappear anything that's not one of ours.
          visible = opt.visible && lib.hasInfix "atticd" opt.name;
          declarations = map (decl: let
            name = lib.removePrefix ourPrefix decl;
          in
            if lib.hasPrefix ourPrefix decl
            then {
              inherit name;
              url = "https://github.com/zhaofengli/attic/blob/main/${name}";
            }
            else decl)
          opt.declarations;
        };
  };
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

    {
      echo "# NixOS Module Options"
      cat ${optionsDoc.optionsCommonMark}
    } >> src/reference/nixos-module-options.md

    mdbook build -d ./build
    cp -r ./build $out
  '';

  installPhase = "true";
}
