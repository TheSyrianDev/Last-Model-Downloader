# Changelog

## Unreleased

### Added
- `hfd queue pause <id>` and `hfd queue resume <id>`. A paused queued job is skipped by the worker. A paused running job is stopped, and its partial files stay on disk, so resume continues them. Paused jobs can still be cancelled.
- Status `paused` in `hfd queue list --json` and `hfd queue show <id> --json` for polling.

## 0.2.0

### Changed
- **HTTP is the default engine.** Xet is used only with `--engine auto` or `--engine xet`.
- **Xet stall fallback.** If Xet gets no data for 10 s, the download switches to HTTP. The host is then skipped for Xet for 24 h, so later downloads go straight to HTTP.
- **Fit rule matches the registry.** A model fits if its size is at most free RAM. It is "tight" if up to 10% over, and too big beyond that. VRAM is not counted.

### Added
- **sha256 in the download log.** LFS files record the Hub's sha256.
- **Models API in `hfd-core`.** `search_models`, `repo_quants` (shards merged, mmproj split out), `group_quants`, `MemoryBudget`/`fit_for`. Documented in `docs/EMBEDDING.md`.
- **Download log** (`HFD_DOWNLOAD_LOG` or `DownloadOptions.download_log`). Records each completed download in a JSON registry.
- **`--flat`** (requires `--local-dir`). Writes each file by basename, with no repo subfolders.
- **Queue `flat` field**, optional and default false, so older queue files still load.
- **Listing retries.** Hub listing requests retry up to 3 times on transport errors, with backoff.

### Fixed
- **Hub rename redirects are followed** on HEAD and size probes (HTTP 302 used to fail).
- Quant table now includes `UD-Q2_K_XL` and related unsloth dynamic types.
- Segment requests no longer use a total timeout that killed slow pieces.
