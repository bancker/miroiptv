# miroiptv / tvplayer

A small, fast IPTV player for Windows, tuned for Xtream Codes portals
(Dutch NPO/RTL shortcuts baked in). One portable exe plus its DLLs, no
installer.

This repo holds two implementations of the same player. Each is a
self-contained project folder with its own sources, build scripts and
build output:

| Folder | App | Stack | Status |
|---|---|---|---|
| [`rust/`](rust/) | `tvplayer` | Rust · libmpv · egui/eframe (wgpu) · tokio | active |
| [`c/`](c/) | `miroiptv` | C · SDL2 · libav · libcurl | fallback |

See each folder's README for build instructions. Design docs and plans
for both live in [`docs/`](docs/).
