{ pkgs, lib, config, flake, attic, ... }:
let
  inherit (lib) types;

  serverConfigFile = config.nodes.server.services.atticd.configFile;

  cmd = {
    atticadm = ". /etc/atticd.env && export ATTIC_SERVER_TOKEN_RS256_SECRET_BASE64 && atticd-atticadm";
    atticd = ". /etc/atticd.env && systemd-run --wait --pipe --property=DynamicUser=yes --property=User=atticd --property=StateDirectory=atticd --setenv=ATTIC_SERVER_TOKEN_RS256_SECRET_BASE64=\\\"$ATTIC_SERVER_TOKEN_RS256_SECRET_BASE64\\\" atticd -f ${serverConfigFile}";
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

        systemd.services.postgresql-setup.postStart = lib.mkAfter ''
          psql -tAc 'ALTER DATABASE "attic" OWNER TO "atticd"'
        '';

        services.atticd.settings = {
          database.url = "postgresql://atticd@localhost/attic?host=/run/postgresql";
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
    garage = let
      accessKey = "GKaaaaaaaaaaaaaaaaaaaaaaaa";
      secretKey = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    in {
      server = {
        services.garage = {
          enable = true;
          package = pkgs.garage_2;
          settings = {
            rpc_bind_addr = "127.0.0.1:3901";
            rpc_public_addr = "127.0.0.1:3901";
            rpc_secret = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
            replication_factor = 1;

            s3_api = {
              s3_region = "garage";
              api_bind_addr = "0.0.0.0:9000";
            };
          };
        };

        networking.firewall.allowedTCPPorts = [ 9000 ];

        services.atticd.settings = {
          storage = {
            type = "s3";
            endpoint = "http://server:9000";
            region = "garage";
            bucket = "attic";
            credentials = {
              access_key_id = accessKey;
              secret_access_key = secretKey;
            };
          };
        };
      };
      testScript = ''
        server.wait_for_unit("garage.service")
        server.wait_for_open_port(3901)
        garage_node_id = server.succeed("garage status | tail -n1 | awk '{ print $1 }'")
        server.succeed(
            f"garage layout assign -c 100MB -z garage {garage_node_id}",
            "garage layout apply --version 1",
            "garage key import ${accessKey} ${secretKey} --yes",
            "garage bucket create attic",
            "garage bucket allow --read --write --owner attic --key ${accessKey}"
        )

        server.wait_for_unit("atticd.service")
        server.wait_for_open_port(9000)
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
      type = types.enum [ "local" "garage" ];
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
        environment.systemPackages = [ pkgs.attic pkgs.python3 ];
      };

      proxy = {
        services.nginx = {
          enable = true;
          commonHttpConfig = ''
            log_format upload '$request_method $uri $http_x_attic_nar_compression';
          '';
          virtualHosts = {
            compressed = {
              listen = [ { addr = "0.0.0.0"; port = 8081; } ];
              locations."/" = {
                proxyPass = "http://server:8080";
                extraConfig = ''
                  proxy_set_header Host $host:$server_port;
                  access_log /var/log/nginx/compressed.log upload;
                '';
              };
            };
            legacy = {
              listen = [ { addr = "0.0.0.0"; port = 8082; } ];
              locations."/" = {
                proxyPass = "http://server:8080";
                extraConfig = ''
                  proxy_set_header Host $host:$server_port;
                  access_log /var/log/nginx/legacy.log upload;
                  sub_filter '"upload_compression"' '"unknown_upload_compression"';
                  sub_filter_types application/json;
                  sub_filter_once off;
                '';
              };
            };
          };
        };

        networking.firewall.allowedTCPPorts = [ 8081 8082 ];
      };
    };

    testScript = ''
      import shlex
      import time

      start_all()

      ${databaseModules.${config.database}.testScript or ""}
      ${storageModules.${config.storage}.testScript or ""}

      server.wait_for_unit('atticd.service')
      proxy.wait_for_unit('nginx.service')
      client.wait_until_succeeds("curl -sL http://server:8080", timeout=40)
      client.wait_until_succeeds("curl -sL http://proxy:8081", timeout=40)
      client.wait_until_succeeds("curl -sL http://proxy:8082", timeout=40)

      root_token = server.succeed("${cmd.atticadm} make-token --sub 'e2e-root' --validity '1 month' --push '*' --pull '*' --delete '*' --create-cache '*' --destroy-cache '*' --configure-cache '*' --configure-cache-retention '*' </dev/null").strip()
      readonly_token = server.succeed("${cmd.atticadm} make-token --sub 'e2e-root' --validity '1 month' --pull 'test' </dev/null").strip()

      client.succeed(f"attic login --set-default root http://server:8080 {root_token}")
      client.succeed(f"attic login compressed http://proxy:8081 {root_token}")
      client.succeed(f"attic login disabled http://proxy:8081 {root_token}")
      client.succeed("sed -i '/\\[servers.disabled\\]/,/^\\[/{s/upload-compression = true/upload-compression = false/}' ~/.config/attic/config.toml")
      client.succeed(f"attic login legacy http://proxy:8082 {root_token}")
      client.succeed(f"attic login readonly http://server:8080 {readonly_token}")
      client.succeed("attic login anon http://server:8080")

      # TODO: Make sure the correct status codes are returned
      # (i.e., 500s shouldn't pass the "should fail" tests)

      with subtest("Check that we can create a cache"):
          client.succeed("attic cache create test")

      with subtest("Check that a negotiated upload is compressed"):
          client.succeed("${makeTestDerivation} test.nix")
          test_file = client.succeed("nix-build --no-out-link test.nix").strip()
          test_file_hash = test_file.removeprefix("/nix/store/")[:32]

          client.succeed(f"attic push compressed:test {test_file}")
          proxy.succeed("grep -E '^PUT /_api/v1/upload-path zstd$' /var/log/nginx/compressed.log")
          client.succeed(f"nix-store --delete {test_file}")
          client.fail(f"ls {test_file}")

      with subtest("Check that we can pull a compressed upload"):
          client.succeed("attic use readonly:test")
          client.succeed(f"nix-store -r {test_file}")
          client.succeed(f"grep hello {test_file}")

      with subtest("Check that we cannot push without required permissions"):
          client.fail(f"attic push readonly:test {test_file}")
          client.fail(f"attic push anon:test {test_file} 2>&1")

      with subtest("Check that per-server compression can be disabled"):
          client.succeed("${makeTestDerivation} disabled.nix")
          disabled_file = client.succeed("nix-build --no-out-link disabled.nix").strip()
          proxy.succeed("truncate -s 0 /var/log/nginx/compressed.log")
          client.succeed(f"attic push disabled:test {disabled_file}")
          proxy.succeed("grep -E '^PUT /_api/v1/upload-path -$' /var/log/nginx/compressed.log")

      with subtest("Check that absent server capability uses a legacy upload"):
          client.succeed("${makeTestDerivation} legacy.nix")
          legacy_file = client.succeed("nix-build --no-out-link legacy.nix").strip()
          client.succeed(f"attic push legacy:test {legacy_file}")
          proxy.succeed("grep -E '^PUT /_api/v1/upload-path -$' /var/log/nginx/legacy.log")

      with subtest("Check that an old-style uncompressed request remains accepted"):
          client.succeed("${makeTestDerivation} raw.nix")
          raw_file = client.succeed("nix-build --no-out-link raw.nix").strip()
          client.succeed(f"nix-store --dump {raw_file} > raw.nar")
          raw_hash = client.succeed("nix-hash --type sha256 --flat --base32 raw.nar").strip()
          raw_size = int(client.succeed("stat -c %s raw.nar").strip())
          raw_store_hash = raw_file.removeprefix("/nix/store/")[:32]
          raw_info = client.succeed(f"python3 -c 'import json; print(json.dumps({{\"cache\":\"test\",\"store_path_hash\":\"{raw_store_hash}\",\"store_path\":\"{raw_file}\",\"references\":[],\"system\":None,\"deriver\":None,\"sigs\":[],\"ca\":None,\"nar_hash\":\"sha256:{raw_hash}\",\"nar_size\":{raw_size}}}))'").strip()
          raw_info_header = shlex.quote(f"X-Attic-Nar-Info: {raw_info}")
          client.succeed(f"curl --fail-with-body -X PUT -H 'Authorization: Bearer {root_token}' -H {raw_info_header} --data-binary @raw.nar http://server:8080/_api/v1/upload-path")

      with subtest("Check that malformed compressed uploads fail cleanly"):
          malformed_status = client.succeed(f"printf broken | curl -sS -o malformed-response.json -w '%{{http_code}}' -X PUT -H 'Authorization: Bearer {root_token}' -H 'X-Attic-Nar-Compression: zstd' -H 'X-Attic-Nar-Info: {{}}' --data-binary @- http://server:8080/_api/v1/upload-path").strip()
          assert malformed_status == "400", f"expected HTTP 400, got {malformed_status}"
          client.succeed("grep -F '\"code\":400' malformed-response.json")

      with subtest("Check that unknown upload compression fails cleanly"):
          unknown_status = client.succeed(f"printf broken | curl -sS -o unknown-response.json -w '%{{http_code}}' -X PUT -H 'Authorization: Bearer {root_token}' -H 'X-Attic-Nar-Compression: gzip' -H 'X-Attic-Nar-Info: {{}}' --data-binary @- http://server:8080/_api/v1/upload-path").strip()
          assert unknown_status == "400", f"expected HTTP 400, got {unknown_status}"
          client.succeed("grep -F '\"code\":400' unknown-response.json")

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

      with subtest("Check that compressed uploads can include the upload info in the payload"):
          client.succeed("${makeTestDerivation} test2.nix")
          test2_file = client.succeed("nix-build --no-out-link test2.nix").strip()
          proxy.succeed("truncate -s 0 /var/log/nginx/compressed.log")
          client.succeed(f"attic push --force-preamble compressed:test {test2_file}")
          proxy.succeed("grep -E '^PUT /_api/v1/upload-path zstd$' /var/log/nginx/compressed.log")
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
