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

## Stateless Login

`attic login` modifies local state. In cases where you don’t want to persist state--such as CI--you can use environment variables instead.

Supported ENVs:

- `ATTIC_LOGIN_ENDPOINT`: Server URL. When set, requires `ATTIC_LOGIN_NAME`.
- `ATTIC_LOGIN_NAME`: Name for the `ATTIC_LOGIN_ENDPOINT`.
- `ATTIC_LOGIN_TOKEN` or `ATTIC_LOGIN_TOKEN_FILE`: Inline token value or path to a file containing the token. When both are set, `ATTIC_LOGIN_TOKEN_FILE` takes precedence.
- `ATTIC_LOGIN_FORCE_DEFAULT`: If set to any value, forces the server specified by `ATTIC_LOGIN_NAME` to be the default server.

Example:

```bash
ATTIC_LOGIN_NAME=test \
ATTIC_LOGIN_ENDPOINT=https://attic.example.com \
ATTIC_LOGIN_TOKEN=eyJ... \
attic push test:foo paths
```

## Enabling a cache

To configure Nix to automatically use cache `foo`:

```
attic use foo
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
