{ lib, stdenv, runCommand, attic, ansi2html }:

with builtins;

let
  commands = {
    attic = [
      null
      "login"
      "use"
      "push"
      "watch-store"
      "cache"
      "cache create"
      "cache configure"
      "cache destroy"
      "cache info"
    ];
    atticd = [
      null
    ];
    atticadm = [
      null
      "make-token"
    ];
  };
  renderMarkdown = name: subcommands: ''
    mkdir -p $out
    (
      ansi2html -H
      ${lib.concatMapStrings (subcommand: let
        fullCommand = "${name} ${if subcommand == null then "" else subcommand}";
      in "${renderCommand fullCommand}\n") subcommands}
    ) >>$out/${name}.md
  '';
  renderCommand = fullCommand: ''
    echo '## `${fullCommand}`'
    echo -n '<pre><div class="hljs">'
    TERM=xterm-256color CLICOLOR_FORCE=1 ${fullCommand} --help | ansi2html -p
    echo '</div></pre>'
  '';
in runCommand "attic-colorized-help" {
  nativeBuildInputs = [ attic ansi2html ];
} (concatStringsSep "\n" (lib.mapAttrsToList renderMarkdown commands))
