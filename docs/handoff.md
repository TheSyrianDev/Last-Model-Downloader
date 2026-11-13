# hf-dl handoff (2026-11-13)

## State
- Public mirror `main` at `b7d030b`, CI green on all five jobs. Tag `v0.4.0-rc.1` and the GitHub prerelease are up.
- Fresh single-commit copy for the rename: `/home/omar/Projects/public/hf-dl-fresh` (64 files, fmt/clippy clean, 91 tests pass). Not pushed. GitHub repos not deleted.
- Private repo `~/Projects/hf-dl` HEAD `b927173` plus this file.

## Open items
- Public `lastengine.rs` still defaults to `~/AI/Rig/bin/hw-probe`. Owner decision pending.
- FLUX.2-dev autoencoder: official repo is gated (403). Mirror `ai-toolkit/flux2_vae` is in use at `~/AI/Models/idol/flux2-vae/ae.safetensors` (sha256 matches the local copy). Keep unless Omar gets gated access.
- DINOv3: unofficial mirror `bschooled/dinov3-vith16plus-pretrain-lvd1689m` at `~/AI/Models/idol/dinov3-vith16plus/`. Official repo still gated.
- Docs: `hfd-core` `#![warn(missing_docs)]` pass. Queue done; http, hub, error, cache, state and the rest not done (about 373 undocumented pub items at last count).
- Pirate Face: aria2c shell-out is in. Live transfers are not tested, by design.

## Notes
- Gated downloads must not be bypassed. Use a mirror only with Omar's approval and log the provenance in `~/AI/Rig/registry/downloads.json`.
- Clean partial `.incomplete` and `.lock` files before retrying a download.
