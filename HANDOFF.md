# Handoff (2026-11-12)

State:
- hfd v0.2.0 released (GitHub release, sha256 verified). Mirror main b2e3012.
- Local commit ca747fe: queue pause/resume + Paused status. Not pushed to main.
- Branch ci-windows-check on the public mirror (run 43273737889): check, ubuntu, windows green; macOS was still running at last check.
- Rig repo: registry/downloads.json committed locally as 38abd4d.

Not started:
- Clean-machine install test (tests/clean-install/, docker only; podman is not installed). Needs a tiny GGUF (candidate: a ~100MB file from bartowski/SmolLM2-135M-Instruct-GGUF, not yet verified).

Rules: commit tested work locally; push, public release, and merge to main need Omar's OK.
