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

On NixOS you can add the cache declaratively in your `configuration.nix`:
```
nix.settings = {
  substituters = [
    "BINARY_CACHE_ENDPOINT"
  ];
  trusted-public-keys = [
    "CACHE_PUBLIC_KEY"
  ];
};
```

To view the binary cache endpoint and cache public key for `foo`:
```
attic cache info foo
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
