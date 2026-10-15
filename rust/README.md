# Bongo Player

The Rust rewrite of [miroiptv](../c/) (formerly `tvplayer`): a clean
rebuild of the same IPTV player on libmpv + egui/eframe (wgpu) + tokio.
This is the active codebase; the C app in [`../c/`](../c/) stays as a
fallback while this one is validated.

- Design: [`docs/superpowers/specs/2026-05-22-rust-tvplayer-design.md`](../docs/superpowers/specs/2026-05-22-rust-tvplayer-design.md)
- Implementation plan: [`docs/superpowers/plans/2026-05-22-rust-tvplayer.md`](../docs/superpowers/plans/2026-05-22-rust-tvplayer.md)
- Manual validation checklist: [`docs/VALIDATION.md`](../docs/VALIDATION.md)

## Build

Run from this folder (`rust/`):

```powershell
.\vendor\fetch-mpv.ps1           # one-time: vendor libmpv-2.dll + headers
cargo test                       # unit + integration tests
cargo build --release            # -> target\release\BongoPlayer.exe
.\target\release\BongoPlayer.exe --selftest
```

## Portable zip

```powershell
.\build-zip.ps1                  # -> dist\BongoPlayer-vX.Y.Z.zip
.\verify.ps1                     # smoke-test the zip
```

All build output stays in this folder: `target\` (cargo) and `dist\`
(portable zips). End-user docs are inside the zip's `README.txt` after
extraction.

## Where the app keeps its data

- Sources: `BongoPlayer.ini` next to the exe holds every source, Xtream portals
  and M3U lists (a URL or a file), and which one is in use. The first-start
  prompt writes the first portal; Settings (`o`, Bronnen) adds, changes,
  deletes and switches sources. The 0.6.1 line `portal=…` stays, so an older
  version still starts on the portal. A real `--xtream` on the command line
  beats the ini on every start, and Bronnen says so when it does. M3U
  addresses can hold passwords: they stay in the ini, never in a log.
- Saved M3U lists: `cache\m3u-<id>.m3u` in the config folder, shown at once on
  the next start while the fresh list downloads.
- Favorites, presets and the last channel are kept per source: source 1 in
  `favorites.json`, `presets.json`, `last_watched.json`, another source in
  `favorites-<id>.json` and so on.
- Preferences: `settings.json` in `%APPDATA%\BongoPlayer\config\`. It holds the
  language, reversed zapping, the adult-content PIN (a salted hash, never the
  PIN) and the news rules; a file that is missing or broken gives the defaults.
- Favorites, resume points, presets and logs: `%APPDATA%\BongoPlayer\config\`.

On the first start after the rename, data from the old `tvplayer`
locations (`%APPDATA%\tvplayer\config\`, `tvplayer.ini`) is carried over
automatically; the old config folder stays as a backup.
