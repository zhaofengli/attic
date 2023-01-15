# FAQs

<!-- TODO: Write more about design decisions in a separate section -->

## Does it replace [Cachix](https://www.cachix.org)?

No, it does not.
Cachix is an awesome product and the direct inspiration for the user experience of Attic.
It works at a much larger scale than Attic and is a proven solution.
Numerous open-source projects in the Nix community (including mine!) use Cachix to share publicly-available binaries.

Attic can be thought to provide a similar user experience at a much smaller scale (personal or team use).

## What happens if a user uploads a path that is already in the global cache?

The user will still fully upload the path to the server because they have to prove possession of the file.
The difference is that instead of having the upload streamed to the storage backend (e.g., S3), it's only run through a hash function and discarded.
Once the NAR hash is confirmed, a mapping is created to grant the local cache access to the global NAR.
The global deduplication behavior is transparent to the client.

This requirement may be disabled by setting `require-proof-of-possession` to false in the configuration.
When disabled, uploads of NARs that already exist in the Global NAR Store will immediately succeed.

## What happens if a user uploads a path with incorrect/malicious metadata?

They will only pollute their own cache.
Path metadata (store path, references, deriver, etc.) are associated with the local cache and the global cache only contains content-addressed NARs and chunks that are "context-free."

## How is authentication handled?

Authentication is done via signed JWTs containing the allowed permissions.
Each instance of `atticd --mode api-server` is stateless.
This design may be revisited later, with option for a more stateful method of authentication.

## On what granularity is deduplication done?

Global deduplication is done on two levels: NAR files and chunks.
During an upload, the NAR file is split into chunks using the [FastCDC algorithm](https://www.usenix.org/system/files/conference/atc16/atc16-paper-xia.pdf).
Identical chunks are only stored once in the storage backend.
If an identical NAR exists in the Global NAR Store, chunking is skipped and the NAR is directly deduplicated.

During a download, `atticd` reassembles the entire NAR from constituent chunks by streaming from the storage backend.

Data chunking is optional and can be disabled entirely for NARs smaller than a threshold.
When chunking is disabled, all new NARs are uploaded as a single chunk and NAR-level deduplication is still in effect.

## Why chunk NARs instead of individual files?

In the current design, chunking is applied to the entire uncompressed NAR file instead of individual constituent files in the NAR.
Big NARs that benefit the most from chunk-based deduplication (e.g., VSCode, Zoom) often have hundreds or thousands of small files.
During NAR reassembly, it's often uneconomical or impractical to fetch thousands of files to reconstruct the NAR in a scalable way.
By chunking the entire NAR, it's possible to configure the average chunk size to a larger value, ignoring file boundaries and lumping small files together.
This is also the approach [`casync`](https://github.com/systemd/casync) has taken.

You may have heard that [the Tvix store protocol](https://flokli.de/posts/2022-06-30-store-protocol/) chunks individual files instead of the NAR.
The design of Attic is driven by the desire to effectively utilize existing platforms with practical limitations, while looking forward to the future.

## What happens if a chunk is corrupt/missing?

When a chunk is deleted from the database, all dependent `.narinfo` and `.nar` will become unavailable (503).
However, this can be recovered from automatically when any NAR containing the chunk is uploaded.

At the moment, Attic cannot automatically detect when a chunk is corrupt or missing.
Correctly distinguishing between transient and persistent failures is difficult.
The `atticadm` utility will have the functionality to kill/delete bad chunks.

## How is compression handled?

Uploaded NARs are chunked then compressed on the server before being streamed to the storage backend.
On the chunk level, we use the hash of the _uncompressed chunk_ to perform global deduplication.

```
                        ┌───────────────────────────────────►Chunk Hash
                        │
                        │
                        ├───────────────────────────────────►Chunk Size
                        │
                ┌───────┴────┐  ┌──────────┐  ┌───────────┐
 Chunk Stream──►│Chunk Hasher├─►│Compressor├─►│File Hasher├─►File Stream─►S3
                └────────────┘  └──────────┘  └─────┬─────┘
                                                    │
                                                    ├───────►File Hash
                                                    │
                                                    │
                                                    └───────►File Size
```
