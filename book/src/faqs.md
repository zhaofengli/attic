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

In the future, schemes to prove data possession without fully uploading the file may be supported.

## What happens if a user uploads a path with incorrect/malicious metadata?

They will only pollute their own cache.
Path metadata (store path, references, deriver, etc.) are associated with the local cache and the global cache only contains content-addressed NARs that are "context-free."

## How is authentication handled?

Authentication is done via signed JWTs containing the allowed permissions.
Each instance of `atticd --mode api-server` is stateless.
This design may be revisited later, with option for a more stateful method of authentication.

## How is compression handled?

Uploaded NARs are compressed on the server before being streamed to the storage backend.
We use the hash of the _uncompressed NAR_ to perform global deduplication.

```
                    ┌───────────────────────────────────►NAR Hash
                    │
                    │
                    ├───────────────────────────────────►NAR Size
                    │
              ┌─────┴────┐  ┌──────────┐  ┌───────────┐
 NAR Stream──►│NAR Hasher├─►│Compressor├─►│File Hasher├─►File Stream─►S3
              └──────────┘  └──────────┘  └─────┬─────┘
                                                │
                                                ├───────►File Hash
                                                │
                                                │
                                                └───────►File Size
```

At first glance, performing compression on the client and deduplicating the result may sound appealing, but this approach isn't without problems:

1. Different compression algorithms and levels naturally lead to different results which can't be deduplicated
2. Even with the same compression algorithm, the results are often non-deterministic (number of compression threads, library version, etc.)

When we perform compression on the server and use the hashes of uncompressed NARs for lookups, non-determinism of compression is no longer a problem since we only compress once.

On the other hand, performing compression on the server leads to additional CPU usage, increasing compute costs and the need to scale.
Such design decisions are to be revisited later.

## On what granularity is deduplication done?

Currently, global deduplication is done on the level of NAR files.
File or chunk-level deduplication (e.g., casync) may be added later.
It remains to be seen how NAR reassembly can be done in a user-friendly yet economical manner.
On compute services, outbound traffic often isn't free while several S3-compatible storage services provide free egress (e.g., [Cloudflare R2](https://developers.cloudflare.com/r2/platform/pricing/)).
