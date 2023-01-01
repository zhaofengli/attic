# Nix Store Bindings

This directory contains a set of high-level Rust bindings to `libnixstore`, compatible with `async`/`await` semantics.
We currently target Nix 2.4+.

## Why?

With this wrapper, now you can do things like:

```rust
let store = NixStore::connect()?;
let store_path = store.parse_store_path("/nix/store/ia70ss13m22znbl8khrf2hq72qmh5drr-ruby-2.7.5")?;
let nar_stream = store.nar_from_path(store_path); # AsyncWrite
```
