# Sheaf

Local, offline PDF and scan toolkit for Windows (Rust + Tauri 2). No network at runtime.

The design and the phase-by-phase build plan are in [ARCHITECTURE.md](ARCHITECTURE.md);
start with its reading guide and the rules for whoever writes code.

**Status:** PH-0 (scaffold). The crates are empty; nothing is usable yet.

## Working on it

```bash
cargo test --workspace             # includes the module-dependency test
cargo run -p link-check            # spec cross-links and generated appendices
cargo run -p link-check -- --write # regenerate the appendices after editing the spec
bash tools/fetch-vendor            # download and verify pinned third-party assets
```
