# Tutorial

Let's spin up Attic in just 15 minutes (yes, it works on macOS too!):

```bash
nix-shell https://github.com/zhaofengli/attic/tarball/main -A demo
```

Simply run `atticd` to start the server in monolithic mode with a SQLite database and local storage:

```console
$ atticd
Attic Server 0.1.0 (release)

-----------------
Welcome to Attic!

A simple setup using SQLite and local storage has been configured for you in:

    /home/zhaofeng/.config/attic/server.toml

Run the following command to log into this server:

    attic login local http://localhost:8080 eyJ...

Documentations and guides:

    https://docs.attic.rs

Enjoy!
-----------------

Running migrations...
Starting API server...
Listening on [::]:8080...
```

## Cache Creation

`atticd` is the server, and `attic` is the client.
We can now log in and create a cache:

```console
# Copy and paste from the atticd output
$ attic login local http://localhost:8080 eyJ...
✍️ Configuring server "local"

$ attic cache create hello
✨ Created cache "hello" on "local"
```

## Pushing

Let's push `attic` itself to the cache:

```console
$ attic push hello $(which attic)
⚙️ Pushing 1 paths to "hello" on "local" (0 already cached, 45 in upstream)...
✅ r5d7217c0rjd5iiz1g2nhvd15frck9x2-attic-0.1.0 (52.89 MiB/s)
```

The interesting thing is that `attic` automatically skipped over store paths cached by `cache.nixos.org`!
This behavior can be configured on a per-cache basis.

Note that Attic performs content-addressed global deduplication, so when you upload the same store path to another cache, the underlying NAR is only stored once.
Each cache is essentially a restricted view of the global cache.

## Pulling

Now, let's pull it back from the cache.
For demonstration purposes, let's use `--store` to make Nix download to another directory because Attic already exists in `/nix/store`:

```console
# Automatically configures ~/.config/nix/nix.conf for you
$ attic use hello
Configuring Nix to use "hello" on "local":
+ Substituter: http://localhost:8080/hello
+ Trusted Public Key: hello:vlsd7ZHIXNnKXEQShVnd7erE8zcuSKrBWRpV6zTibnA=
+ Access Token

$ nix-store --store $PWD/nix-demo -r $(which attic)
[snip]
copying path '/nix/store/r5d7217c0rjd5iiz1g2nhvd15frck9x2-attic-0.1.0' from 'http://localhost:8080/hello'...
warning: you did not specify '--add-root'; the result might be removed by the garbage collector
/nix/store/r5d7217c0rjd5iiz1g2nhvd15frck9x2-attic-0.1.0

$ ls nix-demo/nix/store/r5d7217c0rjd5iiz1g2nhvd15frck9x2-attic-0.1.0/bin/attic
nix-demo/nix/store/r5d7217c0rjd5iiz1g2nhvd15frck9x2-attic-0.1.0/bin/attic
```

