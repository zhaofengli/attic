{ pkgs, lib, config, flake, attic, ... }:
let
  inherit (lib) types;

  serverConfigFile = config.nodes.server.services.atticd.configFile;

  cmd = {
    atticadm = "atticd-atticadm";
    atticd = ". /etc/atticd.env && export ATTIC_SERVER_TOKEN_RS256_SECRET && atticd -f ${serverConfigFile}";
  };

  makeTestDerivation = pkgs.writeShellScript "make-drv" ''
    name=$1
    base=$(basename $name)

    cat >$name <<EOF
    #!/bin/sh
    /*/sh -c "echo hello > \$out"; exit 0; */
    derivation {
      name = "$base";
      builder = ./$name;
      system = builtins.currentSystem;
      preferLocalBuild = true;
      allowSubstitutes = false;
    }
    EOF

    chmod +x $name
  '';

  databaseModules = {
    sqlite = {};
    postgres = {
      server = {
        services.postgresql = {
          enable = true;
          ensureDatabases = [ "attic" ];
          ensureUsers = [
            {
              name = "atticd";
              ensurePermissions = {
                "DATABASE attic" = "ALL PRIVILEGES";
              };
            }

            # For testing only - Don't actually do this
            {
              name = "root";
              ensureClauses = {
                superuser = true;
              };
            }
          ];
        };

        services.atticd.settings = {
          database.url = "postgresql:///attic?host=/run/postgresql";
        };
      };
    };
  };

  storageModules = {
    local = {};
    minio = let
      accessKey = "legit";
      secretKey = "111-1111111";
    in {
      server = {
        services.minio = {
          enable = true;
          rootCredentialsFile = "/etc/minio.env";
        };

        # For testing only - Don't actually do this
        environment.etc."minio.env".text = ''
          MINIO_ROOT_USER=${accessKey}
          MINIO_ROOT_PASSWORD=${secretKey}
        '';

        networking.firewall.allowedTCPPorts = [ 9000 ];

        services.atticd.settings = {
          storage = {
            type = "s3";
            endpoint = "http://server:9000";
            region = "us-east-1";
            bucket = "attic";
            credentials = {
              access_key_id = accessKey;
              secret_access_key = secretKey;
            };
          };
        };
      };
      testScript = ''
        server.succeed("mkdir /var/lib/minio/data/attic")
        server.succeed("chown minio: /var/lib/minio/data/attic")
        client.wait_until_succeeds("curl http://server:9000", timeout=20)
      '';
    };
  };
in {
  options = {
    database = lib.mkOption {
      type = types.enum [ "sqlite" "postgres" ];
      default = "sqlite";
    };
    storage = lib.mkOption {
      type = types.enum [ "local" "minio" ];
      default = "local";
    };
  };

  config = {
    name = "basic-${config.database}-${config.storage}";

    nodes = {
      server = {
        imports = [
          flake.nixosModules.atticd
          (databaseModules.${config.database}.server or {})
          (storageModules.${config.storage}.server or {})
        ];

        # For testing only - Don't actually do this
        environment.etc."atticd.env".text = ''
          ATTIC_SERVER_TOKEN_RS256_SECRET="$(openssl genrsa -traditional -out - 512)"
        '';

        services.atticd = {
          enable = true;
          credentialsFile = "/etc/atticd.env";
          settings = {
            listen = "[::]:8080";

            chunking = {
              nar-size-threshold = 1;
              min-size = 64 * 1024;
              avg-size = 128 * 1024;
              max-size = 256 * 1024;
            };
          };
        };

        environment.systemPackages = [ pkgs.openssl pkgs.attic-server ];

        networking.firewall.allowedTCPPorts = [ 8080 ];
      };

      client = {
        environment.systemPackages = [ pkgs.attic ];
      };
    };

    testScript = ''
      import time

      start_all()

      ${databaseModules.${config.database}.testScript or ""}
      ${storageModules.${config.storage}.testScript or ""}

      server.wait_for_unit('atticd.service')
      client.wait_until_succeeds("curl -sL http://server:8080", timeout=40)

      root_token = server.succeed("${cmd.atticadm} make-token --sub 'e2e-root' --validity '1 month' --push '*' --pull '*' --delete '*' --create-cache '*' --destroy-cache '*' --configure-cache '*' --configure-cache-retention '*'").strip()
      readonly_token = server.succeed("${cmd.atticadm} make-token --sub 'e2e-root' --validity '1 month' --pull 'test'").strip()

      client.succeed(f"attic login --set-default root http://server:8080 {root_token}")
      client.succeed(f"attic login readonly http://server:8080 {readonly_token}")
      client.succeed("attic login anon http://server:8080")

      # TODO: Make sure the correct status codes are returned
      # (i.e., 500s shouldn't pass the "should fail" tests)

      with subtest("Check that we can create a cache"):
          client.succeed("attic cache create test")

      with subtest("Check that we can push a path"):
          client.succeed("${makeTestDerivation} test.nix")
          test_file = client.succeed("nix-build --no-out-link test.nix")
          test_file_hash = test_file.removeprefix("/nix/store/")[:32]

          client.succeed(f"attic push test {test_file}")
          client.succeed(f"nix-store --delete {test_file}")
          client.fail(f"grep hello {test_file}")

      with subtest("Check that we can pull a path"):
          client.succeed("attic use readonly:test")
          client.succeed(f"nix-store -r {test_file}")
          client.succeed(f"grep hello {test_file}")

      with subtest("Check that we cannot push without required permissions"):
          client.fail(f"attic push readonly:test {test_file}")
          client.fail(f"attic push anon:test {test_file} 2>&1")

      with subtest("Check that we can make the cache public"):
          client.fail("curl -sL --fail-with-body http://server:8080/test/nix-cache-info")
          client.fail(f"curl -sL --fail-with-body http://server:8080/test/{test_file_hash}.narinfo")
          client.succeed("attic cache configure test --public")
          client.succeed("curl -sL --fail-with-body http://server:8080/test/nix-cache-info")
          client.succeed(f"curl -sL --fail-with-body http://server:8080/test/{test_file_hash}.narinfo")

      with subtest("Check that we can trigger garbage collection"):
          test_file_hash = test_file.removeprefix("/nix/store/")[:32]
          client.succeed(f"curl -sL --fail-with-body http://server:8080/test/{test_file_hash}.narinfo")
          client.succeed("attic cache configure test --retention-period 1s")
          time.sleep(2)
          server.succeed("${cmd.atticd} --mode garbage-collector-once")
          client.fail(f"curl -sL --fail-with-body http://server:8080/test/{test_file_hash}.narinfo")

      ${lib.optionalString (config.storage == "local") ''
      with subtest("Check that all chunks are actually deleted after GC"):
          files = server.succeed("find /var/lib/atticd/storage -type f")
          print(f"Remaining files: {files}")
          assert files.strip() == ""
      ''}

      with subtest("Check that we can include the upload info in the payload"):
          client.succeed("${makeTestDerivation} test2.nix")
          test2_file = client.succeed("nix-build --no-out-link test2.nix")
          client.succeed(f"attic push --force-preamble test {test2_file}")
          client.succeed(f"nix-store --delete {test2_file}")
          client.succeed(f"nix-store -r {test2_file}")

      with subtest("Check that we can destroy the cache"):
          client.succeed("attic cache info test")
          client.succeed("attic cache destroy --no-confirm test")
          client.fail("attic cache info test")
          client.fail("curl -sL --fail-with-body http://server:8080/test/nix-cache-info")
    '';
  };
}
