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
