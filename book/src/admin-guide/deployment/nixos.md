# Deploying to NixOS

Attic provides [a NixOS module](https://github.com/zhaofengli/attic/blob/main/nixos/atticd.nix) that allows you to deploy the Attic Server on a NixOS machine.

## Prerequisites

1. A machine running NixOS
1. _(Optional)_ A dedicated bucket on S3 or a S3-compatible storage service
    - You can either [set up Minio](https://search.nixos.org/options?query=services.minio) or use a hosted service like [Backblaze B2](https://www.backblaze.com/b2/docs) and [Cloudflare R2](https://developers.cloudflare.com/r2).
1. _(Optional)_ A PostgreSQL database

## Generating the Credentials File

The RS256 JWT secret can be generated with the `openssl` utility:

```bash
openssl genrsa -traditional -out private_key.pem 4096
```

Create a file on the server containing the following contents:

```
ATTIC_SERVER_TOKEN_RS256_SECRET="output from openssl"
```

Ensure the file is only accessible by root.

## Importing the Module

You can import the module in one of two ways:

- Ad-hoc: Import the `nixos/atticd.nix` from [the repository](https://github.com/zhaofengli/attic).
- Flakes: Add `github:zhaofengli/attic` as an input, then import `attic.nixosModules.atticd`.

## Configuration

> Note: These options are subject to change.

```nix
{
  services.atticd = {
    enable = true;

    # Replace with absolute path to your credentials file
    credentialsFile = "/etc/atticd.env";

    settings = {
      listen = "[::]:8080";

      # Data chunking
      #
      # Warning: If you change any of the values here, it will be
      # difficult to reuse existing chunks for newly-uploaded NARs
      # since the cutpoints will be different. As a result, the
      # deduplication ratio will suffer for a while after the change.
      chunking = {
        # The minimum NAR size to trigger chunking
        #
        # If 0, chunking is disabled entirely for newly-uploaded NARs.
        # If 1, all NARs are chunked.
        nar-size-threshold = 64 * 1024; # 64 KiB

        # The preferred minimum size of a chunk, in bytes
        min-size = 16 * 1024; # 16 KiB

        # The preferred average size of a chunk, in bytes
        avg-size = 64 * 1024; # 64 KiB

        # The preferred maximum size of a chunk, in bytes
        max-size = 256 * 1024; # 256 KiB
      };
    };
  };
}
```

After the new configuration is deployed, the Attic Server will be accessible on port 8080.
It's highly recommended to place it behind a reverse proxy like [NGINX](https://nixos.wiki/wiki/Nginx) to provide HTTPS.

## Operations

The NixOS module installs the `atticd-atticadm` wrapper which runs the `atticadm` command as the `atticd` user.
Use this command to [generate new tokens](../../reference/atticadm-cli.md#atticadm-make-token) to be distributed to users.
