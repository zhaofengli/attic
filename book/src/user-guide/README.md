# User Guide

## Logging in

You should have received an `attic login` command from an admin like the following:

```
attic login central https://attic.domain.tld/ eyJ...
```

The `attic` client can work with multiple servers at the same time.
To select the `foo` cache from server `central`, use one of the following:

- `foo`, if the `central` server is configured as the default
- `central:foo`

To configure the default server, set `default-server` in `~/.config/attic/config.toml`.

## Environment Variables Login

`attic login` normally takes arguments.
In cases where you want/need to run commands unattended, such as CI, you can use environment variables instead.

Supported ENVs:

- `ATTIC_LOGIN_ENDPOINT`: Server URL. When set, requires `ATTIC_LOGIN_NAME`.
- `ATTIC_LOGIN_NAME`: Name for the `ATTIC_LOGIN_ENDPOINT`.
- `ATTIC_LOGIN_TOKEN`: Inline token value.
- `ATTIC_LOGIN_FORCE_DEFAULT`: If set to any value, forces the server specified by `ATTIC_LOGIN_NAME` to be the default server.

Example:

```bash
ATTIC_LOGIN_NAME=test \
ATTIC_LOGIN_ENDPOINT=https://attic.example.com \
ATTIC_LOGIN_TOKEN=eyJ... \
attic login
# Now that you have been logged in you can now use any other command.
attic push test:foo paths
```

## Enabling a cache

To configure Nix to automatically use cache `foo`:

```
attic use foo
```

This adds the binary cache to your `~/.config/nix/nix.conf` and configures the credentials required to access it.

If you wish to configure Nix manually, you can view the binary cache endpoint and the cache public key:

```console
$ attic cache info foo
               Public: true
           Public Key: foo:WcnO6s4aVkB6CKRaPPpKvHLZykWXASV6c+/Ssg8uQEY=
Binary Cache Endpoint: https://attic.domain.tld/foo
      Store Directory: /nix/store
             Priority: 41
  Upstream Cache Keys: ["cache.nixos.org-1"]
     Retention Period: Global Default
```

On NixOS, you can configure the cache declaratively in your system configuration with the above information:

```nix
{
  nix.settings = {
    substituters = [
      "https://attic.domain.tld/foo"
    ];
    trusted-public-keys = [
      "foo:WcnO6s4aVkB6CKRaPPpKvHLZykWXASV6c+/Ssg8uQEY="
    ];
  };
}
```

## Disabling a cache

To configure Nix to no longer use a cache, remove the corresponding entries from the list of `substituters` and `trusted-public-keys` in `~/.config/nix/nix.conf`

## Pushing to the cache

To push a store path to cache `foo`:

```bash
attic push foo /nix/store/...
```

Other examples include:

```bash
attic push foo ./result
attic push foo /run/current-system
```
