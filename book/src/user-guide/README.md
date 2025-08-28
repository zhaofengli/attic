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
