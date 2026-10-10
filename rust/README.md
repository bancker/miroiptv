# tvplayer

The Rust rewrite of [miroiptv](../c/): a clean rebuild of the same IPTV
player on libmpv + egui/eframe (wgpu) + tokio. This is the active
codebase; the C app in [`../c/`](../c/) stays as a fallback while this one
is validated.

- Design: [`docs/superpowers/specs/2026-05-22-rust-tvplayer-design.md`](../docs/superpowers/specs/2026-05-22-rust-tvplayer-design.md)
- Implementation plan: [`docs/superpowers/plans/2026-05-22-rust-tvplayer.md`](../docs/superpowers/plans/2026-05-22-rust-tvplayer.md)
- Manual validation checklist: [`docs/VALIDATION.md`](../docs/VALIDATION.md)

## Build

Run from this folder (`rust/`):

```powershell
.\vendor\fetch-mpv.ps1           # one-time: vendor libmpv-2.dll + headers
cargo test                       # unit + integration tests
cargo build --release            # -> target\release\tvplayer.exe
.\target\release\tvplayer.exe --selftest
```

## Portable zip

```powershell
.\build-zip.ps1                  # -> dist\tvplayer-vX.Y.Z.zip
.\verify.ps1                     # smoke-test the zip
```

All build output stays in this folder: `target\` (cargo) and `dist\`
(portable zips). End-user docs are inside the zip's `README.txt` after
extraction.
