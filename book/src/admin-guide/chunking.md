# Chunking

Attic uses the [FastCDC algorithm](https://www.usenix.org/conference/atc16/technical-sessions/presentation/xia) to split uploaded NARs into chunks for deduplication.
There are four main parameters that control chunking in Attic:

- `nar-size-threshold`: The minimum NAR size to trigger chunking
    - When set to 0, chunking is disabled entirely for newly-uploaded NARs
    - When set to 1, all newly-uploaded NARs are chunked
- `min-size`: The preferred minimum size of a chunk, in bytes
- `avg-size`: The preferred average size of a chunk, in bytes
- `max-size`: The preferred maximum size of a chunk, in bytes

## Configuration

When upgrading from an older version without support for chunking, you must include the new `[chunking]` section:

```toml
# Data chunking
#
# Warning: If you change any of the values here, it will be
# difficult to reuse existing chunks for newly-uploaded NARs
# since the cutpoints will be different. As a result, the
# deduplication ratio will suffer for a while after the change.
[chunking]
# The minimum NAR size to trigger chunking
#
# If 0, chunking is disabled entirely for newly-uploaded NARs.
# If 1, all newly-uploaded NARs are chunked.
nar-size-threshold = 131072 # chunk files that are 128 KiB or larger

# The preferred minimum size of a chunk, in bytes
min-size = 65536            # 64 KiB

# The preferred average size of a chunk, in bytes
avg-size = 131072           # 128 KiB

# The preferred maximum size of a chunk, in bytes
max-size = 262144           # 256 KiB
```
