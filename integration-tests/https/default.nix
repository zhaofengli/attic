{ pkgs, ... }:

let
  certificates = pkgs.runCommand "attic-test-certificates" {
    nativeBuildInputs = [ pkgs.openssl ];
  } ''
    mkdir -p "$out"

    for name in system custom; do
      openssl req -x509 -newkey rsa:2048 -nodes -days 36500 \
        -subj "/CN=$name CA" -addext basicConstraints=critical,CA:TRUE \
        -keyout "$name-ca.key" -out "$out/$name-ca.pem"

      openssl req -new -newkey rsa:2048 -nodes -subj /CN=localhost \
        -addext subjectAltName=DNS:localhost \
        -keyout "$out/$name.key" -out "$name.csr"
      openssl x509 -req -in "$name.csr" -days 36500 -copy_extensions copy \
        -CA "$out/$name-ca.pem" -CAkey "$name-ca.key" -CAcreateserial \
        -out "$out/$name.pem"
    done

    cat "$out/system-ca.pem" "$out/custom-ca.pem" > "$out/both.pem"
  '';

  # A cache-config response is enough to exercise the CLI's HTTPS client.
  response = pkgs.writeTextDir "_api/v1/cache-config/test" (builtins.toJSON {
    is_public = true;
    store_dir = "/nix/store";
  });

  httpsHost = name: port: {
    onlySSL = true;
    listen = [{ addr = "127.0.0.1"; inherit port; ssl = true; }];
    sslCertificate = "${certificates}/${name}.pem";
    sslCertificateKey = "${certificates}/${name}.key";
    root = response;
  };
in {
  name = "https";

  nodes.machine = {
    environment.systemPackages = [ pkgs.attic-client ];
    security.pki.certificateFiles = [ "${certificates}/system-ca.pem" ];

    services.nginx = {
      enable = true;
      virtualHosts = {
        system = httpsHost "system" 8443;
        custom = httpsHost "custom" 9443;
      };
    };

    environment.etc = {
      "empty.pem".text = "";
      "non-certificate.pem".text = "not a certificate";
      "malformed.pem".text = ''
        -----BEGIN CERTIFICATE-----
        invalid base64!
        -----END CERTIFICATE-----
      '';
      "invalid-der.pem".text = ''
        -----BEGIN CERTIFICATE-----
        Zm9v
        -----END CERTIFICATE-----
      '';
    };
  };

  testScript = ''
    import shlex

    system_ca = "${certificates}/system-ca.pem"
    custom_ca = "${certificates}/custom-ca.pem"
    both_cas = "${certificates}/both.pem"

    def check_cache(server, *, error=None, **environment):
        # Clear inherited overrides so each case controls its trust settings.
        command = ["env", "-u", "NIX_SSL_CERT_FILE", "-u", "SSL_CERT_FILE", "-u", "SSL_CERT_DIR"]
        command += [f"{name}={value}" for name, value in environment.items()]
        command += ["attic", "cache", "info", f"{server}:test"]
        status, output = machine.execute(shlex.join(command) + " 2>&1")

        if error is None:
            assert status == 0, output
            assert "Store Directory: /nix/store" in output, output
        else:
            assert status != 0, output
            assert error in output, output

    machine.start()
    machine.wait_for_unit("nginx.service")
    machine.succeed("attic login system https://localhost:8443/")
    machine.succeed("attic login custom https://localhost:9443/")

    with subtest("Default trust is preserved when NIX_SSL_CERT_FILE is unset"):
        check_cache("system")
        check_cache("custom", error="certificate")

    with subtest("SSL_CERT_FILE still works when NIX_SSL_CERT_FILE is unset"):
        check_cache("custom", SSL_CERT_FILE=custom_ca)
        check_cache("custom", SSL_CERT_FILE=system_ca, error="certificate")

    with subtest("NIX_SSL_CERT_FILE replaces the default roots"):
        check_cache("custom", NIX_SSL_CERT_FILE=custom_ca)
        check_cache("system", NIX_SSL_CERT_FILE=custom_ca, error="certificate")

    with subtest("All certificates in the bundle are trusted"):
        check_cache("system", NIX_SSL_CERT_FILE=both_cas)
        check_cache("custom", NIX_SSL_CERT_FILE=both_cas)

    with subtest("NIX_SSL_CERT_FILE takes precedence over SSL_CERT_FILE"):
        check_cache("custom", NIX_SSL_CERT_FILE=custom_ca, SSL_CERT_FILE=system_ca)
        check_cache(
            "custom", NIX_SSL_CERT_FILE=system_ca, SSL_CERT_FILE=custom_ca,
            error="certificate",
        )

    with subtest("Invalid overrides fail without falling back to other roots"):
        for path in ["", "/missing.pem"]:
            check_cache(
                "system", NIX_SSL_CERT_FILE=path, SSL_CERT_FILE=system_ca,
                error="Failed to read NIX_SSL_CERT_FILE",
            )

        for path in ["/etc/empty.pem", "/etc/non-certificate.pem"]:
            check_cache("system", NIX_SSL_CERT_FILE=path, error="contains no certificates")

        check_cache(
            "system", NIX_SSL_CERT_FILE="/etc/malformed.pem",
            error="Failed to parse NIX_SSL_CERT_FILE",
        )
        check_cache(
            "system", NIX_SSL_CERT_FILE="/etc/invalid-der.pem",
            error="Failed to build HTTP client",
        )
  '';
}
