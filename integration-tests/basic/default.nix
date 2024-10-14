{ pkgs, lib, config, flake, attic, ... }:
let
  inherit (lib) types;

  serverConfigFile = config.nodes.server.services.atticd.configFile;

  cmd = {
    atticadm = ". /etc/atticd.env && export ATTIC_SERVER_TOKEN_RS256_SECRET_BASE64 && atticd-atticadm";
    atticd = ". /etc/atticd.env && export ATTIC_SERVER_TOKEN_RS256_SECRET_BASE64 && atticd -f ${serverConfigFile}";
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
    sqlite = {
      testScriptPost = ''
        from pathlib import Path
        import os

        schema = server.succeed("${pkgs.sqlite}/bin/sqlite3 /var/lib/atticd/server.db '.schema --indent'")

        schema_path = Path(os.environ.get("out", os.getcwd())) / "schema.sql"
        with open(schema_path, 'w') as f:
            f.write(schema)
      '';
    };
    postgres = {
      server = {
        services.postgresql = {
          enable = true;
          ensureDatabases = [ "attic" ];
          ensureUsers = [
            {
              name = "atticd";
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

        systemd.services.postgresql.postStart = lib.mkAfter ''
          $PSQL -tAc 'ALTER DATABASE "attic" OWNER TO "atticd"'
        '';

        services.atticd.settings = {
          database.url = "postgresql:///attic?host=/run/postgresql";
        };
      };
      testScriptPost = ''
        from pathlib import Path
        import os

        schema = server.succeed("pg_dump --schema-only attic")

        schema_path = Path(os.environ.get("out", os.getcwd())) / "schema.sql"
        with open(schema_path, 'w') as f:
            f.write(schema)
      '';
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
          ATTIC_SERVER_TOKEN_RS256_SECRET_BASE64='LS0tLS1CRUdJTiBSU0EgUFJJVkFURSBLRVktLS0tLQpNSUlFcEFJQkFBS0NBUUVBekhqUzFGKzlRaFFUdlJZYjZ0UGhxS09FME5VYkIraTJMOTByWVBNQVVoYVBUMmlKCmVUNk9vWFlmZWszZlZ1dXIrYks1VWFVRjhUbEx2Y1FHa1Arckd0WDRiQUpGTWJBcTF3Y25FQ3R6ZGVERHJnSlIKMGUvNWJhdXQwSS9YS0ticG9oYjNvWVhtUmR5eG9WVGE3akY1bk11ajBsd25kUTcwYTF1ZGkzMGNpYkdTWHZMagpVeGltL3ByYjUrV3ZPdjN4UnhlbDZHYmptUW1RMVBHeHVLcmx3b1ZKRnlWTjl3QmExajBDelJDcURnTFRwQWw0CjhLVWlDY2V1VUZQcmdZaW9vSVhyVExlWmxVbFVVV3FHSDBJbGFKeVUyQ05iNWJtZWM1TnZ4RDlaakFoYytucmgKRS80VzkxajdQMFVyQnp4am9NUTRlKzBPZDhmQnBvSDAwbm4xUXdJREFRQUJBb0lCQUE2RmxEK21Ed3gyM1pJRAoxSGJBbHBuQ0IwaEhvbFJVK0Q5OC96d3k5ZlplaU00VWVCTUcyTjFweE1HTWIweStqeWU4UkVJaXJNSGRsbDRECllvNEF3bmUwODZCRUp3TG81cG4vOVl2RjhqelFla1ZNLzkrZm9nRGlmUVUvZWdIMm5NZzR4bHlQNUhOWXdicmEKQ25SNVNoQlRQQzdRQWJOa0hRTFU3bUwrUHowZUlXaG9KWVRoUUpkU0g3RDB0K1QwZzVVNDdPam5qbXJaTWwxaApHOE1IUHhKMk5WU1l2N0dobnpjblZvcVVxYzlxeldXRDZXZERtV1BPNGJ1K2p0b2E2U2o4cjJtb0RRZ1A5YXNhCm93RUFJbHBmbVkxYUx2dENwWG4rejRTTWJKcHRXMlVvaktGa2dkYm9jZmtXYWdtSGZRa2xmS0dBQ0hibU9ZV24KeDRCbTU3a0NnWUVBN1dXaXJDZnBRR01hR3A2WWxMQlVUc1VJSXJOclF4UmtuRlc3dFVYd0NqWFZ5SDlTR3FqNgphTkNhYzZpaks3QVNBYXlxY1JQRjFPY2gyNmxpVmRKUHNuRGxwUjhEVXB2TzRVOVRzSTJyZ1lZYzNrSWkzVGFKClgzV0Vic1Z6Nk45WXFPSXlnVnZiTEhLS0F4Uyt4b1Z2SjkzQmdWRHN5SkxRdmhrM3VubXk3M2tDZ1lFQTNINnYKeUhOKzllOVAyOS9zMVY1eWZxSjdvdVdKV0lBTHFDYm9zOTRRSVdPSG5HRUtSSGkydWIzR0d6U2tRSzN1eTUrdQo4M0txaFJOejRVMkdOK1pLaFE0NHhNVmV4TUVvZzJVU3lTaVZ0cFdqWXBwT2Q1NnVaMzRWaFU2TWRNZS9zT0JnCnNoei84MUxUSis2cHdFZE9wV2tPVlRaMXJISlZXQmdtVk5qWjc1c0NnWUVBNVd5YjBaU2dyMEVYTVRLa2NzNFcKTENudXV0cDZodEZtaWsrd29IZCtpOStMUThFSU1BdXVOUzJrbHJJYlAxVmhrWXkxQzZMNFJkRTV2M2ZyT05XUApmL3ZyYzdDTkhZREdacWlyVUswWldvdXB5b0pQLzBsOWFXdkJHT3hxSUZ2NDZ2M3ZvV1NNWkdBdFVOenpvaGZDClhOeks3WmF2dndka0JOT0tNQVQ5RU1FQ2dZRUF3NEhaWDRWNUo1d2dWVGVDQ2RjSzhsb2tBbFpBcUNZeEw5SUEKTjZ4STVUSVpSb0dNMXhXcC81dlRrci9rZkMwOU5YUExiclZYbVZPY1JrTzFKTStmZDhjYWN1OEdqck11dHdMaAoyMWVQR0N3cWlQMkZZZTlqZVFTRkZJU0hhZXpMZll3V2NSZmhvdURudGRxYXpaRHNuU0kvd1RMZXVCOVFxU0lRCnF0NzByczBDZ1lCQ2lzV0VKdXpQUUlJNzVTVkU4UnJFZGtUeUdhOEVBOHltcStMdDVLRDhPYk80Q2JHYVFlWXkKWFpjSHVyOFg2cW1lWHZVU3MwMHBMMUdnTlJ3WCtSUjNMVDhXTm9vc0NqVDlEUW9GOFZveEtseDROVTRoUGlrTQpBc0w1RS9wYnVLeXkvSU5LTnQyT3ZPZmJYVitlTXZQdGs5c1dORjNyRTBYcU15TW9maG9NaVE9PQotLS0tLUVORCBSU0EgUFJJVkFURSBLRVktLS0tLQo='
        '';

        services.atticd = {
          enable = true;
          environmentFile = "/etc/atticd.env";
          settings = {
            listen = "[::]:8080";

            jwt = { };

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

      root_token = server.succeed("${cmd.atticadm} make-token --sub 'e2e-root' --validity '1 month' --push '*' --pull '*' --delete '*' --create-cache '*' --destroy-cache '*' --configure-cache '*' --configure-cache-retention '*' </dev/null").strip()
      readonly_token = server.succeed("${cmd.atticadm} make-token --sub 'e2e-root' --validity '1 month' --pull 'test' </dev/null").strip()

      client.succeed(f"attic login --set-default root http://server:8080 {root_token}")
      client.succeed(f"attic login readonly http://server:8080 {readonly_token}")
      client.succeed("attic login anon http://server:8080")

      # TODO: Make sure the correct status codes are returned
      # (i.e., 500s shouldn't pass the "should fail" tests)

      with subtest("Check that we can create a cache"):
          client.succeed("attic cache create test")

      with subtest("Check that we can push a path"):
          client.succeed("${makeTestDerivation} test.nix")
          test_file = client.succeed("nix-build --no-out-link test.nix").strip()
          test_file_hash = test_file.removeprefix("/nix/store/")[:32]

          client.succeed(f"attic push test {test_file}")
          client.succeed(f"nix-store --delete {test_file}")
          client.fail(f"ls {test_file}")

      with subtest("Check that we can pull a path"):
          client.succeed("attic use readonly:test")
          client.succeed(f"nix-store -r {test_file}")
          client.succeed(f"grep hello {test_file}")

      with subtest("Check that we cannot push without required permissions"):
          client.fail(f"attic push readonly:test {test_file}")
          client.fail(f"attic push anon:test {test_file} 2>&1")

      with subtest("Check that we can push a list of paths from stdin"):
          paths = []
          for i in range(10):
              client.succeed(f"${makeTestDerivation} seq{i}.nix")
              path = client.succeed(f"nix-build --no-out-link seq{i}.nix").strip()
              client.succeed(f"echo {path} >>paths.txt")
              paths.append(path)

          client.succeed("attic push test --stdin <paths.txt 2>&1")

          for path in paths:
              client.succeed(f"nix-store --delete {path}")

      with subtest("Check that we can pull the paths back"):
          for path in paths:
              client.fail(f"ls {path}")
              client.succeed(f"nix-store -r {path}")
              client.succeed(f"grep hello {path}")

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
          files = server.succeed("find /var/lib/atticd/storage -type f ! -name 'VERSION'")
          print(f"Remaining files: {files}")
          assert files.strip() == "", "Some files remain after GC: " + files
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

      ${databaseModules.${config.database}.testScriptPost or ""}
      ${storageModules.${config.storage}.testScriptPost or ""}
    '';
  };
}
