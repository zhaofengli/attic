{
  description = "A Nix binary cache server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    nixpkgs-stable.url = "github:NixOS/nixpkgs/nixos-24.05";

    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nix-github-actions = {
      url = "github:nix-community/nix-github-actions";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = inputs @ { self, flake-parts, ... }: let
    supportedSystems = [
      "x86_64-linux"
      "aarch64-linux"
      "riscv64-linux"
      "aarch64-darwin"
      "x86_64-darwin"
    ];

    inherit (inputs.nixpkgs) lib;

    modules = builtins.foldl' (acc: f: f acc) ./flake [
      builtins.readDir
      (lib.filterAttrs (name: type:
        type == "regular" && lib.hasSuffix ".nix" name
      ))
      (lib.mapAttrsToList (name: _:
        lib.path.append ./flake name
      ))
    ];

  in flake-parts.lib.mkFlake { inherit inputs; } ({ inputs, self, ... }: {
    imports = modules;
    systems = supportedSystems;

    debug = true;

    flake.nixosConfigurations.example = inputs.nixpkgs.lib.nixosSystem {
      modules = [
        self.nixosModules.atticd

        ({ pkgs, ... }: {
          nixpkgs.hostPlatform = "x86_64-linux";
          services.atticd = {
            enable = true;
            credentials.server-token-rs256-secret-base64-file = pkgs.runCommand "rs256.pkcs11.b64" {} ''
              ${lib.getExe pkgs.openssl} genrsa -traditional 4096 | base64 -w0 > "$out"
            '';
            credentials."foo*".import = true;
            credentials."foo*".encrypted = true;
            credentials.this.value = "that";
            credentials.this.encrypted = true;
            credentials.this.set = true;
            settings = {
              jwt = { };
              chunking = {
                nar-size-threshold = 1;
                min-size = 64 * 1024;
                avg-size = 128 * 1024;
                max-size = 256 * 1024;
              };
            };
          };
        })
      ];
    };
  });
}
