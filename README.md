<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/banner-dark.svg">
  <img src="assets/banner-light.svg" alt="Model-Downloader" width="100%">
</picture>

# Model-Downloader

A model downloader in Rust. It fetches models and datasets from the Hugging Face Hub, from ModelScope, and (opt-in, over BitTorrent) from Pirate Face, with resumable, verified transfers. It installs two binaries, `model-downloader` and `mdl`, which are the same program.

<p align="center"><a href="https://github.com/TheSyrianDev/LastAI">Last AI</a> · <a href="https://github.com/TheSyrianDev/LastHarness">LastHarness</a> · <a href="https://github.com/TheSyrianDev/LastEngine">LastEngine</a> · Model-Downloader</p>

> [!NOTE]
> **Beta.** It works and has tests, but it has had little use outside the author's machine. Report problems on the issue tracker.

## Install

```sh
cargo install --path crates/hfd-cli
```

Needs Rust 1.88 or newer.

## Usage

```sh
mdl download <repo-id | hf:// URI | web URL> [paths...]
mdl download Qwen/Qwen3-0.6B --include "*.safetensors" --local-dir ./model
mdl download <repo> <file.gguf> --local-dir ./models --flat   # files land by basename, no subfolders
mdl download <repo> --dry-run          # resolve files and print the plan
```

Useful flags: `--source hf|modelscope|pirateface`, `--type` (model, dataset, space), `--revision`, `--include` / `--exclude` (globs, repeatable), `--cache-dir`, `--local-dir`, `--flat`, `--force-download`, `--verify auto|always|never`, `--max-workers`, `--connections`, `--max-speed` (e.g. `5M`), `--engine auto|http|xet`, `--json`, `--quiet`.

## Where the files come from

Hugging Face is the default (`--source hf`). Two other sources are chosen explicitly; there is no silent fallback between them.

**ModelScope** (`--source modelscope`) lists the repository through ModelScope's own API and downloads each file over HTTPS, checking the sha256 the hub published:

```sh
mdl download Qwen/Qwen3-0.6B --source modelscope --local-dir ./model
mdl download AI-ModelScope/ShareGPT-4o --source modelscope --type dataset --dry-run
```

The revision defaults to `master`, which is ModelScope's default branch. `MODELSCOPE_API` points the source at a mirror, and `MODELSCOPE_API_TOKEN` (or `MODELSCOPE_TOKEN`) is sent for private repositories. Files without a published digest cannot be verified, so they can go to `--local-dir` but not into the cache.

**Pirate Face** (`--source pirateface`) is opt-in, needs `aria2c` and `--local-dir`, and verifies every file against the hashes in the listing.

Plain HTTP is the default engine. Xet is opt-in (`--engine auto` or `--engine xet`); if Xet stalls, the downloader falls back to HTTP and skips Xet on that host for 24 hours.

Downloads can run through a background daemon with a persistent queue:

```sh
mdl download <repo> --detach           # enqueue and return immediately
mdl queue list | show <id> | pause <id> | resume <id> | cancel <id> | clear
mdl daemon                             # run the daemon in the foreground
```

## Authentication

```sh
mdl auth login          # token, or the browser device flow when no token is given
mdl auth whoami
mdl auth list
mdl auth token          # prints the active token to stdout, like `hf auth token`
mdl auth logout
```

The token is read from `--token`, then `HF_TOKEN` / `HUGGING_FACE_HUB_TOKEN`, then stored tokens.

## The rest of the suite

Model-Downloader is the download half of a four-part, local-first AI suite. Each part is its own repository, release and set of limits:

- [Last AI](https://github.com/TheSyrianDev/LastAI) — the studio: a desktop app and a terminal client over a local core.
- [LastHarness](https://github.com/TheSyrianDev/LastHarness) — the harness: the turn loop, tools, memory, team and spend ledger, with no downloader and no engine.
- [LastEngine](https://github.com/TheSyrianDev/LastEngine) — inference in Rust behind one backend trait.
- Model-Downloader — this repository. Last AI's model search and the `mdl lastengine` command use it.

## License

Copyright (c) 2026 Omar Barood.

This repository uses two licenses:

- **`hfd-core`** (the download library in `crates/hfd-core`) is licensed under the [Apache License 2.0](crates/hfd-core/LICENSE). You can use it in any project, including commercial ones.
- **Everything else**, including the `model-downloader` / `mdl` command-line tool in `crates/hfd-cli`, is licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE). Noncommercial use, modification and redistribution are permitted. Commercial use needs a separate written agreement; see [COMMERCIAL-LICENSING.md](COMMERCIAL-LICENSING.md).