Note that to pull into the actual Nix Store, your user must be considered [trusted](https://nixos.org/manual/nix/stable/command-ref/conf-file.html#conf-trusted-users) by the `nix-daemon`.

## Access Control

Attic performs stateless authentication using signed JWT tokens which contain permissions.
The root token printed out by `atticd` is all-powerful and should not be shared.

Let's create another token that can only access the `hello` cache:

```console
$ atticadm make-token --sub alice --validity '3 months' --pull hello --push hello
eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhbGljZSIsImV4cCI6MTY4MDI5MzMzOSwiaHR0cHM6Ly9qd3QuYXR0aWMucnMvdjEiOnsiY2FjaGVzIjp7ImhlbGxvIjp7InIiOjEsInciOjF9fX19.XJsaVfjrX5l7p9z76836KXP6Vixn41QJUfxjiK7D-LM
```

Let's say Alice wants to have her own caches.
Instead of creating caches for her, we can let her do it herself:

```console
$ atticadm make-token --sub alice --validity '3 months' --pull 'alice-*' --push 'alice-*' --create-cache 'alice-*'
eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhbGljZSIsImV4cCI6MTY4MDI5MzQyNSwiaHR0cHM6Ly9qd3QuYXR0aWMucnMvdjEiOnsiY2FjaGVzIjp7ImFsaWNlLSoiOnsiciI6MSwidyI6MSwiY2MiOjF9fX19.MkSnK6yGDWYUVnYiJF3tQgdTlqstfWlbziFWUr-lKUk
```

Now Alice can use this token to _create_ any cache beginning with `alice-` and push to them.
Try passing `--dump-claims` to show the JWT claims without encoding the token to see what's going on.

## Going Public

Let's make the cache public. Making it public gives unauthenticated users pull access:

```console
$ attic cache configure hello --public
✅ Configured "hello" on "local"

# Now we can query the cache without being authenticated
$ curl http://localhost:8080/hello/nix-cache-info
WantMassQuery: 1
StoreDir: /nix/store
Priority: 41
```

## Garbage Collection

It's a bad idea to let binary caches grow unbounded.
Let's configure garbage collection on the cache to automatically delete objects that haven't been accessed in a while:

```
$ attic cache configure hello --retention-period '1s'
✅ Configured "hello" on "local"
```

Now the retention period is only one second.
Instead of waiting for the periodic garbage collection to occur (see `server.toml`), let's trigger it manually:

```bash
atticd --mode garbage-collector-once
```

Now the store path doesn't exist on the cache anymore!

```console
$ nix-store --store $PWD/nix-demo-2 -r $(which attic)
don't know how to build these paths:
  /nix/store/v660wl07i1lcrrgpr1yspn2va5d1xgjr-attic-0.1.0
error: build of '/nix/store/v660wl07i1lcrrgpr1yspn2va5d1xgjr-attic-0.1.0' failed

$ curl http://localhost:8080/hello/v660wl07i1lcrrgpr1yspn2va5d1xgjr.narinfo
{"code":404,"error":"NoSuchObject","message":"The requested object does not exist."}
```

Let's reset it back to the default, which is to not garbage collect (configure it in `server.toml`):

```console
$ attic cache configure hello --reset-retention-period
✅ Configured "hello" on "local"

$ attic cache info hello
               Public: true
           Public Key: hello:vlsd7ZHIXNnKXEQShVnd7erE8zcuSKrBWRpV6zTibnA=
Binary Cache Endpoint: http://localhost:8080/hello
         API Endpoint: http://localhost:8080/
      Store Directory: /nix/store
             Priority: 41
  Upstream Cache Keys: ["cache.nixos.org-1"]
     Retention Period: Global Default
```

Because of Attic's global deduplication, garbage collection actually happens on three levels:

1. **Local Cache**: When an object is garbage collected, only the mapping between the metadata in the local cache and the NAR in the global cache gets deleted. The local cache loses access to the NAR, but the storage isn't freed.
2. **Global NAR Store**: Orphan NARs not referenced by any local cache then become eligible for deletion.
3. **Global Chunk Store**: Finally, orphan chunks not referenced by any NAR become eligible for deletion. This time the storage space is actually freed and subsequent uploads of the same chunk will actually trigger an upload to the storage backend.

## Summary

In just a few commands, we have:

1. Set up a new Attic server and a binary cache
2. Pushed store paths to it
3. Configured Nix to use the new binary cache
4. Generated access tokens that provide restricted access
5. Made the cache public
6. Performed garbage collection

## What's next

> Note: Attic is an early prototype and everything is subject to change! It may be full of holes and APIs may be changed without backward-compatibility. You might even be required to reset the entire database. I would love to have people give it a try, but please keep that in mind ️:)

For a less temporary setup, you can set up `atticd` with PostgreSQL and S3.
You should also place it behind a load balancer like NGINX to provide HTTPS.
Take a look at `~/.config/attic/server.toml` to see what you can configure!

While it's easy to get started by running `atticd` in monolithic mode, for production use it's best to run different components of `atticd` separately with `--mode`:

- `api-server`: Stateless and can be replicated.
- `garbage-collector`: Performs periodic garbage collection. Cannot be replicated.
