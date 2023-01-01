# Tests

The included tests require trusted user access to import the test NAR dumps.

## Test Derivations

To keep things minimal, we have a couple of polyglot derivations that double as their builders in `drv`.
They result in the following store paths when built:

- `no-deps.nix` -> `/nix/store/nm1w9sdm6j6icmhd2q3260hl1w9zj6li-attic-test-no-deps`
- `with-deps.nix` -> `/nix/store/7wp86qa87v2pwh6sr2a02qci0h71rs9z-attic-test-with-deps`

NAR dumps for those store paths are included in `nar`.
`.nar` files are produced by `nix-store --export`, and `.export` files are produced by `nix-store --export`.
