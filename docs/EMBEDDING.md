# Embedding hfd-core

`hfd-core` is the download library behind `hfd`. It is licensed under Apache-2.0 so applications can bundle it; the `hfd` CLI stays under PolyForm Noncommercial.

A complete, runnable example is in [`crates/hfd-core/examples/embed.rs`](../crates/hfd-core/examples/embed.rs):

```sh
cargo run -p hfd-core --example embed -- Qwen/Qwen2.5-0.5B-Instruct-GGUF Q4_K_M
```

## Dependency

```toml
hfd-core = { git = "https://github.com/TheSyrianDev/hf-dl" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

All download calls are async and need a multi-threaded Tokio runtime.

## Flow

1. **Client.** `Config::from_env()` reads the standard Hugging Face variables (`HF_HOME`, `HF_HUB_CACHE`, `HF_ENDPOINT`, …). `auth::resolve_token(None, &config)` finds a token in `HF_TOKEN`, `HUGGING_FACE_HUB_TOKEN`, or the saved login. Never log the token; use `auth::mask_token` to display it. Then `HubClient::new(config, token)`.
2. **List files.** `hub.list_tree(&RepoRef::new(RepoType::Model, "org/name"), "main")` returns every file with `path` and `size`.
3. **Pick a quant.** `parse_quant(path)` returns `Quant { name, bits_per_weight }` (for example `Q4_K_M`, `IQ3_S`, `BF16`), or `None`. `parse_shard(path)` returns `(part, total)` for split files like `-00002-of-00005.gguf`. Download every shard of a split model.
4. **Download.** `Downloader::new(hub, DownloadOptions { .. })`, then `download_file_cancellable(&repo, path, "main", &sink, &cancel)`. This returns `DownloadedFile { local_path, size, skipped, resumed_bytes, .. }`.

## Options (`DownloadOptions`)

| Field | Default | Meaning |
|---|---|---|
| `cache_dir` | HF hub cache | Where the standard `models--org--name` layout goes. It is shared with the `hf` CLI. |
| `local_dir` | `None` | Set this to write plain files into a folder instead, such as `~/AI/models/<name>/`. |
| `engine` | `Http` | `Http`, `Xet` or `Auto`. HTTP is the default until Xet stalls on slow links are fixed. |
| `connections` | 8 | The maximum number of parallel connections per file. It starts at 4 and adds more while speed improves. |
| `max_speed` | `None` | A speed cap in bytes per second, shared by all files. |
| `verify` | `Auto` | SHA-256 checking: `Auto`, `Always` or `Never`. |
| `force` | `false` | Download again even if the file is already complete. |

## Progress (`EventSink`)

`EventSink` is an `Arc<dyn Fn(Event) + Send + Sync>`. It is called from download tasks, so keep it fast; to update a UI, send to a channel. The events are:

- `FileStart { path, size, destination, resumed_bytes }`
- `Progress { path, downloaded, total, bytes_per_sec, eta_secs }`. This arrives about every 200 ms. Speed is averaged over about 5 seconds. `eta_secs` is `None` until the speed is known.
- `FileDone { path, local_path, size, skipped }`
- `Message(String)`: human-readable notices, such as an engine switch.

## Pause, cancel and resume

Create a `CancelToken` and call `token.cancel()` from anywhere. The download returns `Err(Error::Cancelled)`. Finished pieces stay on disk next to a `.hfd.state` file, so calling `download_file_cancellable` again for the same file resumes it. Pause is cancel followed by a later download.

Resume works in whole pieces of 8–64 MB, depending on file size. A piece that was only partly written when you cancelled is downloaded again.

## Errors

`hfd_core::Error` includes `Cancelled`, `Auth` (a gated or private repo that needs a token whose account accepted the repo's terms), `Http`, `Io` and `Xet`. Check free disk space before starting large files, though the downloader also checks it.

## Platforms

CI builds and tests on Linux, macOS and Windows. Real downloads have only been tested on Linux. On Windows, the cache uses hard links or copies when symlinks aren't allowed. The CLI's background daemon is Unix-only and is not part of the library.
