# Zenderlijst and defect marks for M3U sources — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On an M3U source, `g` opens a channel list (groups left, the chosen group's channels A–Z right) whose selection starts the channel behind it at the zap tempo; and a channel that fails before its first picture gets a defect mark (confirmed by another channel's picture within 2 minutes, kept 7 days per source) that keeps it out of zapping and out of the list's normal groups.

**Architecture:** A pure `bad_streams.rs` keeps the suspicions (in memory) and the marks (a small JSON file per source), with the clocks handed in. app.rs feeds it at the moments a start fails or a first picture comes, and hands the marks to the catalog (`CatalogStore::set_defect`), which stays the one filter. The catalog's channels become one `Arc<Vec<LiveChannel>>`: the zap ring (`ZapRing`, positions, cached per lock and marks) and the list's groups (`channel_list.rs`, positions, built when the list opens) share it and never copy a channel. `channel_list_ui.rs` draws the list over the running picture and returns `ListAction`s, which app.rs runs through its one `ZapPacer`.

**Tech Stack:** Rust 2021, egui/eframe 0.27.2, tokio, serde + serde_json, chrono, parking_lot, libmpv (client API 2.5); tempfile and wiremock (tests); Python 3 (`mock_portal.py`), PowerShell 5.1 (e2e).

**Spec:** `docs/superpowers/specs/2026-10-15-zenderlijst-design.md`

**Where:** the main checkout `C:\Dropbox\Sources\tv`, branch `feat/zenderlijst`, which the controller rebases on the finished `feat/m3u-bronnen` and checks out there before Task 1. The M3U branch got commits after this plan was written (docs, an early exit in `catalog::guide_channels_of`, and a fix wave and a residual round up to the M3U branch's final head: `Cmd::LoadWithHeaders` and `player::loadfile_command`, `split_headers` skipping empty values, `Portal::retire`, `Sources::keep` returning `Option` and never giving a list id 1, the `zap_to` empty-address guard, the eof-before-picture frame drop, test-only `Debug` on the source types, the tests' `loads`/`zapped` taking both load kinds, and `list_server`; then a played channel that keeps its own last frame when its reloads run out (`current_has_played`), the NUL check reading only the first KB, `Cmd`, `XtreamCreds`, `XtreamPortal`, `LastWatched` and `args::Cli` deriving `Debug` in tests only, a pick that never played taking off the picture of one that did (a test), and the download's buffer sized from Content-Length only once its first KB has passed the NUL check). This plan names functions and types, never line numbers. When a function it names reads differently from what a step shows around the change, keep the step's meaning, make the change there, and say so in your report.

## Global Constraints

- **Performance is binding.** Startup and zapping are not slower than 0.6.1: the A/B script (`rust/e2e/measure-startup.ps1 -Baseline`) against the 0.6.1 release exe, at least 10 runs each, median and minimum within ±10 %, with an Xtream source, and with an active M3U source of about 11,000 channels (`-M3u`). The groups and their order are worked out in 20 ms or less for 11,000 channels in a release build, and keep less than 1 MB: positions, never copies of channels. A zap press copies no channel list. A closed list costs nothing per frame: no work, no memory. No network, except one check per confirmed suspect (M3U only); no new thread. The only new work at startup is reading one small file when an M3U catalog loads.
- **Dependencies:** no new crates. `serde`, `serde_json`, `chrono` and `parking_lot` are direct dependencies already; `tempfile` is a dev-dependency.
- **Texts:** no UI text outside `i18n.rs`; every key has NL, EN and DE; log lines stay English.
- **Credentials:** an M3U address is a credential. The marks file holds stream ids and unix times only: no names, no URLs. Log lines name a channel by its name (through `crate::redact::redact`) and its stream id, as the zap line does; never a URL, also not the address the check before a mark asks (`Check` has no `Debug`). `Cmd`, `args::Cli`, `XtreamCreds`, `CredsSource`, `XtreamPortal` and `LastWatched` derive `Debug` in the crate's own tests only, and the integration tests (`rust/tests/`) link the lib without them: code there never formats one with `{:?}`, nor compares one with `assert_eq!` or takes one out of `unwrap_err`.
- **The catalog is the only filter.** Zapping, the channel list and the search read the adult lock and the defect marks through `CatalogStore`, never from a copy of their own.
- **Defect marks are an M3U source's alone.** An Xtream portal never reads, writes or applies them, and never checks an address.
- **Release profile** is `panic = "abort"`: nothing may rely on catching a panic.
- **End-to-end runs:**
  - Only through `rust/e2e/run-scenario.ps1`, or `rust/e2e/regression.ps1`, which calls it, against `rust/e2e/mock_portal.py`. Never against the real portal or a real M3U provider.
  - No real mouse or keyboard injection (no SetCursorPos, mouse_event, keybd_event, SendInput). The user works on this machine.
  - An implementer never runs a scenario: `-DryRun` only. The controller asks the user before any run that opens app windows.
- **The real ini:**
  - Never open, print, copy or modify `rust\target\release\BongoPlayer.ini`.
  - Never copy `rust\target` or `rust\target\release` as a folder (copying the single file `BongoPlayer.exe` is fine).
  - A test that can make the app write the ini sets `BONGO_INI` to a temp file while it holds `ENV_LOCK`, as the existing tests in `app.rs` do.
  - `rust/e2e/check-staged.sh` reads the real ini and prints only counts.
- **Formatting:**
  - Every new file is rustfmt-clean (`rustfmt --check --edition 2021 <file>`).
  - In `app.rs` and `catalog.rs` (not rustfmt-clean), format only the lines you add. Before you edit such a file, note `rustfmt --check --edition 2021 <file> 2>&1 | grep -c '^Diff in'`; the count must be the same afterwards. Put new `use` lines inside the file's existing import block.
  - The old hunks at the base (Task 1 notes them): `app.rs` 7, `catalog.rs` 3, `main.rs` 0, `player/mod.rs` 2, `portal/mod.rs` 1 (the one in `portal/xtream.rs`, which it declares), `portal/xtream.rs` 1, `tests/test_args.rs` 8, `tests/test_xtream.rs` 8; every other file is clean. No task touches `main.rs`, `player/`, `portal/` or those two test files. Every task keeps these counts: its new or touched code adds no hunk.
  - rustfmt on a file also checks the modules that file declares. Never pass `lib.rs` to it.
  - Files that are clean before you edit them stay clean: check `storage.rs`, `i18n.rs`, `guide_ui.rs` and the test files you touch before and after.
- **Clippy:** `cargo clippy --all-targets` shows only the 2 pre-existing warnings (`portal/xtream.rs` items after a test module, `main.rs` nul-terminated string).
- **Building and testing:** run cargo from `C:\Dropbox\Sources\tv\rust` with `CARGO_TARGET_DIR=C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt`, `-j 4` (memory is tight) and `--config 'profile.dev.package.bongo-player.debug=1'` (the linker otherwise hits "Permission denied" on the test exe). Task 1 notes how many tests pass before this plan; each task reports the new total. Only Task 10 builds the release into `rust\target`.
- **Command shorthands used in the steps** (bash; shell functions do not survive between tool calls, so type the command out):
  - `ct ARGS` means `cd /c/Dropbox/Sources/tv/rust && CARGO_TARGET_DIR='C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt' cargo test -j 4 --config 'profile.dev.package.bongo-player.debug=1' ARGS`.
  - `cc` means the same with `cargo clippy -j 4 --all-targets` in place of `cargo test …`.
  - `fmt FILES` means `cd /c/Dropbox/Sources/tv/rust && rustfmt --check --edition 2021 FILES`.
- **Commits:**
  - On branch `feat/zenderlijst` only.
  - Commit with `git add <files> && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- <files>`, run from the repo root `C:\Dropbox\Sources\tv`.
  - Stage only the files you changed. Never stage `docs/huur methode 1/`, `2/`, `3/` or `docs/inspectielijst/`.
  - Never push.
  - Every commit message ends with these two lines:

  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
  ```
- **Machine quirks:**
  - cargo runs from `C:\Dropbox\Sources\tv\rust`, git from `C:\Dropbox\Sources\tv`.
  - Python is `py -3`.
  - On git's "os error 32" or "unable to write new index file", retry.
  - `ld: cannot find -lmpv`: `touch rust/build.rs` and build again.
  - PowerShell 5.1 does not wait for the GUI exe started with `&`. Use `Start-Process -Wait -RedirectStandardOutput` for `--version`/`--selftest`.
- **The version is not bumped.** The user decides at release.

## Rulings

The spec is the authority. These settle what it leaves open, one line each: what, and why.

1. **Where a start "fails before its first picture" (§4.1):** `EndOfFile` "error"; "eof" once the reload budget is spent; a refused load (`Event::Error` while starting); the stall watchdog. Only on an M3U source, while `stream_starting_since` is set, for a live channel with a stream id. These are all the ways a start ends without a picture today.
2. **"The app gives the start up" (stall watchdog):** an M3U channel that showed no picture since it was picked is no longer reloaded when the watchdog fires; it stops as a refused load does (Stop, the old picture goes, "streamfout op …") and becomes a suspect. A reload would only start the 25 s wait over, with a "herverbinden" toast each time. Xtream is unchanged.
3. **A channel that showed a picture since it was picked** (`current_has_played`, the base's flag: kept by reloads of the same stream, cleared by every other load and by `stop_playback`) is never a suspect: its failing reloads are the "zender die beeld gaf en daarna wegvalt" of §4.1.
4. **"First picture" is mpv's playback-restart (`Event::PlaybackStarted`)**, the moment that clears `stream_starting_since`. Any first picture confirms the other suspects, also one of a stream without a stream id: the connection worked.
5. **Clocks:** `BadStreams` takes `now: Instant` (the 2 minutes) and `now_unix: i64` (the 7 days) as arguments; the app passes `Instant::now()` and `chrono::Utc::now().timestamp()`; tests pass made-up values. No clock trait: the module's tests cover both windows exactly, and app tests use real time and files with old timestamps.
6. **A mark keeps the time of the failure**, which "hoe lang geleden" shows and the 7 days count from; "older than 7 days" is more than 604,800 s.
7. **Marks are read** when the source's catalog first loads in a catalog generation (a start, a switch, its address saved again); every new catalog revision and F5 expire old ones (an F5 that brings the same list adds no revision, so F5 expires explicitly).
8. **`BadStreams` (app) is the truth, the catalog holds a copy** (`set_defect`) for its filter; every change goes through the app, which hands the new map over at once.
9. **Log lines (§4.6):** `m3u: <name> (stream_id <id>) failed before its first picture`, `m3u: <name> (stream_id <id>) marked defect (<what the check got: http 404, connect failed, timeout, …>)`, `… marked defect (failed again)` for a renewal, `m3u: <name> (stream_id <id>) answers again, not marked`, `m3u: <name> (stream_id <id>) plays again, mark cleared`, `m3u: <n> defect marks expired`.
10. **The zap ring** holds positions in the catalog's channels (one shared `Arc<Vec<LiveChannel>>`), cached inside the stored catalog per (lock, marks revision); `zap_channels()` stays, as a copy, for tests and tools. This also ends the per-press dedup over ~13,000 Xtream channels.
11. **A channel out of the ring that plays** (marked defect) steps from its place in the file: ↓ to the first ring entry after it, ↑ to the last one before it, round the ring.
12. **The pacer's target is `(i64, String, Option<usize>)`:** a pick in the channel list carries no ring position; zapping then finds the channel by its id.
13. **A start that comes due for the channel that plays loads nothing** (flipped away and back, ↓↓↑): "plays" means current and not ended with an error (`stall_watchdog_armed`), so a failed channel flipped back to is tried again. On the main screen too: a second load would only re-buffer it.
14. **The list's preview reuses the app's one `ZapPacer`:** a move in the list is a press of it; Esc, g and ←/→ cancel a waiting start; Enter cancels it and plays the selection now unless it plays.
15. **←/→ stop at the first and the last group** (no wrap, as the rows); a group change drops a waiting start: the selection left that channel.
16. **The groups are built when the list opens**, and again while it is open when the catalog, the marks or the favorites change (`favorites_revision`). A closed list keeps only its last group's key. §5 asks for "once per version" and for a closed list without memory; keeping groups between opens would keep the catalog's old channel list alive after a refresh. `favorites_revision` is insurance: no path changes the favorites while the list is open.
17. **Group names** are the trimmed `;`-parts of `group-title`, empty parts dropped, spelled as the file has them: names that differ in case are two groups, sorted next to each other. A channel in one group twice counts once.
18. **Natural order:** Unicode lower case, a run of ASCII digits by its value, digits before letters; equal names keep the file's order.
19. **Favorieten, Zonder groep and Defecte zenders show only when they hold channels;** Alle zenders always (it is empty when every channel is marked: "geen zenders in deze groep", and Enter only closes).
20. **Names of the fixed groups:** "Favorieten" reuses `T::FavoritesTitle`; the others are new keys. English says "broken" for the defect label ("Broken channels").
21. **A catalog that changes while the list is open:** the same group by its key (else All), the same channel by its id, else the same row as far as the group goes.
22. **Mouse (egui 0.27.2):** a double click's second release reports both `clicked()` and `double_clicked()`; the first click starts the channel (one press), the double click closes the list as Enter does. The wheel scrolls the column under the pointer by whole rows and moves nothing else.
23. **The list is never reversed** by "zaprichting omdraaien", as the guide is not.
24. **The list leaves 64 px under it** for the toast: "streamfout op …" must stay visible (§3.5).
25. **While the list is open no other hotkey works** (§3.1), and opening it closes the favorites list (its rows take clicks).
26. **`T::NoGuideForSource` goes; `T::HelpOpenGuide` names the list.** `g` while an M3U catalog loads shows the guide's existing `T::GuideCatalogLoading`.
27. **e2e:** `M-list` replaces `M-guide` (its toast is gone); `M-defect` goes M3U DOOD → M3U UA → Down and expects M3U REF, since DOOD sits between UA and REF in the mock's file order; the check finds `/m3uplay/dead.m3u8` 404, so the mark comes as before. The mock serves DOOD under `localhost` and every other channel under `127.0.0.1`, so DOOD is not on the server of UA, which plays (ruling 28b).
28. **The check before a mark (the user, 2026-10-15: "Ja, met controle"):** a suspect that the 2-minute rule confirms is not marked at once. Its address is asked once, in the background: one GET with the channel's own User-Agent and Referer (the `|…` split `loadfile_args` uses), 5 s for the connection and the headers, the status only (the body is never read). A status of 400 or more after redirects, no connection (DNS, refused, a failed TLS handshake) or no answer in time marks it; 2xx or 3xx drops the suspicion. At most one check per suspect and 4 in flight; a renewal of a marked channel needs none; another source (switch, save) drops the running checks and ignores their answers. On the app's tokio runtime, the client built on the first check, the answers through a slot `update_ui` drains (as the logos'). Why: a Wi-Fi drop of under 2 minutes would mark every channel tried during it.
    - **28b.** **A check never asks the server of the stream that plays now.** That stream is the channel whose first picture confirms the suspect. A paid list served by an Xtream portal gives every channel one account, and the portal allows one session per account. Either the newest wins, so the check's GET takes the playing channel's session (an eof, a reload, a visible "herverbinden"), or the portal refuses it with a 4xx, which would mark a live channel. Such a suspect is not marked and its suspicion goes. The log says `m3u: <name> (stream_id <id>) not checked (the server of what plays), not marked`. "The same server" is the same scheme, host (any letter case) and port, the scheme's default port filled in (`stream_check::same_server`).
29. **An address the check cannot ask** (no http or https, or no HTTP client) is not marked: the spec marks a confirmed suspect only when the check finds it dead too (§4.2), and its error falls to the safe side. Its suspicion goes; the log says `m3u: <name> (stream_id <id>) not checked (<what>), not marked`.
30. **A channel that names no User-Agent is asked as mpv asks it**, with libmpv's default `libmpv` (pinned by an ignored smoke test against the vendored libmpv), so the server answers the check as it answered the player. It takes a certificate and header bytes as mpv does: no certificate check, header values as bytes. The check hands no `Referer` of its own on through a redirect: that would give a tokenized address to another server. A delete never touches the source in use, so only a switch or a save drops checks.
31. **`bad_streams.rs` keeps a `write_atomic` of its own**, as each file kind of the app does (args.rs, settings.rs, sources.rs, guide/cache.rs, portal/m3u.rs): one shared helper is a cleanup of its own.

## Review Focus

The six inputs the spec implies but its test list does not exercise, most likely to bite first. Each has its test in the owning task.

1. **A fresh copy of the list lands while the list is open** (the saved list replaced by the download at start, or F5): the list keeps its group and its channel, and no row points at another channel. → Task 8, `the_list_keeps_its_place_when_a_fresh_copy_of_the_list_lands`.
2. **A channel gets marked while the list is open** (the user previews a dead channel, then one that plays): it moves to "Defecte zenders" and the selection stays on the channel that plays. → Task 8, `a_channel_marked_while_the_list_is_open_moves_to_broken_channels`.
3. **Enter on the channel that failed** (it is the current one, under "streamfout op …"): it is tried again, not taken for "already playing". → Task 8, `enter_on_a_channel_that_failed_tries_it_again`.
4. **Overshoot and come back** (↓↓↑ quickly, on the main screen or in the list): the channel that plays keeps playing, with no second load. → Task 3, `flipping_away_and_back_to_the_channel_that_plays_loads_nothing`.
5. **Untidy `group-title`s and twin names** (`" Kinderen ; ;Kinderen"`, `Nieuws` next to `nieuws`, two channels called "Een"): parts trimmed, empty ones dropped, a channel once per group, twins in file order. → Task 6, `untidy_group_titles_are_read_as_people_mean_them` and `channels_of_one_name_keep_the_files_order`.
6. **A check that answers late, up to its 5 s, while the list is open:** the list keeps working (keys, groups), nothing waits for the answer, and the channel moves to "Defecte zenders" in the frame the answer is taken in. → Task 8, `a_channel_marked_while_the_list_is_open_moves_to_broken_channels` (its address answers half a second late).

Not a finding: the search row's `" · "` before the defect label (Task 5) is punctuation, as the search's own hardcoded `[LIVE]` tag is; the label itself is `T::DefectLabel`.

## File structure

| File | Responsibility |
|---|---|
| `rust/src/bad_streams.rs` (new) | suspicions, confirmation within 2 minutes, marks, renewal, clearing, 7-day expiry, the marks file |
| `rust/src/stream_check.rs` (new) | the check before a mark: one GET of a confirmed suspect's address, 4 at a time, the answers in a slot; `same_server` |
| `rust/src/channel_list.rs` (new) | the list's groups (`ChannelGroups`), natural order, the selection (`Cursor`); no egui |
| `rust/src/channel_list_ui.rs` (new) | draws the list, reads its keys and the mouse, returns `ListAction`s |
| `rust/src/catalog.rs` | the shared channel list, `ZapRing`/`ZapFrom`, the defect filter, `channel_groups` |
| `rust/src/storage.rs` | `bad_streams_path`; `remove_source_files` takes the marks along |
| `rust/src/app.rs` | zapping through the ring, the failure and picture hooks, the checks' verdicts, the labels, `g` on M3U, the list's actions |
| `rust/src/guide_ui.rs` | `single_line`, `play_mark` and `CLICK` shared with the list (visibility, a colour) |
| `rust/src/i18n.rs` | the new texts; `NoGuideForSource` goes |
| `rust/src/lib.rs` | the four new modules |
| `rust/tests/test_bad_streams.rs`, `test_stream_check.rs`, `test_zap_ring.rs`, `test_channel_list.rs` (new); `test_storage.rs`, `test_mpv_smoke.rs`, `test_catalog_m3u.rs` | tests |
| `rust/e2e/regression.ps1`, `run-scenario.ps1`, `README.md` | `M-list`, `M-defect` |
| `CLAUDE.md`, `rust/README.md`, `docs/VALIDATION.md`, `rust/build-zip.ps1` | docs |

## Tasks

1. Defect marks: the module and the file per source
2. The catalog: the zap ring and the defect filter
3. App: zapping through the ring
4. The check before a mark
5. App: defect marks
6. The channel list's groups and selection
7. The channel list on screen
8. App: `g` opens the channel list on an M3U source
9. End-to-end: `M-list` and `M-defect`
10. Docs, the release measurement, full verification

---

### Task 1: Defect marks: the module and the file per source

A pure module that keeps the suspicions of this session and the marks of one source. A suspect is confirmed when another channel shows a first picture within 2 minutes of its failure, and marked only when the check of its address (Task 4) finds it dead too; without a confirmation the suspicion lapses. The marks live 7 days in a small file of ids and times. Callers bring the clocks.

**Files:**
- Create: `rust/src/bad_streams.rs`
- Modify: `rust/src/lib.rs` (`pub mod bad_streams;` after `pub mod args;`)
- Modify: `rust/src/storage.rs` (`bad_streams_path`, `remove_source_files`)
- Test: `rust/tests/test_bad_streams.rs` (new), `rust/tests/test_storage.rs`

**Interfaces:**
- Consumes: `Storage::source_file(stem, id)` (private, in storage.rs: `favorites.json` for source 1, `favorites-<id>.json` for the others).
- Produces:
  ```rust
  // bad_streams.rs
  pub const CONFIRM_WITHIN: Duration;      // 120 s
  pub const KEEP_SECS: i64;                // 604_800
  pub enum Failure { Suspected, Renewed }  // Debug, Clone, Copy, PartialEq, Eq
  pub struct Confirmed { pub stream_id: i64, pub name: String, pub failed_unix: i64 }  // Debug, Clone, PartialEq, Eq
  pub struct Picture { pub confirmed: Vec<Confirmed>, pub cleared: bool }  // Debug, Default, PartialEq, Eq
  pub struct BadStreams;
  impl BadStreams {
      pub fn load(path: PathBuf) -> Self;
      pub fn marks(&self) -> &HashMap<i64, i64>;      // stream id -> unix seconds of the failure
      pub fn is_marked(&self, stream_id: i64) -> bool;
      pub fn suspects(&self) -> usize;
      pub fn failed(&mut self, stream_id: i64, name: &str, now: Instant, now_unix: i64) -> Failure;
      pub fn first_picture(&mut self, stream_id: Option<i64>, now: Instant) -> Picture;  // confirms; marks nothing
      pub fn mark(&mut self, stream_id: i64, failed_unix: i64);  // the check found it dead; writes the file
      pub fn expire(&mut self, now_unix: i64) -> usize;
  }
  // storage.rs
  impl Storage { pub fn bad_streams_path(&self) -> PathBuf; }  // remove_source_files removes it too
  ```
  Log lines (warn): `m3u: defect marks unreadable (<error>) - none until the next change`, `m3u: defect marks of version <n> not read`, `m3u: defect marks not written (<error>)`.

- [ ] **Step 1: The baseline**

Run: `ct`, then `cc`, then the old rustfmt hunks of every file but `lib.rs` (it prints only the files that have some): `cd /c/Dropbox/Sources/tv/rust && for f in $(find src tests -name '*.rs' | grep -v '^src/lib.rs$' | sort); do n=$(rustfmt --check --edition 2021 $f 2>&1 | grep -c '^Diff in'); [ "$n" = 0 ] || echo "$f: $n"; done`
Expected: everything passes; clippy shows the 2 old warnings; the hunks are these seven lines, and every other file (`main.rs` among them) is clean (Global Constraints, Formatting):

```
src/app.rs: 7
src/catalog.rs: 3
src/player/mod.rs: 2
src/portal/mod.rs: 1
src/portal/xtream.rs: 1
tests/test_args.rs: 8
tests/test_xtream.rs: 8
```

Note the passing tests, the warnings and the hunks for your report: every later task compares with them.

- [ ] **Step 2: Write the failing tests**

Create `rust/tests/test_bad_streams.rs`:

```rust
//! The defect marks of an M3U source (bad_streams.rs): suspicions, the
//! confirmation by another channel's first picture, the marks the check sets,
//! the seven days, the file.

use bongo_player::bad_streams::{
    BadStreams, Confirmed, Failure, Picture, CONFIRM_WITHIN, KEEP_SECS,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 15 October 2026, 00:00 UTC, as unix seconds.
const NOW: i64 = 1_792_022_400;

fn marks_file(dir: &Path) -> PathBuf {
    dir.join("bad_streams.json")
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// Marks of a folder that holds no file yet, and the folder.
fn fresh() -> (tempfile::TempDir, BadStreams) {
    let dir = tempfile::tempdir().unwrap();
    let bad = BadStreams::load(marks_file(dir.path()));
    (dir, bad)
}

/// Channel 1, "Een", confirmed: it failed at `failed_unix`.
fn een(failed_unix: i64) -> Vec<Confirmed> {
    vec![Confirmed {
        stream_id: 1,
        name: "Een".to_string(),
        failed_unix,
    }]
}

/// What the app does with each channel a picture confirmed once the check
/// finds its address dead too: it marks it.
fn mark_all(bad: &mut BadStreams, picture: Picture) {
    for c in picture.confirmed {
        bad.mark(c.stream_id, c.failed_unix);
    }
}

#[test]
fn a_failure_alone_marks_nothing_and_writes_nothing() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    assert_eq!(bad.failed(1, "Een", t0, NOW), Failure::Suspected);
    assert!(!bad.is_marked(1));
    assert_eq!(bad.suspects(), 1);
    assert!(!marks_file(dir.path()).exists(), "a suspicion lives in memory only");
}

#[test]
fn another_channels_picture_within_two_minutes_confirms_it_for_the_check() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    let seen = bad.first_picture(Some(2), t0 + CONFIRM_WITHIN);
    assert_eq!(
        seen,
        Picture {
            confirmed: een(NOW),
            cleared: false
        }
    );
    assert!(!bad.is_marked(1), "its address is checked first");
    assert_eq!(bad.suspects(), 0, "a suspect is confirmed once");
    assert!(!marks_file(dir.path()).exists());
}

#[test]
fn a_mark_after_the_check_keeps_the_time_of_the_failure_and_is_written_at_once() {
    let (dir, mut bad) = fresh();
    bad.mark(1, NOW);
    assert_eq!(bad.marks().get(&1), Some(&NOW));
    let again = BadStreams::load(marks_file(dir.path()));
    assert_eq!(again.marks().get(&1), Some(&NOW), "written");
    bad.mark(1, NOW - 50);
    assert_eq!(bad.marks().get(&1), Some(&NOW), "the latest failure counts");
}

#[test]
fn a_mark_drops_the_suspicion_of_its_channel_and_its_next_failure_renews_it() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    // It failed again while its address was being checked.
    bad.failed(1, "Een", t0, NOW);
    bad.mark(1, NOW - 10);
    assert_eq!(bad.suspects(), 0);
    assert_eq!(bad.failed(1, "Een", t0 + secs(5), NOW + 5), Failure::Renewed);
}

#[test]
fn a_picture_after_two_minutes_confirms_nothing_and_the_suspicion_lapses() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    let seen = bad.first_picture(Some(2), t0 + CONFIRM_WITHIN + secs(1));
    assert_eq!(seen, Picture::default());
    assert!(!bad.is_marked(1));
    assert_eq!(bad.suspects(), 0, "lapsed");
}

#[test]
fn its_own_picture_drops_the_suspicion() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    assert_eq!(bad.first_picture(Some(1), t0 + secs(5)), Picture::default());
    // A later picture of another channel finds no suspect left.
    assert_eq!(bad.first_picture(Some(2), t0 + secs(10)), Picture::default());
    assert!(!bad.is_marked(1));
}

#[test]
fn a_dropped_connection_confirms_nothing() {
    // Five channels fail in a row; nothing shows for more than two minutes.
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    for id in 1..=5 {
        bad.failed(id, &format!("K{id}"), t0 + secs(10 * id as u64), NOW);
    }
    let seen = bad.first_picture(Some(6), t0 + secs(50) + CONFIRM_WITHIN + secs(1));
    assert!(seen.confirmed.is_empty(), "{seen:?}");
    assert!(bad.marks().is_empty());
}

#[test]
fn a_stream_that_is_no_channel_of_the_list_confirms_too() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    assert_eq!(bad.first_picture(None, t0 + secs(3)).confirmed, een(NOW));
}

#[test]
fn a_second_failure_of_a_suspect_counts_from_the_latest() {
    let (_dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    bad.failed(1, "Een", t0 + secs(100), NOW + 100);
    assert_eq!(bad.suspects(), 1, "one suspicion per channel");
    let seen = bad.first_picture(Some(2), t0 + secs(200));
    assert_eq!(seen.confirmed, een(NOW + 100));
}

#[test]
fn a_marked_channels_picture_clears_its_mark_and_the_file_says_so() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.mark(1, NOW);
    let seen = bad.first_picture(Some(1), t0 + secs(60));
    assert!(seen.cleared);
    assert!(!bad.is_marked(1));
    let again = BadStreams::load(marks_file(dir.path()));
    assert!(again.marks().is_empty(), "written");
}

#[test]
fn a_marked_channel_that_fails_again_starts_its_seven_days_again_without_confirmation() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.mark(1, NOW);
    let later = NOW + 3 * 24 * 3600;
    assert_eq!(bad.failed(1, "Een", t0 + secs(30), later), Failure::Renewed);
    assert_eq!(bad.suspects(), 0, "no check for a renewal");
    assert_eq!(bad.marks().get(&1), Some(&later));
    let again = BadStreams::load(marks_file(dir.path()));
    assert_eq!(again.marks().get(&1), Some(&later), "written at once");
}

#[test]
fn marks_older_than_seven_days_expire() {
    let (dir, mut bad) = fresh();
    for (id, failed) in [(1, NOW - KEEP_SECS - 1), (2, NOW - KEEP_SECS), (3, NOW - 60)] {
        bad.mark(id, failed);
    }
    assert_eq!(bad.expire(NOW), 1);
    assert!(!bad.is_marked(1), "more than seven days");
    assert!(bad.is_marked(2) && bad.is_marked(3));
    assert_eq!(bad.expire(NOW), 0);
    let again = BadStreams::load(marks_file(dir.path()));
    assert_eq!(again.marks().len(), 2, "written");
}

#[test]
fn the_file_holds_ids_and_times_only() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    // A name may be an address with a password in it: it never reaches the file.
    bad.failed(12345, "https://user:secret@x.example/1.m3u8", t0, NOW);
    let seen = bad.first_picture(Some(2), t0 + secs(1));
    mark_all(&mut bad, seen);
    let text = std::fs::read_to_string(marks_file(dir.path())).unwrap();
    assert_eq!(
        text,
        format!("{{\"version\":1,\"marks\":{{\"12345\":{NOW}}}}}")
    );
}

#[test]
fn the_marks_survive_a_new_load() {
    let (dir, mut bad) = fresh();
    let t0 = Instant::now();
    bad.failed(1, "Een", t0, NOW);
    bad.failed(7, "Zeven", t0 + secs(1), NOW + 1);
    let seen = bad.first_picture(Some(2), t0 + secs(2));
    mark_all(&mut bad, seen);
    let again = BadStreams::load(marks_file(dir.path()));
    assert_eq!(again.marks(), bad.marks());
    assert_eq!(again.marks().len(), 2);
    assert_eq!(again.suspects(), 0, "suspicions are not kept");
}

#[test]
fn a_broken_file_reads_as_none_and_the_next_change_writes_it_anew() {
    let dir = tempfile::tempdir().unwrap();
    let path = marks_file(dir.path());
    std::fs::write(&path, "{not json").unwrap();
    let mut bad = BadStreams::load(path.clone());
    assert!(bad.marks().is_empty());
    bad.mark(7, NOW);
    let again = BadStreams::load(path);
    assert_eq!(again.marks().get(&7), Some(&NOW));
}

#[test]
fn a_file_of_another_version_reads_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = marks_file(dir.path());
    std::fs::write(&path, r#"{"version":2,"marks":{"1":5}}"#).unwrap();
    assert!(BadStreams::load(path).marks().is_empty());
}

#[test]
fn nothing_to_expire_writes_no_file() {
    let (dir, mut bad) = fresh();
    assert_eq!(bad.expire(NOW), 0);
    assert!(!marks_file(dir.path()).exists());
}
```

Run `rustfmt --edition 2021 tests/test_bad_streams.rs` once written.

In `rust/tests/test_storage.rs`:

1. In `source_1_keeps_the_0_6_1_file_names_and_the_others_carry_their_id`, after the `last_watched_path` assertion of source 1, add:

```rust
    assert_eq!(storage.bad_streams_path(), root.join("bad_streams.json"));
```

and after the `last_watched-2.json` assertion of source 2, add:

```rust
    assert_eq!(
        storage.bad_streams_path(),
        root.join("bad_streams-2.json")
    );
```

2. In `removing_a_sources_files_leaves_the_other_sources_alone`, add `storage.bad_streams_path(),` to the list of paths written per source (after `storage.last_watched_path(),`), and change `assert_eq!(storage.remove_source_files(2), 4);` to:

```rust
    assert_eq!(storage.remove_source_files(2), 5, "its marks go too");
```

- [ ] **Step 3: Run them to see them fail**

Run: `ct --test test_bad_streams --test test_storage`
Expected: compile errors: there is no module `bad_streams`, and `Storage` has no `bad_streams_path`.

- [ ] **Step 4: Write the module and the path**

Create `rust/src/bad_streams.rs`:

```rust
//! Defect marks for the channels of an M3U list (spec 2026-10-15, §4).
//!
//! A channel whose start ends without a first picture is a suspect. It is
//! confirmed once another channel shows a first picture within two minutes of
//! that failure: the connection worked. Its address is then asked once more
//! (`stream_check.rs`), and only a dead answer marks it (spec §4.2), so a Wi-Fi
//! drop of under two minutes marks no good channel. Without a confirmation the
//! suspicion lapses. A first picture of the channel itself clears its
//! suspicion and its mark; a marked channel that fails again is marked anew at
//! once, without a check.
//!
//! Marks are kept per source for seven days, in `bad_streams.json` for source
//! 1 and `bad_streams-<id>.json` for the others (`Storage::bad_streams_path`):
//! `{"version":1,"marks":{"<stream_id>":<unix seconds>}}`, ids and times only.
//! No names and no addresses: an M3U address can hold a password. Suspicions
//! live in memory only. The callers bring the clocks: an `Instant` for the two
//! minutes, unix seconds for the seven days.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long after a failure another channel's first picture confirms it.
pub const CONFIRM_WITHIN: Duration = Duration::from_secs(120);

/// How long a mark is kept: seven days, in seconds.
pub const KEEP_SECS: i64 = 7 * 24 * 60 * 60;

/// The file's format.
const VERSION: u32 = 1;

/// The marks file as it is written.
#[derive(serde::Serialize, serde::Deserialize)]
struct MarksFile {
    version: u32,
    marks: BTreeMap<i64, i64>,
}

/// A channel whose start failed, waiting for another channel's picture.
struct Suspect {
    stream_id: i64,
    name: String,
    failed_at: Instant,
    failed_unix: i64,
}

/// What a failure did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The channel is a suspect now, counted from this failure.
    Suspected,
    /// It was marked already: the mark's seven days start again.
    Renewed,
}

/// A suspect that another channel's first picture confirmed: its address is
/// checked (`stream_check.rs`) before it gets the mark (spec §4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    pub stream_id: i64,
    pub name: String,
    /// When it failed: the mark keeps this time.
    pub failed_unix: i64,
}

/// What a first picture did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Picture {
    /// The suspects it confirmed, for the check. None of them is marked yet.
    pub confirmed: Vec<Confirmed>,
    /// The channel that shows it was marked: its mark is gone.
    pub cleared: bool,
}

/// The marks of one source, and the suspicions of this session.
pub struct BadStreams {
    path: PathBuf,
    /// Stream id → when it failed, unix seconds.
    marks: HashMap<i64, i64>,
    suspects: Vec<Suspect>,
}

impl BadStreams {
    /// The marks in the file at `path`. A file that is not there holds none.
    /// One that cannot be read, or is no marks file of this version, holds none
    /// either: the log says so, and the next change writes it anew.
    pub fn load(path: PathBuf) -> Self {
        let marks = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<MarksFile>(&bytes) {
                Ok(file) if file.version == VERSION => file.marks.into_iter().collect(),
                Ok(file) => {
                    tracing::warn!("m3u: defect marks of version {} not read", file.version);
                    HashMap::new()
                }
                Err(e) => {
                    tracing::warn!("m3u: defect marks unreadable ({e}) - none until the next change");
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => {
                tracing::warn!("m3u: defect marks unreadable ({e}) - none until the next change");
                HashMap::new()
            }
        };
        Self {
            path,
            marks,
            suspects: Vec::new(),
        }
    }

    /// When each marked channel failed: stream id → unix seconds.
    pub fn marks(&self) -> &HashMap<i64, i64> {
        &self.marks
    }

    pub fn is_marked(&self, stream_id: i64) -> bool {
        self.marks.contains_key(&stream_id)
    }

    /// How many suspicions wait for a picture.
    pub fn suspects(&self) -> usize {
        self.suspects.len()
    }

    /// Channel `stream_id` (`name`, for the log) failed before its first
    /// picture at `now` (`now_unix`). A marked one is marked anew at once and
    /// the file written; another one is a suspect, counted from this failure.
    pub fn failed(&mut self, stream_id: i64, name: &str, now: Instant, now_unix: i64) -> Failure {
        // Suspicions too old to be confirmed would only lapse: they go now.
        self.suspects.retain(|s| {
            s.stream_id != stream_id && now.saturating_duration_since(s.failed_at) <= CONFIRM_WITHIN
        });
        if let Some(at) = self.marks.get_mut(&stream_id) {
            *at = now_unix;
            self.save();
            return Failure::Renewed;
        }
        self.suspects.push(Suspect {
            stream_id,
            name: name.to_owned(),
            failed_at: now,
            failed_unix: now_unix,
        });
        Failure::Suspected
    }

    /// A first picture at `now` of channel `stream_id` (`None`: a stream that is
    /// no channel of the list). The suspects that failed within
    /// [`CONFIRM_WITHIN`] before it are confirmed: they leave the suspects, for
    /// the caller to check their addresses and [`BadStreams::mark`] the dead
    /// ones. Older ones lapse. The channel's own suspicion goes, and so does its
    /// mark (the file is written then).
    pub fn first_picture(&mut self, stream_id: Option<i64>, now: Instant) -> Picture {
        let mut picture = Picture::default();
        for s in std::mem::take(&mut self.suspects) {
            if Some(s.stream_id) == stream_id {
                continue;
            }
            if now.saturating_duration_since(s.failed_at) <= CONFIRM_WITHIN {
                picture.confirmed.push(Confirmed {
                    stream_id: s.stream_id,
                    name: s.name,
                    failed_unix: s.failed_unix,
                });
            }
        }
        if let Some(id) = stream_id {
            picture.cleared = self.marks.remove(&id).is_some();
        }
        if picture.cleared {
            self.save();
        }
        picture
    }

    /// Marks channel `stream_id`, which failed at `failed_unix` and whose
    /// address the check found dead, and writes the file. A mark it has for a
    /// later failure keeps that time. A suspicion of it (it failed again while
    /// its address was asked) goes: from now on a failure renews the mark.
    pub fn mark(&mut self, stream_id: i64, failed_unix: i64) {
        let at = self.marks.entry(stream_id).or_insert(failed_unix);
        *at = (*at).max(failed_unix);
        self.suspects.retain(|s| s.stream_id != stream_id);
        self.save();
    }

    /// Marks older than [`KEEP_SECS`] at `now_unix` go; how many went. Writes
    /// the file when any did.
    pub fn expire(&mut self, now_unix: i64) -> usize {
        let before = self.marks.len();
        self.marks
            .retain(|_, at| now_unix.saturating_sub(*at) <= KEEP_SECS);
        let gone = before - self.marks.len();
        if gone > 0 {
            self.save();
        }
        gone
    }

    /// Writes the marks. A failure is logged; the marks stay in memory.
    fn save(&self) {
        let file = MarksFile {
            version: VERSION,
            marks: self.marks.iter().map(|(id, at)| (*id, *at)).collect(),
        };
        let written = serde_json::to_vec(&file)
            .map_err(std::io::Error::other)
            .and_then(|bytes| write_atomic(&self.path, &bytes));
        if let Err(e) = written {
            tracing::warn!("m3u: defect marks not written ({e})");
        }
    }
}

/// `bytes` to `path` through a temp file beside it, so a write that fails
/// leaves the old file whole.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}
```

Run `rustfmt --edition 2021 src/bad_streams.rs` once you have written it: the layout above is close to rustfmt's but not guaranteed to be it.

In `rust/src/lib.rs`, add `pub mod bad_streams;` after `pub mod args;`.

In `rust/src/storage.rs`:

1. After `last_watched_path`, add:

```rust
    /// The defect marks of the source in use (`bad_streams.rs`): stream ids and
    /// times only, never an address.
    pub fn bad_streams_path(&self) -> PathBuf {
        self.source_file("bad_streams", self.source)
    }
```

2. In `remove_source_files`, add `self.source_file("bad_streams", id),` to the array (after `self.source_file("last_watched", id),`), and make its doc comment say "Removes source `id`'s favorites, presets, last channel, defect marks and saved list, with the temp file of that list; how many of them there were."

- [ ] **Step 5: Run them to see them pass**

Run: `ct --test test_bad_streams --test test_storage`
Expected: all pass (17 in `test_bad_streams`).

- [ ] **Step 6: Format, lint, full suite**

- Run `fmt src/bad_streams.rs src/storage.rs tests/test_bad_streams.rs tests/test_storage.rs`. Expected: no output.
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus these.

- [ ] **Step 7: Commit**

Message file (in the scratchpad):
```
feat(rust): defect marks for M3U channels: suspects, confirmation, seven days, a file per source

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/bad_streams.rs rust/src/lib.rs rust/src/storage.rs rust/tests/test_bad_streams.rs rust/tests/test_storage.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/bad_streams.rs rust/src/lib.rs rust/src/storage.rs rust/tests/test_bad_streams.rs rust/tests/test_storage.rs`

---

### Task 2: The catalog: the zap ring and the defect filter

The catalog keeps its live channels in one `Arc<Vec<LiveChannel>>`. What up/down zapping steps through becomes a `ZapRing`: positions in that list, worked out once per catalog, lock and marks, cached in the stored catalog, so a press copies nothing. The catalog also holds the defect marks the app hands it: the ring leaves them out; the search and the names still know them.

**Files:**
- Modify: `rust/src/catalog.rs`
- Test: `rust/tests/test_zap_ring.rs` (new), `rust/tests/test_catalog_m3u.rs` (its timing test times a press of the ring)

**Interfaces:**
- Consumes: `crate::shortcuts::next_live_idx(current: Option<usize>, len: usize, delta: i32) -> Option<usize>`; in catalog.rs: `adult_live`, `normalize_channel_name`, `variant_score`, `StoredCatalog`, `CatalogStore::{adult_unlocked, kind, shows_live}`.
- Produces:
  ```rust
  pub struct ZapRing;
  impl ZapRing {
      pub fn len(&self) -> usize;
      pub fn is_empty(&self) -> bool;
      pub fn get(&self, pos: usize) -> Option<&LiveChannel>;
      pub fn channels(&self) -> impl Iterator<Item = &LiveChannel> + '_;
      pub fn position_of(&self, sid: i64) -> Option<usize>;
      pub fn step(&self, from: ZapFrom, delta: i32) -> Option<usize>;
  }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum ZapFrom { Start, Pos { pos: usize, sid: i64 }, Channel(i64) }
  impl CatalogStore {
      pub fn zap_ring(&self) -> Option<Arc<ZapRing>>;          // None until a catalog has loaded
      pub fn zap_channels(&self) -> Vec<LiveChannel>;          // a copy of the ring: tests and tools only
      pub fn live_name(&self, sid: i64) -> Option<String>;     // a channel that shows, defect ones too
      pub fn set_defect(&self, marks: HashMap<i64, i64>);
      pub fn is_defect(&self, sid: i64) -> bool;
      pub fn defect_marks(&self) -> Arc<HashMap<i64, i64>>;
      pub fn defect_revision(&self) -> u64;
  }
  // private, for Task 6: StoredCatalog.live: Arc<Vec<LiveChannel>>
  ```

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_zap_ring.rs`:

```rust
//! The zap ring (catalog.rs): what up/down zapping steps through, worked out
//! once per catalog, lock and marks, with the defect channels out.

use bongo_player::args::XtreamCreds;
use bongo_player::catalog::{CatalogStore, ZapFrom, ZapRing};
use bongo_player::m3u::stream_id;
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::{Catalog, LiveChannel};
use std::collections::HashMap;
use std::sync::Arc;

/// Four channels; with no tvg-id their keys, and so their stream ids, come
/// from their names.
const LIST: &str = "#EXTM3U\n\
#EXTINF:-1,Een\nhttps://a.example/1.m3u8\n\
#EXTINF:-1,Twee\nhttps://a.example/2.m3u8\n\
#EXTINF:-1,Drie\nhttps://a.example/3.m3u8\n\
#EXTINF:-1,Vier\nhttps://a.example/4.m3u8\n";

/// 15 October 2026, 00:00 UTC.
const FAILED: i64 = 1_792_022_400;

fn m3u_store() -> CatalogStore {
    let (portal, catalog) = M3uPortal::from_text(LIST).unwrap();
    CatalogStore::from_catalog(Arc::new(portal), catalog)
}

fn mark(store: &CatalogStore, names: &[&str]) {
    store.set_defect(names.iter().map(|n| (stream_id(n), FAILED)).collect());
}

fn ring(store: &CatalogStore) -> Arc<ZapRing> {
    store.zap_ring().expect("a loaded catalog")
}

fn names(ring: &ZapRing) -> Vec<&str> {
    ring.channels().map(|c| c.name.as_str()).collect()
}

/// The name of the channel `delta` steps on from `from`.
fn step(ring: &ZapRing, from: ZapFrom, delta: i32) -> &str {
    let pos = ring.step(from, delta).expect("a ring with channels");
    ring.get(pos).unwrap().name.as_str()
}

#[test]
fn a_defect_channel_is_out_of_the_ring_until_its_mark_goes() {
    let store = m3u_store();
    assert_eq!(names(&ring(&store)), ["Een", "Twee", "Drie", "Vier"]);
    mark(&store, &["Twee"]);
    assert_eq!(names(&ring(&store)), ["Een", "Drie", "Vier"]);
    assert!(store.is_defect(stream_id("Twee")));
    assert_eq!(
        store.defect_marks().get(&stream_id("Twee")),
        Some(&FAILED)
    );
    store.set_defect(HashMap::new());
    assert_eq!(names(&ring(&store)), ["Een", "Twee", "Drie", "Vier"]);
    assert!(!store.is_defect(stream_id("Twee")));
}

#[test]
fn the_ring_is_worked_out_once_per_catalog_lock_and_marks() {
    let store = m3u_store();
    let first = ring(&store);
    assert!(Arc::ptr_eq(&first, &ring(&store)), "a press copies nothing");
    mark(&store, &["Drie"]);
    let marked = ring(&store);
    assert!(!Arc::ptr_eq(&first, &marked), "new marks, a new ring");
    assert!(Arc::ptr_eq(&marked, &ring(&store)));
    store.set_adult_unlocked(true);
    assert!(!Arc::ptr_eq(&marked, &ring(&store)), "the lock is part of it");
    assert_eq!(store.defect_revision(), 1);
}

#[test]
fn a_defect_channel_that_plays_steps_from_its_place_in_the_file() {
    let store = m3u_store();
    mark(&store, &["Twee", "Vier"]);
    let ring = ring(&store);
    let twee = ZapFrom::Channel(stream_id("Twee"));
    assert_eq!(step(&ring, twee, 1), "Drie");
    assert_eq!(step(&ring, twee, -1), "Een");
    let vier = ZapFrom::Channel(stream_id("Vier"));
    assert_eq!(step(&ring, vier, 1), "Een", "round the ring");
    assert_eq!(step(&ring, vier, -1), "Drie");
    assert_eq!(
        ring.position_of(stream_id("Twee")),
        None,
        "it has no place in the ring"
    );
}

#[test]
fn a_position_that_holds_another_channel_now_is_found_again_by_its_channel() {
    let store = m3u_store();
    let drie = stream_id("Drie");
    // Drie was at 2; with Een marked it is at 1.
    mark(&store, &["Een"]);
    let ring = ring(&store);
    assert_eq!(step(&ring, ZapFrom::Pos { pos: 2, sid: drie }, 1), "Vier");
    assert_eq!(step(&ring, ZapFrom::Pos { pos: 2, sid: drie }, -1), "Twee");
    assert_eq!(step(&ring, ZapFrom::Pos { pos: 1, sid: drie }, 1), "Vier");
}

#[test]
fn with_nothing_known_a_step_counts_from_the_start_as_before() {
    let store = m3u_store();
    let ring = ring(&store);
    assert_eq!(step(&ring, ZapFrom::Start, 1), "Twee", "as next_live_idx(None, ..)");
    assert_eq!(step(&ring, ZapFrom::Start, -1), "Vier");
    assert_eq!(
        step(&ring, ZapFrom::Channel(42), 1),
        "Twee",
        "a channel the list does not have"
    );
}

#[test]
fn a_ring_with_every_channel_marked_has_no_step() {
    let store = m3u_store();
    mark(&store, &["Een", "Twee", "Drie", "Vier"]);
    let ring = ring(&store);
    assert!(ring.is_empty());
    assert_eq!(ring.step(ZapFrom::Start, 1), None);
    assert!(store.zap_channels().is_empty());
}

#[test]
fn the_search_and_the_names_still_know_a_defect_channel() {
    let store = m3u_store();
    mark(&store, &["Twee"]);
    let found: Vec<String> = store.search("twee").into_iter().map(|i| i.name).collect();
    assert_eq!(found, ["Twee"], "the search names it; the app adds the label");
    assert_eq!(
        store.live_name(stream_id("Twee")).as_deref(),
        Some("Twee"),
        "a preset still plays it"
    );
    assert_eq!(store.live_name(42), None);
}

#[test]
fn an_unloaded_catalog_has_no_ring() {
    let (portal, _) = M3uPortal::from_text(LIST).unwrap();
    let store = CatalogStore::new(Arc::new(portal));
    assert!(store.zap_ring().is_none());
    assert!(store.zap_channels().is_empty());
    assert_eq!(store.live_name(stream_id("Een")), None);
}

// ---- an Xtream portal ----

fn xtream(live: Vec<LiveChannel>) -> CatalogStore {
    let portal = XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h".into(),
        port: 80,
    });
    let catalog = Catalog {
        live,
        ..Catalog::default()
    };
    CatalogStore::from_catalog(Arc::new(portal), catalog)
}

fn ch(stream_id: i64, name: &str, is_adult: bool) -> LiveChannel {
    LiveChannel {
        stream_id,
        name: name.into(),
        category_id: None,
        epg_channel_id: None,
        tv_archive: 0,
        tv_archive_duration: 0,
        is_adult,
        stream_icon: None,
    }
}

#[test]
fn an_xtream_variant_out_of_the_ring_steps_from_the_entry_that_stands_for_it() {
    let store = xtream(vec![
        ch(1, "NL | NPO 1 HD", false),
        ch(2, "NL | NPO 1 FHD", false),
        ch(3, "NL | RTL 4", false),
        ch(4, "NL | NOS", false),
    ]);
    let ring = ring(&store);
    assert_eq!(names(&ring), ["NL | NPO 1 FHD", "NL | RTL 4", "NL | NOS"]);
    assert_eq!(ring.position_of(1), Some(0), "the FHD entry stands for NPO 1 HD");
    assert_eq!(step(&ring, ZapFrom::Channel(1), 1), "NL | RTL 4");
    assert_eq!(step(&ring, ZapFrom::Channel(1), -1), "NL | NOS");
}

#[test]
fn the_xtream_ring_follows_the_lock() {
    let store = xtream(vec![ch(1, "NL | NPO 1", false), ch(9, "XXX | NACHT", true)]);
    assert_eq!(names(&ring(&store)), ["NL | NPO 1"]);
    assert_eq!(ring(&store).position_of(9), None, "hidden while locked");
    store.set_adult_unlocked(true);
    assert_eq!(names(&ring(&store)), ["NL | NPO 1", "XXX | NACHT"]);
}
```

Run `rustfmt --edition 2021 tests/test_zap_ring.rs` once written.

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_zap_ring`
Expected: compile errors: no `ZapRing`, `ZapFrom`, `zap_ring`, `set_defect`, `live_name`.

- [ ] **Step 3: The shared channel list and the ring's cache in the stored catalog**

In `rust/src/catalog.rs`, note the hunk count first (Global Constraints). Then replace `struct StoredCatalog` and `impl StoredCatalog` with:

```rust
/// A loaded catalog plus what the app asks of it while it draws, worked out once
/// where the catalog is stored: a lookup by stream id is one probe of a set, not
/// a scan of ~13,000 channels. Reads as the catalog itself (`Deref`).
struct StoredCatalog {
    /// The films and series; its `live` is empty (see `live`).
    catalog: PortalCatalog,
    /// The live channels, taken out of `catalog`: what is worked out from them
    /// (the zap ring, the channel list's groups) holds this `Arc` and positions
    /// in it, never copies of the channels. `c.live` reads this field, not the
    /// catalog's empty one.
    live: Arc<Vec<LiveChannel>>,
    /// Stream ids of the adult live channels (see `adult_live`).
    adult_live_ids: HashSet<i64>,
    /// The guide's columns (see `guide_channels_of`), adult ones included.
    guide: Vec<GuideChannel>,
    /// The zap ring last asked for, with the lock and the marks it was made for.
    zap: Mutex<Option<Arc<ZapRing>>>,
}

impl StoredCatalog {
    /// The one way a catalog gets stored: the fetch and `from_catalog` both come
    /// through here, so what is worked out cannot differ between them.
    fn new(mut catalog: PortalCatalog) -> Self {
        let adult_live_ids = catalog
            .live
            .iter()
            .filter(|c| adult_live(c))
            .map(|c| c.stream_id)
            .collect();
        let guide = guide_channels_of(&catalog.live);
        // Only GuideChannel::logo keeps logos: ~13,000 stored URLs cost memory and a clone each.
        for c in &mut catalog.live {
            c.stream_icon = None;
        }
        let live = Arc::new(std::mem::take(&mut catalog.live));
        Self {
            catalog,
            live,
            adult_live_ids,
            guide,
            zap: Mutex::new(None),
        }
    }

    /// The zap ring for this lock and these marks: the one made last when it was
    /// made for them, else a new one, kept for the next press.
    fn zap_ring(
        &self,
        kind: SourceKind,
        unlocked: bool,
        defect: &HashMap<i64, i64>,
        defect_revision: u64,
    ) -> Arc<ZapRing> {
        let mut slot = self.zap.lock();
        if let Some(ring) = slot
            .as_ref()
            .filter(|r| r.unlocked == unlocked && r.defect_revision == defect_revision)
        {
            return ring.clone();
        }
        let ring = Arc::new(ZapRing::build(
            &self.live,
            kind,
            unlocked,
            defect,
            defect_revision,
        ));
        *slot = Some(ring.clone());
        ring
    }
}
```

Keep `impl std::ops::Deref for StoredCatalog` as it is. Every `c.live` in this file now reads the `Arc<Vec<LiveChannel>>`: `c.live.iter()` and `&c.live` passed as a slice (`quality_ladder(&c.live, …)`) work through it unchanged. A `for` loop does not look through the `Arc`: in `search_items`, `for x in &c.live {` becomes `for x in c.live.iter() {`.

In the module's test `a_stored_catalog_knows_its_adult_channels_by_the_portals_flag_alone`, the comment `// The rest of the catalog reads through it.` above `assert_eq!(stored.live.len(), 4);` becomes `` // Its live channels are kept apart, in `live`. ``

- [ ] **Step 4: Deduplication on positions, and the ring**

Replace `dedupe_zap` with:

```rust
/// The positions (as `channels` numbers them) of the entries [`dedupe_zap`]
/// keeps: ONE per distinct channel ([`normalize_channel_name`]), the
/// highest-scoring variant, in the order of each channel's first appearance.
fn dedupe<'a>(channels: impl IntoIterator<Item = (usize, &'a LiveChannel)>) -> Vec<usize> {
    let mut order: Vec<String> = Vec::new();
    let mut best: HashMap<String, (usize, &LiveChannel)> = HashMap::new();
    for (i, ch) in channels {
        let key = normalize_channel_name(&ch.name);
        if key.is_empty() {
            continue;
        }
        match best.get(&key) {
            None => {
                order.push(key.clone());
                best.insert(key, (i, ch));
            }
            Some((_, cur)) if variant_score(ch) > variant_score(cur) => {
                best.insert(key, (i, ch));
            }
            Some(_) => {}
        }
    }
    order
        .into_iter()
        .filter_map(|k| best.remove(&k))
        .map(|(i, _)| i)
        .collect()
}

/// Collapse a live-channel list to ONE entry per distinct channel (keyed by
/// [`normalize_channel_name`]), keeping the highest-scoring variant so the
/// user doesn't zap through NPO1 HD / UHD / FHD / catch-up duplicates. Order
/// follows each channel's first appearance in the portal list.
pub fn dedupe_zap<'a>(channels: impl IntoIterator<Item = &'a LiveChannel>) -> Vec<LiveChannel> {
    let channels: Vec<&LiveChannel> = channels.into_iter().collect();
    dedupe(channels.iter().copied().enumerate())
        .into_iter()
        .map(|i| channels[i].clone())
        .collect()
}

/// What up/down zapping steps through: one entry per channel the user zaps to,
/// in zap order. An Xtream portal's variants of a channel are one entry, its
/// best ([`dedupe_zap`]); an M3U list's entries are all there, in the file's
/// order. Adult channels while locked and channels marked defect are out.
/// Worked out once per catalog, lock and marks ([`CatalogStore::zap_ring`]), it
/// holds the catalog's channels and positions in them, never copies: a press
/// copies nothing.
pub struct ZapRing {
    live: Arc<Vec<LiveChannel>>,
    /// Indexes into `live`, in zap order.
    order: Vec<u32>,
    /// An M3U list's ring: `order` keeps the file's order and ascends.
    file_order: bool,
    /// The lock state and the marks revision it was made for.
    unlocked: bool,
    defect_revision: u64,
}

/// Where a zap step starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZapFrom {
    /// Nothing plays: the step counts from the ring's first entry.
    Start,
    /// Ring position `pos`, which held channel `sid` when it was taken. When it
    /// holds another one now (the ring changed), as `Channel(sid)`.
    Pos { pos: usize, sid: i64 },
    /// Channel `sid`, wherever it is.
    Channel(i64),
}

/// Where a channel sits in the ring.
enum Place {
    /// Its entry, or the one that stands for it.
    At(usize),
    /// Out of the ring (an M3U channel marked defect): just before this ring
    /// position, which may be the ring's length.
    Before(usize),
    /// Not in the catalog, or hidden.
    Unknown,
}

impl ZapRing {
    fn build(
        live: &Arc<Vec<LiveChannel>>,
        kind: SourceKind,
        unlocked: bool,
        defect: &HashMap<i64, i64>,
        defect_revision: u64,
    ) -> Self {
        let shows =
            |c: &LiveChannel| (unlocked || !adult_live(c)) && !defect.contains_key(&c.stream_id);
        let entries = live.iter().enumerate().filter(|(_, c)| shows(c));
        let order: Vec<usize> = match kind {
            SourceKind::Xtream => dedupe(entries),
            // Every entry, in the file's order: merging variants is made for the
            // Xtream portal's names.
            SourceKind::M3u => entries.map(|(i, _)| i).collect(),
        };
        Self {
            live: live.clone(),
            order: order.into_iter().map(|i| i as u32).collect(),
            file_order: kind == SourceKind::M3u,
            unlocked,
            defect_revision,
        }
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The channel at ring position `pos`.
    pub fn get(&self, pos: usize) -> Option<&LiveChannel> {
        self.order
            .get(pos)
            .and_then(|&i| self.live.get(i as usize))
    }

    /// The ring's channels, in zap order.
    pub fn channels(&self) -> impl Iterator<Item = &LiveChannel> + '_ {
        self.order
            .iter()
            .filter_map(|&i| self.live.get(i as usize))
    }

    /// Where channel `sid` sits: its own entry, else (on an Xtream portal) the
    /// entry that stands for its channel. None for a channel out of the ring.
    pub fn position_of(&self, sid: i64) -> Option<usize> {
        match self.place_of(sid) {
            Place::At(pos) => Some(pos),
            Place::Before(_) | Place::Unknown => None,
        }
    }

    /// The ring position `delta` entries on from `from`, round the ring; None
    /// for an empty ring. A channel out of the ring (an M3U channel marked
    /// defect) steps from its place in the file: on to the first entry after
    /// it, back to the last one before it. With nothing known the step counts
    /// from the start, as zapping always did ([`crate::shortcuts::next_live_idx`]).
    pub fn step(&self, from: ZapFrom, delta: i32) -> Option<usize> {
        let len = self.order.len();
        if len == 0 {
            return None;
        }
        let place = match from {
            ZapFrom::Start => Place::Unknown,
            ZapFrom::Pos { pos, sid } if self.get(pos).is_some_and(|c| c.stream_id == sid) => {
                Place::At(pos)
            }
            ZapFrom::Pos { sid, .. } | ZapFrom::Channel(sid) => self.place_of(sid),
        };
        match place {
            Place::At(pos) => crate::shortcuts::next_live_idx(Some(pos), len, delta),
            Place::Before(pos) => {
                // One step on lands on `pos`, one back on the entry before it.
                let to = pos as i64 + i64::from(delta) - i64::from(delta > 0);
                Some(to.rem_euclid(len as i64) as usize)
            }
            Place::Unknown => crate::shortcuts::next_live_idx(None, len, delta),
        }
    }

    fn place_of(&self, sid: i64) -> Place {
        let Some(file) = self.live.iter().position(|c| c.stream_id == sid) else {
            return Place::Unknown;
        };
        let own = if self.file_order {
            self.order.binary_search(&(file as u32)).ok()
        } else {
            self.order.iter().position(|&i| i as usize == file)
        };
        if let Some(pos) = own {
            return Place::At(pos);
        }
        let channel = &self.live[file];
        if !self.unlocked && adult_live(channel) {
            return Place::Unknown;
        }
        if self.file_order {
            return Place::Before(self.order.partition_point(|&i| (i as usize) < file));
        }
        let key = normalize_channel_name(&channel.name);
        self.order
            .iter()
            .position(|&i| normalize_channel_name(&self.live[i as usize].name) == key)
            .map_or(Place::Unknown, Place::At)
    }
}
```

`zap_position` stays as it is (its tests use it).

- [ ] **Step 5: The defect marks and the ring in the store**

1. Add two fields at the end of `pub struct CatalogStore`:

```rust
    /// The M3U channels marked defect: stream id → when it failed, unix seconds
    /// (spec 2026-10-15 §4). The zap ring and the channel list's groups leave
    /// them out; the search and the favorites still name them. The app sets them
    /// (`set_defect`); an Xtream catalog has none.
    defect: RwLock<Arc<HashMap<i64, i64>>>,
    /// Counts every `set_defect`: what is worked out from the marks follows it.
    defect_revision: AtomicU64,
```

2. In `CatalogStore::new`, add `defect: RwLock::new(Arc::new(HashMap::new())),` and `defect_revision: AtomicU64::new(0),` to the struct literal.

3. Replace `zap_channels` with:

```rust
    /// What up/down zapping steps through ([`ZapRing`]), `None` until a catalog
    /// has loaded. Worked out once per catalog, lock and marks: a press asks
    /// again and gets the same ring, an `Arc`, with nothing copied.
    pub fn zap_ring(&self) -> Option<Arc<ZapRing>> {
        let unlocked = self.adult_unlocked();
        // The revision before the marks: marks set in between are noticed next time.
        let defect_revision = self.defect_revision();
        let defect = self.defect_marks();
        let g = self.inner.read();
        Some(g.as_ref()?.zap_ring(self.kind(), unlocked, &defect, defect_revision))
    }

    /// The zap ring's channels as a list: a copy, for tests and tools. The app
    /// steps through [`CatalogStore::zap_ring`].
    pub fn zap_channels(&self) -> Vec<LiveChannel> {
        self.zap_ring()
            .map(|ring| ring.channels().cloned().collect())
            .unwrap_or_default()
    }

    /// The name of live channel `sid` when it shows: adult ones while locked do
    /// not; a defect one does (a preset, a favorite or the search still plays
    /// it). One read lock and a scan, no copy of the list.
    pub fn live_name(&self, sid: i64) -> Option<String> {
        let g = self.inner.read();
        g.as_ref()?
            .live
            .iter()
            .find(|c| c.stream_id == sid && self.shows_live(c))
            .map(|c| c.name.clone())
    }

    /// The M3U channels marked defect from now on: stream id → when it failed,
    /// unix seconds. The app hands over every change of its marks.
    pub fn set_defect(&self, marks: HashMap<i64, i64>) {
        *self.defect.write() = Arc::new(marks);
        self.defect_revision.fetch_add(1, Ordering::Release);
    }

    /// Whether channel `sid` is marked defect.
    pub fn is_defect(&self, sid: i64) -> bool {
        self.defect.read().contains_key(&sid)
    }

    /// The marks, as an `Arc`: no copy of the map.
    pub fn defect_marks(&self) -> Arc<HashMap<i64, i64>> {
        self.defect.read().clone()
    }

    /// How many times the marks were set: 0 until the first `set_defect`.
    pub fn defect_revision(&self) -> u64 {
        self.defect_revision.load(Ordering::Acquire)
    }
```

- [ ] **Step 6: The M3U timing test times a press of the ring**

The ignored `an_eleven_thousand_channel_m3u_catalog_timed` in `rust/tests/test_catalog_m3u.rs` times `zap_channels()` as the cost of an up/down press, and spec §5 cites that measurement. A press now asks `zap_ring()`, so the test times that too. Check the file with `fmt tests/test_catalog_m3u.rs` before and after: no output either time.

1. Its doc comment's "(the zap list: every key repeat clones the whole list)" becomes "(the zap ring, an `Arc`: nothing is copied; `zap_channels` is a copy, for tests and tools)", so the comment reads:

```rust
/// Timed by hand: what an M3U catalog of iptv-org's size costs where it is
/// stored (the guide's merge, once for every catalog put: the saved list and
/// the fresh one) and per up/down press (the zap ring, an `Arc`: nothing is
/// copied; `zap_channels` is a copy, for tests and tools).
/// `cargo test --release --test test_catalog_m3u -- --ignored --nocapture`
```

2. After its `print_timing("zap_channels", 100, …);` call, at the end of the test, add:

```rust
    print_timing("zap_ring (a press)", 100, || {
        assert_eq!(
            std::hint::black_box(store.zap_ring()).map(|r| r.len()),
            Some(11_262)
        );
    });
```

- [ ] **Step 7: Run the tests to see them pass**

Run: `ct --test test_zap_ring --test test_catalog_m3u --test test_catalog_adult --test test_guide_channels`
Expected: all pass. The old ones go through `zap_channels`, now a copy of the ring, and must come out as before. The timing test is ignored: it builds here, and Task 10 runs it in a release build.

Run: `ct --lib catalog::`
Expected: all pass (the module's own tests, `dedupe_zap` among them).

- [ ] **Step 8: Format, lint, full suite**

- Run `fmt tests/test_zap_ring.rs tests/test_catalog_m3u.rs`. Expected: no output.
- Check the hunk count of `catalog.rs`: 3, as at the baseline (Task 1).
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes.

- [ ] **Step 9: Commit**

Message: `feat(rust): the zap ring: positions in one shared channel list, once per catalog, lock and marks; the catalog's defect filter`, followed by the two attribution lines.
Files: `rust/src/catalog.rs`, `rust/tests/test_zap_ring.rs`, `rust/tests/test_catalog_m3u.rs`.

---

### Task 3: App: zapping through the ring

Up/down and every pick by id go through the ring: a press copies no channel list (it used to clone every channel of an M3U list, and deduplicate ~13,000 Xtream channels). A marked channel that plays zaps on from its place in the file. A start that comes due for the channel that plays already loads nothing. A flip to a channel that a fresh copy of the list dropped leaves no toast of it.

**Files:**
- Modify: `rust/src/app.rs`

**Interfaces:**
- Consumes (Task 2): `CatalogStore::{zap_ring, live_name, set_defect}`, `ZapRing::{is_empty, get, position_of, step}`, `ZapFrom`.
- Produces (`app.rs`):
  ```rust
  // TvApp field: zap_pacer: ZapPacer<(i64, String, Option<usize>)>   (sid, name, ring position)
  fn plays(&self, sid: i64) -> bool   // current, and not ended with an error
  // tests: const RING: &str (Kanaal A..D in file order), fn m3u_sid(name: &str) -> i64,
  //        fn ring_url(letter: char) -> String
  ```
  `zap_index_of_current` goes; `app.rs` no longer imports `zap_position`, `crate::shortcuts` or, outside its tests, `LiveChannel`. `zap_to` keeps its signature; a target with no address takes the flip's toast of it down.

- [ ] **Step 1: Write the failing tests**

In `rust/src/app.rs`'s `mod tests`, after the section `// ---- an M3U source ----` and its tests, add:

```rust
    // ---- zapping through the ring ----

    /// Four channels, A to D, in this order. With no tvg-id their keys, and so
    /// their stream ids, are their names.
    const RING: &str = "#EXTM3U\n\
#EXTINF:-1,Kanaal A\nhttps://a.example/a.m3u8\n\
#EXTINF:-1,Kanaal B\nhttps://b.example/b.m3u8\n\
#EXTINF:-1,Kanaal C\nhttps://c.example/c.m3u8\n\
#EXTINF:-1,Kanaal D\nhttps://d.example/d.m3u8\n";

    /// The stream id of a channel of a list whose channels have no tvg-id.
    fn m3u_sid(name: &str) -> i64 {
        crate::m3u::stream_id(name)
    }

    /// Where channel `letter` of `RING` plays from.
    fn ring_url(letter: char) -> String {
        format!("https://{letter}.example/{letter}.m3u8")
    }

    #[tokio::test]
    async fn zapping_skips_a_defect_channel_both_ways() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), RING);
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        h.app.catalog.set_defect([(m3u_sid("Kanaal B"), 0)].into());
        h.app.zap_by_id(m3u_sid("Kanaal A"));
        h.sent();
        h.press(&[Key::ArrowDown]);
        assert_eq!(loads(h.sent()), [ring_url('c')], "past B");
        h.press(&[Key::ArrowUp]);
        assert_eq!(loads(h.sent()), [ring_url('a')], "and back past B");
    }

    #[tokio::test]
    async fn a_defect_channel_that_plays_zaps_on_from_its_place_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), RING);
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        h.app.catalog.set_defect([(m3u_sid("Kanaal B"), 0)].into());
        // A preset, a favorite or the search still plays it.
        h.app.zap_by_id(m3u_sid("Kanaal B"));
        assert_eq!(loads(h.sent()), [ring_url('b')]);
        assert_eq!(h.app.current_idx, None, "it has no place in the ring");
        h.press(&[Key::ArrowDown]);
        assert_eq!(loads(h.sent()), [ring_url('c')], "the one after it in the file");
        h.app.zap_by_id(m3u_sid("Kanaal B"));
        h.sent();
        h.press(&[Key::ArrowUp]);
        assert_eq!(loads(h.sent()), [ring_url('a')], "the one before it");
    }

    #[tokio::test]
    async fn flipping_away_and_back_to_the_channel_that_plays_loads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), RING);
        h.app.zap_pacer = ZapPacer::new(Duration::from_millis(300));
        h.app.zap_by_id(m3u_sid("Kanaal A"));
        h.sent();
        // A lone press: B at once. A held key then overshoots to C and comes back.
        h.frame(&[Key::ArrowDown]);
        h.frame(&[Key::ArrowDown]);
        h.frame(&[Key::ArrowUp]);
        assert_eq!(loads(h.sent()), [ring_url('b')]);
        std::thread::sleep(Duration::from_millis(350));
        h.frame(&[]);
        assert!(loads(h.sent()).is_empty(), "B plays on");
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal B"));
    }

    #[tokio::test]
    async fn a_hidden_channel_is_not_played_by_its_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        h.sent();
        h.app.zap_by_id(901); // adult, locked
        assert!(loads(h.sent()).is_empty());
        h.app.zap_by_id(3);
        assert_eq!(loads(h.sent()), [live_url(3)]);
        assert_eq!(h.app.current_idx, Some(0));
    }

    #[tokio::test]
    async fn a_flip_to_a_channel_the_fresh_list_dropped_leaves_no_toast_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), RING);
        h.app.zap_pacer = ZapPacer::new(Duration::from_millis(300));
        h.app.zap_by_id(m3u_sid("Kanaal A"));
        // A lone press: B at once. The held key then flips on to C, which waits.
        h.frame(&[Key::ArrowDown]);
        h.frame(&[Key::ArrowDown]);
        let toast = |h: &Harness| h.app.toast.as_ref().map(|(t, _, _)| t.clone());
        assert_eq!(toast(&h).as_deref(), Some("[TV] Kanaal C"), "the flip");
        // The fresh copy of the list has no Kanaal C.
        let fresh = RING.replace("#EXTINF:-1,Kanaal C\nhttps://c.example/c.m3u8\n", "");
        let (portal, catalog) = M3uPortal::from_text(&fresh).unwrap();
        h.app.catalog = Arc::new(CatalogStore::from_catalog(Arc::new(portal), catalog));
        h.sent();
        std::thread::sleep(Duration::from_millis(350));
        h.frame(&[]);
        assert!(loads(h.sent()).is_empty(), "C has no address now");
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal B"));
        // No toast names the channel that vanished.
        let named = |t: &String| t.contains("Kanaal C");
        assert!(toast(&h).filter(named).is_none(), "{:?}", toast(&h));
        assert!(!h.painted.iter().any(named), "{:?}", h.painted);
    }
```

(`h.frame(&[Key::ArrowDown])` twice in a row: the harness never lets a key go, so egui marks the second press a repeat, as a held key.)

- [ ] **Step 2: Run them to see them fail**

Run: `ct --lib app::tests::`
Expected:
- `a_defect_channel_that_plays_zaps_on_from_its_place_in_the_file` fails: the old code steps from the list's start for a channel that is not in it, so Up from B lands on D.
- `flipping_away_and_back_to_the_channel_that_plays_loads_nothing` fails: B loads a second time.
- `a_flip_to_a_channel_the_fresh_list_dropped_leaves_no_toast_of_it` fails: `zap_to` drops C, which has no address now, but the flip's "[TV] Kanaal C" stays up over B.
- `zapping_skips_a_defect_channel_both_ways` passes already (Task 2's `zap_channels` leaves B out) and guards the change; `a_hidden_channel_is_not_played_by_its_id` passes before and after.

- [ ] **Step 3: Zapping through the ring**

In `rust/src/app.rs`:

1. Imports: add `ZapFrom` to the `use crate::catalog::{…}` list and remove `zap_position` from it; remove `use crate::shortcuts;`; take `LiveChannel` out of `use crate::portal::{xtream::XtreamPortal, LiveChannel, Portal};` (its one use outside the tests was `zap_index_of_current`) and add `use crate::portal::LiveChannel;` to `mod tests`, under `use super::*;`.

2. The field `zap_pacer` becomes:

```rust
    /// Paces up/down zapping: quick presses or a held arrow key only move the
    /// target (sid, name, ring position; none for a pick in the channel list);
    /// the channel the user stops on loads.
    zap_pacer: ZapPacer<(i64, String, Option<usize>)>,
```

3. Delete `fn zap_index_of_current`.

4. Replace `fn zap_delta` with:

```rust
    /// Up/down zapping. A lone press zaps at once; while the user flips
    /// (quick presses, a held key) only the channel names show and the one
    /// they stop on loads - a portal session per keypress gets throttled.
    fn zap_delta(&mut self, delta: i32, repeat: bool) {
        // The ring: one entry per channel (an Xtream portal's variants merged,
        // the defect channels of an M3U list out), worked out once per catalog,
        // lock and marks, so a press copies nothing.
        let ring = match self.catalog.zap_ring() {
            Some(ring) if !ring.is_empty() => ring,
            _ => {
                let key = if self.catalog.is_loaded() {
                    T::NoLiveChannels
                } else {
                    T::CatalogLoading
                };
                self.set_toast(self.t(key));
                return;
            }
        };
        // From the channel a flip waits on, else from the one that plays: its
        // ring position while that still holds it (the list may have changed
        // under it), else the channel itself; with nothing, from the start.
        let from = match self.zap_pacer.waiting() {
            Some((sid, _, Some(pos))) => ZapFrom::Pos {
                pos: *pos,
                sid: *sid,
            },
            Some((sid, _, None)) => ZapFrom::Channel(*sid),
            None => match (self.current_idx, self.current_stream_id) {
                (Some(pos), Some(sid)) => ZapFrom::Pos { pos, sid },
                (None, Some(sid)) => ZapFrom::Channel(sid),
                (_, None) => ZapFrom::Start,
            },
        };
        let Some((pos, channel)) = ring
            .step(from, delta)
            .and_then(|pos| ring.get(pos).map(|c| (pos, c)))
        else {
            return;
        };
        let (sid, name) = (channel.stream_id, channel.name.clone());
        match self
            .zap_pacer
            .press(Instant::now(), (sid, name.clone(), Some(pos)), repeat)
        {
            Some((sid, name, idx)) => self.zap_to(sid, &name, idx),
            None => self.set_toast(format!("[TV] {}", name)),
        }
    }
```

5. Replace `fn zap_by_id` with:

```rust
    fn zap_by_id(&mut self, sid: i64) {
        // One look at the catalog, no copy of its list: the name of a channel
        // that shows (a defect one does: a preset or a favorite plays it).
        let Some(name) = self.catalog.live_name(sid) else {
            return;
        };
        // Up/down then go on from here: its own entry in the ring (on an M3U
        // list two channels may share a name), else the one that stands for it
        // in the deduped Xtream list. A channel out of the ring (marked defect)
        // has none: zapping steps from its place in the file.
        let idx = self
            .catalog
            .zap_ring()
            .and_then(|ring| ring.position_of(sid));
        self.zap_to(sid, &name, idx);
    }
```

6. After `fn is_refused`, add:

```rust
    /// Whether live channel `sid` is what plays and has not ended with an error:
    /// a start of it now would only start it over. One that failed is no
    /// longer playing, so a flip back to it, or Enter in the channel list,
    /// tries it again.
    fn plays(&self, sid: i64) -> bool {
        self.current_stream_id == Some(sid) && self.stall_watchdog_armed
    }
```

7. In `update_ui`, replace the block after `// The channel the user stopped flipping on.` with:

```rust
        // The channel the user stopped flipping on; not when it is the one that
        // plays (flipped away and back): a second load would only re-buffer it.
        if let Some((sid, name, idx)) = self.zap_pacer.due(Instant::now()) {
            if self.plays(sid) {
                tracing::debug!("zap: {} plays already", crate::redact::redact(&name));
                if idx.is_some() {
                    self.current_idx = idx;
                }
            } else {
                self.zap_to(sid, &name, idx);
            }
        }
```

8. In `zap_to`, the guard for a target with no address, `if url.is_empty() { … }` with the comment above it, becomes:

```rust
        // An M3U list has no address for an id it does not hold: a target the
        // zap pacer held while a fresh list dropped it. What plays stays, and so
        // would the flip's toast that named the target, over the channel that
        // plays on: it goes.
        if url.is_empty() {
            tracing::warn!("zap: stream_id={} has no address - not loaded", sid);
            let flip = format!("[TV] {}", name);
            if self.toast.as_ref().is_some_and(|t| t.0 == flip) {
                self.toast = None;
            }
            return;
        }
```

Only that toast goes: one set since the flip (a stream error, a lock) stays.

- [ ] **Step 4: Run the tests to see them pass**

Run: `ct --lib app::tests::`
Expected: all pass, the old zapping tests among them (`an_xtream_portal_zaps_on_from_the_channel_after_a_quality_switch_or_a_variant`, `an_xtream_resume_of_a_variant_zaps_on_from_its_channel`, `up_after_a_resume_continues_from_the_resumed_m3u_channel`, `zapping_after_the_list_changed_under_it_goes_on_from_the_channel_that_plays`, and `a_channel_the_list_no_longer_has_loads_nothing_and_is_not_the_last_channel`, which goes through the guard item 8 changed). If one fails, the ring's `step` or `position_of` differs from the old `zap_index_of_current`/`zap_position` rule: fix the app code, not the test, and say so in your report.

- [ ] **Step 5: Format, lint, full suite**

- Check the hunk count of `app.rs`: 7, as at the baseline (Task 1).
- Run `cc`. Expected: only the 2 old warnings (no unused import).
- Run `ct`. Expected: everything passes.

- [ ] **Step 6: Commit**

Message: `feat(rust): zap through the ring: no list copied per press, a defect channel zaps on from its place, a flip back to the channel that plays loads nothing, a flip to one the list dropped leaves no toast`, followed by the two attribution lines.
Files: `rust/src/app.rs`.

---

### Task 4: The check before a mark

A confirmed suspect is not marked at once: its address is asked once more (spec §4.2, ruling 28). One GET with the channel's own headers, at most 5 s for the connection and the headers, the status only; the body is never read (a live stream never ends). A small checker owns the HTTP client (built on the first check), runs at most 4 checks at once and one per channel, and keeps the answers in a slot the app drains once per frame. It knows nothing of the marks: Task 5 wires it. A pure `same_server` says whether two addresses are on one server, so the app never asks the server of what plays (ruling 28b).

**Files:**
- Create: `rust/src/stream_check.rs`
- Modify: `rust/src/lib.rs` (`pub mod stream_check;` after `pub mod storage;`)
- Test: `rust/tests/test_stream_check.rs` (new), `rust/tests/test_mpv_smoke.rs` (mpv's User-Agent pinned)

**Interfaces:**
- Consumes: `crate::m3u::split_headers(url) -> (&str, Headers { user_agent, referrer })`, the split `player::loadfile_args` hands to mpv; `crate::m3u::with_headers` (tests); reqwest's `ClientBuilder::danger_accept_invalid_certs`, `header::HeaderValue::from_bytes` and `Url` (the `url` crate's, handed on by reqwest).
- Produces:
  ```rust
  // stream_check.rs
  pub const CHECK_TIMEOUT: Duration;   // 5 s: the connection and the headers
  pub const AT_ONCE: usize;            // 4
  pub const MPV_USER_AGENT: &str;      // "libmpv": what mpv sends when a channel names none
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Answer { Answers(u16), Status(u16), ConnectFailed, TimedOut, Failed, Unchecked }
  impl Answer { pub fn is_dead(self) -> bool; pub fn kind(self) -> String; }
  pub struct Check { pub stream_id: i64, pub name: String, pub failed_unix: i64, pub url: String }  // no Debug
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Checked { pub stream_id: i64, pub name: String, pub failed_unix: i64, pub answer: Answer }
  pub struct StreamChecker;            // also Default
  impl StreamChecker {
      pub fn new() -> Self;            // builds nothing
      pub fn start(&mut self, check: Check);
      pub fn drain(&mut self) -> Vec<Checked>;
      pub fn forget(&mut self, stream_id: i64);
      pub fn reset(&mut self);
      pub fn checking(&self, stream_id: i64) -> bool;
      pub fn in_flight(&self) -> usize;
  }
  pub fn http_client() -> reqwest::Result<reqwest::Client>;  // no certificate check, as mpv's tls-verify=no
  pub async fn ask(client: &reqwest::Client, url: &str) -> Answer;  // header values as bytes
  pub fn same_server(a: &str, b: &str) -> bool;  // scheme, host (any case), port with its default; no path, query or `|…`
  ```
  `Answer::is_dead` is false for `Answers` and `Unchecked`: an address the check cannot ask is not marked (ruling 29).
  Log line (warn, once): `m3u: no HTTP client for the defect check (<error>) - no channel is marked`.

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_stream_check.rs`:

```rust
//! The check before a defect mark (stream_check.rs): one GET of a confirmed
//! suspect's address, with its own headers, the status only; and which
//! addresses are on one server.

use bongo_player::m3u::with_headers;
use bongo_player::stream_check::{
    ask, http_client, same_server, Answer, Check, Checked, StreamChecker, AT_ONCE, MPV_USER_AGENT,
};
use std::time::Duration;
use wiremock::matchers::{any, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 15 October 2026, 00:00 UTC.
const FAILED: i64 = 1_792_022_400;

/// A server that answers every request with `status`, after `delay`.
async fn answering(status: u16, delay: Duration) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(status).set_delay(delay))
        .mount(&server)
        .await;
    server
}

/// A port on this machine that nothing listens on.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn check(stream_id: i64, url: String) -> Check {
    Check {
        stream_id,
        name: format!("K{stream_id}"),
        failed_unix: FAILED,
        url,
    }
}

/// Drains `checker` until `n` answers came (5 s at most).
async fn answers(checker: &mut StreamChecker, n: usize) -> Vec<Checked> {
    let mut got = Vec::new();
    for _ in 0..1000 {
        got.extend(checker.drain());
        if got.len() >= n {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{} of {n} answers came", got.len());
}

#[tokio::test]
async fn an_address_that_answers_200_answers_again() {
    let server = answering(200, Duration::ZERO).await;
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("{}/live.m3u8", server.uri())).await;
    assert_eq!(answer, Answer::Answers(200));
    assert!(!answer.is_dead());
}

#[tokio::test]
async fn an_error_status_after_redirects_is_dead_and_a_redirect_to_a_live_one_is_not() {
    let server = MockServer::start().await;
    let to = |place: &str| ResponseTemplate::new(302).insert_header("Location", place);
    Mock::given(path("/dead")).respond_with(to("/gone")).mount(&server).await;
    Mock::given(path("/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(path("/moved")).respond_with(to("/here")).mount(&server).await;
    Mock::given(path("/here"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let client = http_client().unwrap();
    let dead = ask(&client, &format!("{}/dead", server.uri())).await;
    assert_eq!(dead, Answer::Status(404));
    assert!(dead.is_dead());
    assert_eq!(dead.kind(), "http 404");
    let moved = ask(&client, &format!("{}/moved", server.uri())).await;
    assert_eq!(moved, Answer::Answers(200));
}

#[tokio::test]
async fn a_closed_port_is_a_failed_connection() {
    let client = http_client().unwrap();
    let url = format!("http://127.0.0.1:{}/live.m3u8", closed_port());
    let answer = ask(&client, &url).await;
    assert_eq!(answer, Answer::ConnectFailed);
    assert!(answer.is_dead());
    assert_eq!(answer.kind(), "connect failed");
}

#[tokio::test]
async fn no_headers_within_the_time_is_a_timeout() {
    let server = answering(200, Duration::from_secs(3)).await;
    // The check's own client waits CHECK_TIMEOUT (5 s); this one 200 ms.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let answer = ask(&client, &format!("{}/slow", server.uri())).await;
    assert_eq!(answer, Answer::TimedOut);
    assert!(answer.is_dead());
    assert_eq!(answer.kind(), "timeout");
}

#[tokio::test]
async fn the_check_sends_the_channels_own_user_agent_and_referer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/twee.m3u8"))
        .and(header("user-agent", "Bongo/1.0"))
        .and(header("referer", "https://www.example.com/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    // Anything else gets wiremock's 404, which the check would call dead.
    let url = with_headers(
        format!("{}/twee.m3u8", server.uri()),
        Some("Bongo/1.0"),
        Some("https://www.example.com/"),
    );
    let client = http_client().unwrap();
    assert_eq!(ask(&client, &url).await, Answer::Answers(200));
}

#[tokio::test]
async fn without_a_user_agent_of_its_own_the_check_sends_mpvs() {
    let server = MockServer::start().await;
    Mock::given(header("user-agent", MPV_USER_AGENT))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let client = http_client().unwrap();
    let answer = ask(&client, &format!("{}/een.m3u8", server.uri())).await;
    assert_eq!(answer, Answer::Answers(200));
}

#[tokio::test]
async fn a_user_agent_that_is_not_ascii_is_sent_as_its_bytes() {
    let server = answering(200, Duration::ZERO).await;
    // mpv hands a "Zürich-TV" User-Agent to the server as its UTF-8 bytes.
    let url = with_headers(
        format!("{}/zurich.m3u8", server.uri()),
        Some("Zürich-TV"),
        None,
    );
    let client = http_client().unwrap();
    assert_eq!(ask(&client, &url).await, Answer::Answers(200));
    let asked = server.received_requests().await.unwrap();
    let agent = asked[0].headers.get("user-agent").map(|v| v.as_bytes());
    assert_eq!(agent, Some("Zürich-TV".as_bytes()));
}

#[tokio::test]
async fn a_redirect_hands_the_address_on_to_nobody() {
    let server = MockServer::start().await;
    Mock::given(path("/a"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/b"))
        .mount(&server)
        .await;
    Mock::given(path("/b"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let client = http_client().unwrap();
    let url = format!("{}/a?token=geheim", server.uri());
    assert_eq!(ask(&client, &url).await, Answer::Answers(200));
    let asked = server.received_requests().await.unwrap();
    assert_eq!(asked.len(), 2);
    assert!(
        asked[1].headers.get("referer").is_none(),
        "no Referer of the check's own: {:?}",
        asked[1].headers
    );
}

#[test]
fn what_the_log_says_of_an_answer_names_no_address() {
    assert_eq!(Answer::Status(404).kind(), "http 404");
    assert_eq!(Answer::Answers(302).kind(), "http 302");
    assert_eq!(Answer::ConnectFailed.kind(), "connect failed");
    assert_eq!(Answer::TimedOut.kind(), "timeout");
    assert_eq!(Answer::Failed.kind(), "request failed");
    assert_eq!(Answer::Unchecked.kind(), "an address it cannot ask");
    assert!(!Answer::Answers(302).is_dead());
    assert!(!Answer::Unchecked.is_dead(), "no check, no mark");
}

#[tokio::test]
async fn four_checks_at_a_time_and_the_next_when_one_answers() {
    let server = answering(404, Duration::from_millis(300)).await;
    let mut checker = StreamChecker::new();
    for id in 1..=6 {
        checker.start(check(id, format!("{}/{id}", server.uri())));
    }
    assert_eq!(checker.in_flight(), AT_ONCE);
    assert!(checker.checking(6), "the sixth waits for a place");
    let got = answers(&mut checker, 6).await;
    let mut ids: Vec<i64> = got.iter().map(|c| c.stream_id).collect();
    ids.sort();
    assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
    assert!(got
        .iter()
        .all(|c| c.answer == Answer::Status(404) && c.failed_unix == FAILED));
    assert_eq!(checker.in_flight(), 0);
    let asked = server.received_requests().await.unwrap();
    assert_eq!(asked.len(), 6, "one request per channel");
}

#[tokio::test]
async fn one_check_per_channel() {
    let server = answering(404, Duration::from_millis(100)).await;
    let mut checker = StreamChecker::new();
    checker.start(check(1, format!("{}/1", server.uri())));
    checker.start(check(1, format!("{}/1", server.uri())));
    assert_eq!(checker.in_flight(), 1);
    assert_eq!(answers(&mut checker, 1).await.len(), 1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(checker.drain().is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_forgotten_or_reset_check_answers_nothing() {
    let server = answering(404, Duration::from_millis(200)).await;
    let mut checker = StreamChecker::new();
    for id in 1..=3 {
        checker.start(check(id, format!("{}/{id}", server.uri())));
    }
    checker.forget(1);
    assert!(!checker.checking(1));
    assert_eq!(checker.in_flight(), 2);
    checker.reset();
    assert_eq!(checker.in_flight(), 0);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(checker.drain().is_empty(), "their answers are dropped");
}

#[tokio::test]
async fn an_address_that_is_no_web_address_is_not_asked() {
    let mut checker = StreamChecker::new();
    checker.start(check(9, "rtmp://a.example/live/9".into()));
    let got = answers(&mut checker, 1).await;
    assert_eq!(got[0].answer, Answer::Unchecked);
}

// ---- one server or two (the app never asks the server of what plays) ----

#[test]
fn the_same_host_and_port_with_another_path_is_the_same_server() {
    assert!(same_server(
        "http://tv.example:8080/live/1.m3u8",
        "http://tv.example:8080/live/2.ts?token=abc"
    ));
}

#[test]
fn another_port_is_another_server() {
    assert!(!same_server(
        "http://tv.example:8080/1.m3u8",
        "http://tv.example:8081/1.m3u8"
    ));
}

#[test]
fn a_default_port_written_out_is_the_port_left_out() {
    assert!(same_server(
        "http://tv.example:80/1.m3u8",
        "http://tv.example/2.m3u8"
    ));
    assert!(same_server(
        "https://tv.example/1.m3u8",
        "https://tv.example:443/2.m3u8"
    ));
}

#[test]
fn the_letter_case_of_the_host_does_not_count() {
    assert!(same_server(
        "http://TV.Example/1.m3u8",
        "http://tv.example/2.m3u8"
    ));
}

#[test]
fn the_headers_after_a_pipe_do_not_count() {
    // Right after the port: read as part of the address, they would make an
    // address that cannot be read.
    let headed = with_headers(
        "http://tv.example:8080".into(),
        Some("Bongo/1.0"),
        Some("https://www.example.com/"),
    );
    assert!(same_server(&headed, "http://tv.example:8080/live/2.m3u8"));
}

#[test]
fn http_and_https_on_one_host_are_two_servers() {
    assert!(!same_server(
        "http://tv.example/1.m3u8",
        "https://tv.example/1.m3u8"
    ));
}

#[test]
fn an_address_that_cannot_be_read_is_no_server() {
    assert!(!same_server("tv.example/1.m3u8", "tv.example/1.m3u8"));
    assert!(!same_server("http://tv.example/1.m3u8", ""));
}
```

Run `rustfmt --edition 2021 tests/test_stream_check.rs` once written.

In `rust/tests/test_mpv_smoke.rs`, add:

```rust
#[test]
#[ignore]
fn the_defect_check_sends_mpvs_own_user_agent() {
    // A channel that names no User-Agent is asked with the one mpv sent it.
    let mpv = Mpv::new().expect("mpv init");
    assert_eq!(
        mpv.get_property_string("user-agent").as_deref(),
        Some(bongo_player::stream_check::MPV_USER_AGENT),
        "set stream_check::MPV_USER_AGENT to what mpv says"
    );
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_stream_check`
Expected: compile errors: no module `stream_check`.

- [ ] **Step 3: Write the module**

Create `rust/src/stream_check.rs`:

```rust
//! The check before a defect mark (spec 2026-10-15 §4.2). An M3U channel that
//! failed before its first picture, and that another channel's picture then
//! confirmed, is not marked at once: its address is asked once more. One GET
//! with the channel's own headers, the status only, a short limit, and as mpv
//! asks: no certificate check, header values as bytes. A dead answer (a status
//! of 400 or more after redirects, no connection, no answer in time) marks it;
//! a live one (2xx, 3xx) drops the suspicion, so a Wi-Fi drop of under two
//! minutes marks no good channel. An address it cannot ask is not marked
//! either: no check, no mark. At most one check per channel and [`AT_ONCE`] at
//! a time, on the app's tokio runtime; the answers wait in a slot the app
//! drains once per frame. The app never asks the server of the stream that
//! plays ([`same_server`]). An address never reaches a log line or a `Debug`
//! print: a paid list's addresses hold credentials.

use parking_lot::Mutex;
use reqwest::header::HeaderValue;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

/// How long a check waits for the connection and the answer's headers.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Checks on their way at once.
pub const AT_ONCE: usize = 4;

/// What mpv sends as its User-Agent when a channel names none: the vendored
/// libmpv's default (`tests/test_mpv_smoke.rs` pins it). The check says the
/// same, so the server answers it as it answered the player.
pub const MPV_USER_AGENT: &str = "libmpv";

/// What a channel's address answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// 2xx or 3xx after the redirects: it answers again.
    Answers(u16),
    /// A status of 400 or more after the redirects.
    Status(u16),
    /// No connection: refused, a name that does not resolve, a failed TLS
    /// handshake.
    ConnectFailed,
    /// No headers within [`CHECK_TIMEOUT`].
    TimedOut,
    /// The request failed otherwise (an address it cannot send).
    Failed,
    /// Not asked: no http(s) address, or no HTTP client. No mark then (spec
    /// §4.2). Nor is an address asked with a header value that cannot be sent
    /// (a control character).
    Unchecked,
}

impl Answer {
    /// Whether the channel gets the mark: not when it answers again, and not
    /// when it was not asked.
    pub fn is_dead(self) -> bool {
        !matches!(self, Answer::Answers(_) | Answer::Unchecked)
    }

    /// What the log says of it, never the address: "http 404", "connect
    /// failed", "timeout".
    pub fn kind(self) -> String {
        match self {
            Answer::Answers(status) | Answer::Status(status) => format!("http {status}"),
            Answer::ConnectFailed => "connect failed".to_owned(),
            Answer::TimedOut => "timeout".to_owned(),
            Answer::Failed => "request failed".to_owned(),
            Answer::Unchecked => "an address it cannot ask".to_owned(),
        }
    }
}

/// A check to make. No `Debug`: the address of a paid list holds credentials.
pub struct Check {
    pub stream_id: i64,
    /// For the app's log line.
    pub name: String,
    /// When the channel failed: its mark keeps this time.
    pub failed_unix: i64,
    /// What the channel plays, with its headers (`url|User-Agent=…`).
    pub url: String,
}

/// A check's answer, for the app; no address in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub stream_id: i64,
    pub name: String,
    pub failed_unix: i64,
    pub answer: Answer,
}

/// The checks of the source in use.
pub struct StreamChecker {
    /// Built on the first check, not before: a session without one pays
    /// nothing for it.
    client: Option<reqwest::Client>,
    /// The client could not be built (warned once): no check asks then.
    client_failed: bool,
    /// Checks on their way: stream id → the ticket its answer must carry, and
    /// its task, aborted when the check is dropped.
    in_flight: HashMap<i64, (u64, tokio::task::JoinHandle<()>)>,
    /// Checks waiting for one of the [`AT_ONCE`] places.
    waiting: VecDeque<Check>,
    next_ticket: u64,
    /// Answers that came, with their tickets, until `drain` takes them.
    done: Arc<Mutex<Vec<(u64, Checked)>>>,
}

impl Default for StreamChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamChecker {
    /// No check yet; nothing is built.
    pub fn new() -> Self {
        Self {
            client: None,
            client_failed: false,
            in_flight: HashMap::new(),
            waiting: VecDeque::new(),
            next_ticket: 0,
            done: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Asks the address of `check` once, in the background: now when fewer
    /// than [`AT_ONCE`] run, else when a place frees. Not a second time while
    /// a check of that channel runs or waits.
    pub fn start(&mut self, check: Check) {
        if self.checking(check.stream_id) {
            return;
        }
        if self.in_flight.len() < AT_ONCE {
            self.launch(check);
        } else {
            self.waiting.push_back(check);
        }
    }

    /// The answers that came since the last call, of the checks not dropped
    /// since; their places go to checks that wait. Once per frame.
    pub fn drain(&mut self) -> Vec<Checked> {
        let arrived = std::mem::take(&mut *self.done.lock());
        let mut answers = Vec::new();
        for (ticket, checked) in arrived {
            let ours = self
                .in_flight
                .get(&checked.stream_id)
                .is_some_and(|(t, _)| *t == ticket);
            if ours {
                self.in_flight.remove(&checked.stream_id);
                answers.push(checked);
            }
        }
        while self.in_flight.len() < AT_ONCE {
            let Some(next) = self.waiting.pop_front() else {
                break;
            };
            self.launch(next);
        }
        answers
    }

    /// Drops channel `stream_id`'s check: it showed a picture, and its answer
    /// means nothing now.
    pub fn forget(&mut self, stream_id: i64) {
        if let Some((_, task)) = self.in_flight.remove(&stream_id) {
            task.abort();
        }
        self.waiting.retain(|c| c.stream_id != stream_id);
    }

    /// Drops every check: another source is in use, and their answers are
    /// ignored.
    pub fn reset(&mut self) {
        for (_, (_, task)) in self.in_flight.drain() {
            task.abort();
        }
        self.waiting.clear();
    }

    /// Whether a check of channel `stream_id` runs or waits.
    pub fn checking(&self, stream_id: i64) -> bool {
        self.in_flight.contains_key(&stream_id)
            || self.waiting.iter().any(|c| c.stream_id == stream_id)
    }

    /// Checks on their way, not counting the ones that wait.
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    fn launch(&mut self, check: Check) {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let client = self.http_client();
        let done = self.done.clone();
        let stream_id = check.stream_id;
        let task = tokio::spawn(async move {
            let web = check.url.starts_with("http://") || check.url.starts_with("https://");
            let answer = match client.filter(|_| web) {
                Some(client) => ask(&client, &check.url).await,
                None => Answer::Unchecked,
            };
            let checked = Checked {
                stream_id: check.stream_id,
                name: check.name,
                failed_unix: check.failed_unix,
                answer,
            };
            done.lock().push((ticket, checked));
        });
        self.in_flight.insert(stream_id, (ticket, task));
    }

    /// The HTTP client, built on the first check. `None` when it cannot be
    /// built (warned once): the checks then answer `Unchecked`.
    fn http_client(&mut self) -> Option<reqwest::Client> {
        if self.client.is_none() && !self.client_failed {
            match http_client() {
                Ok(client) => self.client = Some(client),
                Err(e) => {
                    self.client_failed = true;
                    tracing::warn!(
                        "m3u: no HTTP client for the defect check ({e}) - no channel is marked"
                    );
                }
            }
        }
        self.client.clone()
    }
}

/// The check's HTTP client: [`CHECK_TIMEOUT`] for the connection and the
/// headers (the body is never read, so the limit ends there), redirects
/// followed, no `Referer` of its own on a redirect (that would hand a
/// tokenized address on to another server), and no certificate check.
pub fn http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CHECK_TIMEOUT)
        .timeout(CHECK_TIMEOUT)
        .referer(false)
        // As mpv, which runs with tls-verify=no; the M3U maturity round turns
        // verification on for both, with one ini escape for both.
        .danger_accept_invalid_certs(true)
        .build()
}

/// Asks `url` once, with the headers its `|…` suffix names
/// ([`crate::m3u::split_headers`], as `player::loadfile_args` hands them to
/// mpv), and mpv's own User-Agent when it names none. The header values go as
/// their bytes, as mpv sends them, so a "Zürich-TV" User-Agent is no error; a
/// value that still cannot be sent (a control character) is not asked. Only
/// the status counts: the body (a live stream never ends) is not read.
pub async fn ask(client: &reqwest::Client, url: &str) -> Answer {
    let (address, headers) = crate::m3u::split_headers(url);
    let agent = headers.user_agent.as_deref().unwrap_or(MPV_USER_AGENT);
    let Ok(agent) = HeaderValue::from_bytes(agent.as_bytes()) else {
        return Answer::Unchecked;
    };
    let mut request = client
        .get(address)
        .header(reqwest::header::USER_AGENT, agent);
    if let Some(referrer) = &headers.referrer {
        let Ok(referrer) = HeaderValue::from_bytes(referrer.as_bytes()) else {
            return Answer::Unchecked;
        };
        request = request.header(reqwest::header::REFERER, referrer);
    }
    match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            if status >= 400 {
                Answer::Status(status)
            } else {
                Answer::Answers(status)
            }
        }
        Err(e) if e.is_timeout() => Answer::TimedOut,
        Err(e) if e.is_connect() => Answer::ConnectFailed,
        Err(_) => Answer::Failed,
    }
}

/// Whether addresses `a` and `b` are on one server: the same scheme, host (in
/// any letter case) and port, the scheme's default port filled in (http 80,
/// https 443). The path, the query and the headers after a Kodi `|` do not
/// count. An address that cannot be read is on no server, so never on the
/// same one. The app asks no address on the server of the stream that plays:
/// a paid list's portal allows one session per account (ruling 28b).
pub fn same_server(a: &str, b: &str) -> bool {
    match (server_of(a), server_of(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The scheme, the host in lower case and the port of `url` (the scheme's
/// default when it names none).
fn server_of(url: &str) -> Option<(String, String, Option<u16>)> {
    let address = url.split('|').next().unwrap_or(url);
    let parsed = reqwest::Url::parse(address).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let port = parsed.port_or_known_default();
    Some((parsed.scheme().to_owned(), host, port))
}
```

The response is dropped unread: the connection closes, nothing of a live stream is downloaded. `reqwest::Url` is the `url` crate's `Url`, which reqwest hands on: no new dependency.

Run `rustfmt --edition 2021 src/stream_check.rs` once written.

In `rust/src/lib.rs`, add `pub mod stream_check;` after `pub mod storage;`.

- [ ] **Step 4: Run the tests to see them pass**

Run: `ct --test test_stream_check`
Expected: all 20 pass. `a_closed_port_is_a_failed_connection` takes about 2 s on Windows (it retries a refused connection twice).

Run once: `ct --test test_mpv_smoke -- --ignored the_defect_check_sends_mpvs_own_user_agent`
Expected: it passes. When it fails, set `MPV_USER_AGENT` to what it says mpv sends and run it again; say so in your report. When libmpv cannot be loaded there, say that instead and leave the constant.

- [ ] **Step 5: Format, lint, full suite**

- Run `fmt src/stream_check.rs tests/test_stream_check.rs tests/test_mpv_smoke.rs`. Expected: no output.
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes.

- [ ] **Step 6: Commit**

Message: `feat(rust): the check before a defect mark: one GET of the suspect's address with its own headers, four at a time`, followed by the two attribution lines.
Files: `rust/src/stream_check.rs`, `rust/src/lib.rs`, `rust/tests/test_stream_check.rs`, `rust/tests/test_mpv_smoke.rs`.

---

### Task 5: App: defect marks

The app keeps the M3U source's marks (`BadStreams`), read when its catalog loads, and feeds them at the two moments that count: a start that ends without a first picture, and a first picture. A suspect that a first picture confirms is not marked at once: its address is checked first (Task 4), and the answer, taken in once per frame, decides; one on the server of what plays is never asked, nor marked (ruling 28b). Every change goes to the catalog's filter at once. The search and the favorites list name a marked channel. An Xtream portal never touches any of it.

**Files:**
- Modify: `rust/src/app.rs`, `rust/src/i18n.rs`

**Interfaces:**
- Consumes:
  - Task 1: `BadStreams::{load, marks, failed, first_picture, mark, expire}`, `Failure`, `Picture { confirmed, cleared }`, `Confirmed { stream_id, name, failed_unix }`, `Storage::bad_streams_path`;
  - Task 2: `CatalogStore::{set_defect, is_defect, live_name}`;
  - Task 3: `TvApp::plays`, the tests' `RING`, `m3u_sid`, `ring_url`;
  - Task 4: `StreamChecker::{new, start, drain, forget, reset, checking, in_flight}`, `Check`, `Checked`, `Answer::{is_dead, kind, Unchecked}`, `same_server`;
  - `current_has_played: bool` (app.rs, from the base): the stream that plays has shown a picture since it was picked; `PlaybackStarted` sets it, a load that is no reload clears it;
  - existing in `app.rs`: `drain_events` (the `PlaybackStarted`, `EndOfFile` and `Error` arms), `stop_for_refused_load` (renamed here to `stop_failed_start`), `check_stall`, `load_url_with`, `stop_playback`, `install_portal`, `paint_search`, `paint_favs`, `handle_keys` (F5), the field `current_url`; the tests' `Harness`, `m3u_app`, `loads`, `live_url`, `loaded_catalog`, `two_sources`, `write_last_watched`, `wait_until`, `list_server`, `f_pressed`, `shift_f_pressed`, `LogCapture`, `ENV_LOCK`.
- Produces (`app.rs`):
  ```rust
  // TvApp fields (`current_has_played`, the base's, gains a doc and a clear in stop_playback)
  bad_streams: Option<BadStreams>,
  bad_streams_for: Option<(u64, u64)>,     // (catalog generation, catalog revision)
  stream_checks: StreamChecker,            // the checks before a mark
  // TvApp methods
  fn refresh_defects(&mut self);           // per frame in update_ui
  fn expire_defects(&mut self);            // also on F5
  fn note_start_failure(&mut self);
  fn note_first_picture(&mut self);        // confirmed suspects go to the check, none on the server of what plays
  fn drain_stream_checks(&mut self);       // per frame in update_ui: the checks' verdicts
  fn stop_failed_start(&mut self) -> bool; // was stop_for_refused_load; the watchdog uses it too
  // tests: fn start_fails(h), fn picture_comes(h), fn now_unix() -> i64,
  //        fn write_marks(root, &[(i64, i64)]), fn marks_in(root) -> HashMap<i64, i64>,
  //        fn ring_with_b_at(server: &wiremock::MockServer) -> String,
  //        async fn checks_answered(h: &mut Harness)
  ```
  i18n key: `DefectLabel`. Log lines: Ruling 9; `m3u: <name> (stream_id <id>) not checked (<what>), not marked` (rulings 28b, 29); and `m3u: no picture ~<n>s after the start - given up`.

- [ ] **Step 1: Write the failing tests**

In `rust/src/app.rs`'s `mod tests`, after the section `// ---- zapping through the ring ----` (Task 3), add:

```rust
    // ---- defect marks ----

    /// The start of what plays ends before its first picture: mpv closes the
    /// file with an error, as for a server that answers 404.
    fn start_fails(h: &mut Harness) {
        h.events
            .send(Event::EndOfFile {
                reason: "error".into(),
            })
            .unwrap();
        h.frame(&[]);
    }

    /// What plays shows its first picture (mpv's playback-restart).
    fn picture_comes(h: &mut Harness) {
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
    }

    fn now_unix() -> i64 {
        chrono::Utc::now().timestamp()
    }

    /// Source 1's marks file in `root`, holding these marks (stream id, unix
    /// seconds).
    fn write_marks(root: &std::path::Path, marks: &[(i64, i64)]) {
        let body: Vec<String> = marks
            .iter()
            .map(|(id, at)| format!("\"{id}\":{at}"))
            .collect();
        let text = format!("{{\"version\":1,\"marks\":{{{}}}}}", body.join(","));
        std::fs::write(root.join("bad_streams.json"), text).unwrap();
    }

    /// The marks in source 1's file in `root`.
    fn marks_in(root: &std::path::Path) -> std::collections::HashMap<i64, i64> {
        BadStreams::load(root.join("bad_streams.json"))
            .marks()
            .clone()
    }

    /// `RING` with Kanaal B at `server` (a `list_server`, whose every answer to
    /// a GET is what a channel's address answers the check before a mark), so
    /// the check asks it and no other machine.
    fn ring_with_b_at(server: &wiremock::MockServer) -> String {
        RING.replace(
            "https://b.example/b.m3u8",
            &format!("{}/b.m3u8", server.uri()),
        )
    }

    /// Frames until the checks before a mark have answered and the answers were
    /// taken in (2 s at most).
    async fn checks_answered(h: &mut Harness) {
        for _ in 0..400 {
            h.frame(&[]);
            if h.app.stream_checks.in_flight() == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the checks did not answer");
    }

    #[tokio::test]
    async fn a_dead_channel_followed_by_one_that_plays_is_marked_once_its_address_fails_too() {
        let server = list_server(404, "", Duration::from_millis(200)).await;
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), &ring_with_b_at(&server));
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        let log = LogCapture::start();
        let b = m3u_sid("Kanaal B");
        h.app.zap_by_id(b);
        start_fails(&mut h);
        assert!(!h.app.catalog.is_defect(b), "a suspect, not marked yet");
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        picture_comes(&mut h);
        // Confirmed: its address is asked first; the mark waits for the answer.
        assert!(h.app.stream_checks.checking(b));
        assert!(!h.app.catalog.is_defect(b), "not before the answer is taken in");
        checks_answered(&mut h).await;
        assert!(h.app.catalog.is_defect(b));
        assert!(marks_in(dir.path()).contains_key(&b), "written");
        let text = log.text();
        for said in ["failed before its first picture", "marked defect (http 404)"] {
            let line = format!("m3u: Kanaal B (stream_id {b}) {said}");
            assert!(text.contains(&line), "{line}\n{text}");
        }
        assert!(!text.contains(&server.uri()), "no address in the log:\n{text}");
        h.sent();
        h.press(&[Key::ArrowUp]);
        assert_eq!(loads(h.sent()), [ring_url('a')], "zapping skips B");
    }

    #[tokio::test]
    async fn a_dead_channel_whose_address_answers_again_is_not_marked() {
        let server = list_server(200, "", Duration::ZERO).await;
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), &ring_with_b_at(&server));
        let log = LogCapture::start();
        let b = m3u_sid("Kanaal B");
        h.app.zap_by_id(b);
        start_fails(&mut h);
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        picture_comes(&mut h);
        checks_answered(&mut h).await;
        assert!(!h.app.catalog.is_defect(b), "a Wi-Fi drop, not a dead channel");
        assert!(!dir.path().join("bad_streams.json").exists());
        let text = log.text();
        let line = format!("m3u: Kanaal B (stream_id {b}) answers again, not marked");
        assert!(text.contains(&line), "{text}");
        assert!(!text.contains(&server.uri()), "no address in the log:\n{text}");
    }

    #[tokio::test]
    async fn a_suspect_on_the_server_of_what_plays_is_not_asked_and_one_on_another_port_is() {
        // B and C on one server, D on another port of that host. Both would
        // answer 404, so a check of B would mark it.
        let here = list_server(404, "", Duration::ZERO).await;
        let other_port = list_server(404, "", Duration::ZERO).await;
        let at = |server: &wiremock::MockServer, file: &str| format!("{}/{file}", server.uri());
        let list = RING
            .replace("https://b.example/b.m3u8", &at(&here, "b.m3u8"))
            .replace("https://c.example/c.m3u8", &at(&here, "c.m3u8"))
            .replace("https://d.example/d.m3u8", &at(&other_port, "d.m3u8"));
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), &list);
        let log = LogCapture::start();
        let (b, d) = (m3u_sid("Kanaal B"), m3u_sid("Kanaal D"));
        h.app.zap_by_id(b);
        start_fails(&mut h);
        h.app.zap_by_id(d);
        start_fails(&mut h);
        // C plays: its picture confirms B and D.
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        picture_comes(&mut h);
        assert!(!h.app.stream_checks.checking(b), "C's server");
        assert!(h.app.stream_checks.checking(d), "another port");
        let suspects = h.app.bad_streams.as_ref().map(|bad| bad.suspects());
        assert_eq!(suspects, Some(0), "B is no suspect any more");
        checks_answered(&mut h).await;
        assert!(!h.app.catalog.is_defect(b), "not marked");
        assert!(h.app.catalog.is_defect(d), "asked, and dead");
        let asked = here.received_requests().await.unwrap();
        assert!(asked.is_empty(), "nothing asked C's server");
        let text = log.text();
        let said = "not checked (the server of what plays), not marked";
        let line = format!("m3u: Kanaal B (stream_id {b}) {said}");
        assert!(text.contains(&line), "{text}");
        assert!(!text.contains(&here.uri()), "no address:\n{text}");
    }

    #[tokio::test]
    async fn a_switch_of_source_during_a_check_leaves_no_mark() {
        let server = list_server(404, "", Duration::from_millis(400)).await;
        let dir = tempfile::tempdir().unwrap();
        let list = ring_with_b_at(&server);
        let (twee, drie) = (dir.path().join("twee.m3u"), dir.path().join("drie.m3u"));
        std::fs::write(&twee, &list).unwrap();
        std::fs::write(&drie, &list).unwrap();
        let sources = Sources::parse(&format!(
            "source=2;m3u;Twee;{}\nsource=3;m3u;Drie;{}\nactive=2\n",
            twee.display(),
            drie.display()
        ));
        let portal = M3uPortal::new(2, &twee.display().to_string(), dir.path().join("m3u-2.m3u"));
        let store = Arc::new(CatalogStore::new(Arc::new(portal)));
        let mut h = Harness::start_with(
            dir.path(),
            store.clone(),
            PortalState::Configured,
            None,
            None,
            sources,
            None,
        );
        // Source 2's files, as main starts the app on it. Set before its list
        // loads, which is when its marks are read (as `downloading` does).
        h.app.storage.set_source(2);
        store.spawn_fetch();
        wait_until("list 2", || store.is_loaded()).await;
        h.frame(&[]);
        let b = m3u_sid("Kanaal B");
        h.app.zap_by_id(b);
        start_fails(&mut h);
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        picture_comes(&mut h);
        assert!(h.app.stream_checks.checking(b), "B's address is being asked");
        {
            // The variable is the whole process's: held for the switch only.
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            h.app.activate_source(3);
            std::env::remove_var("BONGO_INI");
        }
        assert!(!h.app.stream_checks.checking(b), "dropped with the old source");
        wait_until("list 3", || h.app.catalog.is_loaded()).await;
        // Past the old check's answer: source 3's marks are read and stay clean.
        tokio::time::sleep(Duration::from_millis(600)).await;
        for _ in 0..5 {
            h.frame(&[]);
        }
        assert!(!h.app.catalog.is_defect(b), "no mark from the old source's check");
        assert!(!dir.path().join("bad_streams-3.json").exists());
        assert!(!dir.path().join("bad_streams.json").exists());
        assert!(!dir.path().join("bad_streams-2.json").exists());
    }

    #[tokio::test]
    async fn a_dead_m3u_channel_with_no_picture_after_it_is_not_marked() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), RING);
        // B fails, then C fails too: the connection may be gone.
        h.app.zap_by_id(m3u_sid("Kanaal B"));
        start_fails(&mut h);
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        start_fails(&mut h);
        assert!(!h.app.catalog.is_defect(m3u_sid("Kanaal B")));
        assert!(!h.app.catalog.is_defect(m3u_sid("Kanaal C")));
        let suspects = h.app.bad_streams.as_ref().map(|bad| bad.suspects());
        assert_eq!(suspects, Some(2), "two suspects, neither confirmed");
        assert_eq!(h.app.stream_checks.in_flight(), 0, "nothing asked");
        assert!(!dir.path().join("bad_streams.json").exists());
    }

    #[tokio::test]
    async fn a_channel_that_showed_a_picture_and_then_failed_is_no_suspect() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), RING);
        let b = m3u_sid("Kanaal B");
        h.app.zap_by_id(b);
        picture_comes(&mut h);
        // It drops: an eof reloads it, and the reload fails before a picture.
        h.events
            .send(Event::EndOfFile {
                reason: "eof".into(),
            })
            .unwrap();
        h.frame(&[]);
        start_fails(&mut h);
        let suspects = |h: &Harness| h.app.bad_streams.as_ref().map(|bad| bad.suspects());
        assert_eq!(suspects(&h), Some(0), "a stream that dropped: no suspect");
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        picture_comes(&mut h);
        assert!(!h.app.stream_checks.checking(b), "nothing confirmed, nothing asked");
        assert!(!h.app.catalog.is_defect(b));
    }

    #[tokio::test]
    async fn the_reload_budget_spent_on_a_start_without_a_picture_makes_a_suspect() {
        let server = list_server(404, "", Duration::ZERO).await;
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), &ring_with_b_at(&server));
        let b = m3u_sid("Kanaal B");
        h.app.zap_by_id(b);
        h.sent();
        for _ in 0..4 {
            h.events
                .send(Event::EndOfFile {
                    reason: "eof".into(),
                })
                .unwrap();
            h.frame(&[]);
        }
        assert_eq!(
            loads(h.sent()).len(),
            3,
            "three reloads, then the budget is spent"
        );
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        picture_comes(&mut h);
        checks_answered(&mut h).await;
        assert!(h.app.catalog.is_defect(b));
    }

    #[tokio::test]
    async fn a_refused_load_makes_a_suspect() {
        let server = list_server(404, "", Duration::ZERO).await;
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), &ring_with_b_at(&server));
        let b = m3u_sid("Kanaal B");
        h.app.zap_by_id(b);
        let refused = Event::Error {
            msg: "mpv error -4: an argument holds a NUL byte".into(),
        };
        h.events.send(refused).unwrap();
        h.frame(&[]);
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        picture_comes(&mut h);
        checks_answered(&mut h).await;
        assert!(h.app.catalog.is_defect(b));
    }

    #[tokio::test]
    async fn the_stall_watchdog_gives_up_an_m3u_start_without_a_picture() {
        let server = list_server(404, "", Duration::ZERO).await;
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), &ring_with_b_at(&server));
        let b = m3u_sid("Kanaal B");
        h.app.zap_by_id(b);
        // A frame for the zap's toast: egui keeps a new area (the toast's)
        // invisible in its first frame, and the error toast below must show.
        h.frame(&[]);
        h.sent();
        // No picture for longer than the watchdog gives a start.
        h.app.last_progress_at = Instant::now()
            .checked_sub(Duration::from_secs(26))
            .expect("the machine has run for 26 s");
        h.frame(&[]);
        let sent = h.sent();
        assert!(sent.iter().any(|c| matches!(c, Cmd::Stop)), "{sent:?}");
        assert!(loads(sent).is_empty(), "no reload, no herverbinden");
        let named = "stream error on Kanaal B - zap up or down to retry";
        assert!(h.painted.iter().any(|t| t == named), "{:?}", h.painted);
        assert!(!h.app.stall_watchdog_armed);
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        picture_comes(&mut h);
        checks_answered(&mut h).await;
        assert!(h.app.catalog.is_defect(b));
    }

    #[tokio::test]
    async fn an_xtream_portal_stalled_before_its_first_picture_still_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        h.app.zap_by_id(3);
        h.sent();
        h.app.last_progress_at = Instant::now()
            .checked_sub(Duration::from_secs(26))
            .expect("the machine has run for 26 s");
        h.frame(&[]);
        assert_eq!(loads(h.sent()), [live_url(3)], "reloaded as before");
    }

    #[tokio::test]
    async fn a_marked_channel_found_by_the_search_is_named_so_and_loses_its_mark_once_it_plays() {
        let dir = tempfile::tempdir().unwrap();
        let b = m3u_sid("Kanaal B");
        write_marks(dir.path(), &[(b, now_unix() - 60)]);
        let mut h = m3u_app(dir.path(), RING);
        assert!(h.app.catalog.is_defect(b), "read at the start");
        let log = LogCapture::start();
        h.frame_with(f_pressed());
        h.frame(&[]);
        h.type_text("kanaal b");
        h.frame(&[]);
        assert!(
            h.painted.iter().any(|t| t == "[LIVE] Kanaal B · broken"),
            "{:?}",
            h.painted
        );
        h.sent();
        h.press(&[Key::Enter]);
        assert_eq!(loads(h.sent()), [ring_url('b')]);
        picture_comes(&mut h);
        assert!(!h.app.catalog.is_defect(b));
        assert!(marks_in(dir.path()).is_empty(), "written");
        let line = format!("m3u: Kanaal B (stream_id {b}) plays again, mark cleared");
        assert!(log.text().contains(&line), "{}", log.text());
    }

    #[tokio::test]
    async fn the_favorites_list_names_a_defect_channel() {
        let dir = tempfile::tempdir().unwrap();
        let b = m3u_sid("Kanaal B");
        write_marks(dir.path(), &[(b, now_unix() - 60)]);
        let mut favorites = Favorites::default();
        favorites.toggle(b, "Kanaal B");
        favorites.save(&dir.path().join("favorites.json")).unwrap();
        let mut h = m3u_app(dir.path(), RING);
        h.frame_holding(egui::Modifiers::SHIFT, shift_f_pressed());
        h.frame(&[]);
        assert!(h.painted.iter().any(|t| t == "Kanaal B"), "{:?}", h.painted);
        assert!(h.painted.iter().any(|t| t == "broken"), "{:?}", h.painted);
    }

    #[tokio::test]
    async fn a_preset_still_plays_a_defect_channel() {
        let dir = tempfile::tempdir().unwrap();
        let b = m3u_sid("Kanaal B");
        write_marks(dir.path(), &[(b, now_unix() - 60)]);
        let mut h = m3u_app(dir.path(), RING);
        h.app.presets.set(1, b, "Kanaal B");
        h.sent();
        h.app.recall_preset(1);
        assert_eq!(loads(h.sent()), [ring_url('b')]);
    }

    #[tokio::test]
    async fn a_resumed_defect_channel_plays_and_its_picture_clears_the_mark() {
        let dir = tempfile::tempdir().unwrap();
        let b = m3u_sid("Kanaal B");
        write_marks(dir.path(), &[(b, now_unix() - 60)]);
        write_last_watched(dir.path(), &ring_url('b'), true, Some(false));
        let mut h = m3u_app(dir.path(), RING);
        assert_eq!(loads(h.sent()), [ring_url('b')], "the resume plays it");
        assert!(h.app.catalog.is_defect(b));
        picture_comes(&mut h);
        assert!(!h.app.catalog.is_defect(b));
    }

    #[tokio::test]
    async fn a_resumed_channel_that_showed_its_picture_before_the_list_loaded_loses_its_mark() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, RING).unwrap();
        let b = m3u_sid("Kanaal B");
        write_marks(dir.path(), &[(b, now_unix() - 60)]);
        write_last_watched(dir.path(), &ring_url('b'), true, Some(false));
        let portal = M3uPortal::new(1, &file.display().to_string(), dir.path().join("m3u-1.m3u"));
        let store = Arc::new(CatalogStore::new(Arc::new(portal)));
        let mut h = Harness::start_with(
            dir.path(),
            store.clone(),
            PortalState::Configured,
            None,
            None,
            Sources::default(),
            None,
        );
        assert_eq!(loads(h.sent()), [ring_url('b')], "resumed before the list is read");
        // Its picture comes while nothing knows its stream id or the marks.
        picture_comes(&mut h);
        store.spawn_fetch();
        wait_until("the list", || store.is_loaded()).await;
        h.frame(&[]);
        assert!(!h.app.catalog.is_defect(b), "it plays: the mark goes");
        assert!(marks_in(dir.path()).is_empty(), "written");
    }

    #[tokio::test]
    async fn the_marks_survive_a_restart_and_old_ones_expire() {
        let server = list_server(404, "", Duration::ZERO).await;
        let dir = tempfile::tempdir().unwrap();
        let b = m3u_sid("Kanaal B");
        {
            let mut h = m3u_app(dir.path(), &ring_with_b_at(&server));
            h.app.zap_by_id(b);
            start_fails(&mut h);
            h.app.zap_by_id(m3u_sid("Kanaal C"));
            picture_comes(&mut h);
            checks_answered(&mut h).await;
            assert!(h.app.catalog.is_defect(b));
        }
        // The next start: B is still marked. A mark of C from eight days ago is
        // put in the file; it goes when the list loads.
        let c = m3u_sid("Kanaal C");
        let mut marks = marks_in(dir.path());
        marks.insert(c, now_unix() - 8 * 24 * 3600);
        let marks: Vec<(i64, i64)> = marks.into_iter().collect();
        write_marks(dir.path(), &marks);
        let log = LogCapture::start();
        let h = m3u_app(dir.path(), RING);
        assert!(h.app.catalog.is_defect(b), "kept");
        assert!(!h.app.catalog.is_defect(c), "expired");
        assert!(
            log.text().contains("m3u: 1 defect marks expired"),
            "{}",
            log.text()
        );
        let kept: Vec<i64> = marks_in(dir.path()).keys().copied().collect();
        assert_eq!(kept, [b], "written");
    }

    #[tokio::test]
    async fn an_xtream_portal_reads_and_marks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write_marks(dir.path(), &[(3, now_unix() - 60)]);
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        assert!(!h.app.catalog.is_defect(3), "no marks read");
        h.app.zap_by_id(3);
        start_fails(&mut h);
        picture_comes(&mut h);
        assert!(h.app.bad_streams.is_none());
        assert_eq!(h.app.stream_checks.in_flight(), 0, "and checks nothing");
        assert_eq!(marks_in(dir.path()).len(), 1, "the file is left alone");
    }

    #[tokio::test]
    async fn switching_to_a_list_reads_that_sources_own_marks() {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, _) = two_sources(dir.path());
        let een = crate::m3u::stream_id("Een.nl");
        let record = format!(
            "{{\"version\":1,\"marks\":{{\"{een}\":{}}}}}",
            now_unix() - 60
        );
        std::fs::write(dir.path().join("bad_streams-2.json"), record).unwrap();
        {
            // The variable is the whole process's: held for the switch only.
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            h.app.activate_source(2);
            std::env::remove_var("BONGO_INI");
        }
        wait_until("the list", || h.app.catalog.is_loaded()).await;
        h.frame(&[]);
        assert!(h.app.catalog.is_defect(een), "bad_streams-2.json");
    }
```

In the existing test `deleting_a_source_takes_its_files_and_never_the_source_in_use`, add `two.bad_streams_path(),` to the paths it writes, and add `&& !two.bad_streams_path().exists()` to its `assert!(!two.favorites_path().exists() && …)`.

- [ ] **Step 2: Run them to see them fail**

Run: `ct --lib app::tests::`
Expected: a compile error: `TvApp` has no field `bad_streams` (`an_xtream_portal_reads_and_marks_nothing` reads it).

- [ ] **Step 3: The text**

In `rust/src/i18n.rs`, under `// Sources: an M3U list`, add:

```rust
    DefectLabel => ["defect", "broken", "defekt"],
```

- [ ] **Step 4: The marks in the app**

In `rust/src/app.rs`:

1. Imports: add `use crate::bad_streams::{BadStreams, Failure};` and `use crate::stream_check::{same_server, Answer, Check, StreamChecker};` inside the import block.

2. The marks read the base's field `current_has_played` (after `stream_starting_since`): the stream that plays has shown a picture since it was picked. It keeps its name; its doc gains what the marks read in it, so the whole doc becomes:

```rust
    /// The stream that plays has shown a picture since it was picked: its
    /// first `PlaybackStarted` sets this, and any load but a reload of it
    /// clears it, as `stop_playback` does. A reload sets `stream_starting_since`
    /// again, but the picture on the screen is still its own. The defect marks
    /// read it too (spec 2026-10-15 §4.1): a start of a stream that has played
    /// and then fails is a stream that dropped, never a suspect
    /// (`note_start_failure`), and the stall watchdog gives up only a start
    /// that has not played (`check_stall`).
    current_has_played: bool,
```

Fields, after `current_has_played`:

```rust
    /// The defect marks of the M3U source in use, read when its catalog loads;
    /// none on an Xtream portal. The catalog holds a copy for its filter
    /// (`CatalogStore::set_defect`): every change here is handed to it.
    bad_streams: Option<BadStreams>,
    /// The catalog (generation, revision) the marks were brought up to date for.
    bad_streams_for: Option<(u64, u64)>,
    /// The checks before a mark (spec §4.2): a confirmed suspect's address is
    /// asked once more in the background; `drain_stream_checks` takes the
    /// answers in. Builds nothing until the first check.
    stream_checks: StreamChecker,
```

and in `TvApp::start`'s struct literal, after `current_has_played: false,`: `bad_streams: None, bad_streams_for: None, stream_checks: StreamChecker::new(),` (one per line).

3. `load_url_with` stays as it is: its `if !reload { … }` block clears `current_has_played` already.

4. In `stop_playback`, after `self.stream_starting_since = None;`, add `self.current_has_played = false;`. The base has no such line; the frame drop of the `EndOfFile` arm reads the flag only while `stream_starting_since` is set, which `stop_playback` clears, so nothing changes for it.

5. In `drain_events`, in the `Event::PlaybackStarted` arm, after the base's `self.current_has_played = true;`, add:

```rust
                    // What plays has shown a picture since it was picked, and the
                    // connection works: the marks hear of it (spec §4.2, §4.4).
                    self.note_first_picture();
```

6. In `drain_events`, in the `Event::EndOfFile` arm, at the end of the branch `if reason == "error" || (reason == "eof" && live) { … }` (after the M3U block that takes the old picture off the screen, which reads `stream_starting_since` and `current_has_played`; `note_start_failure` takes `stream_starting_since`, so it comes after), add:

```rust
                        // A start that ended without a picture: a suspect (spec §4.1).
                        self.note_start_failure();
```

7. In the `Event::Error` arm, `self.stop_for_refused_load()` becomes `self.stop_failed_start()`.

8. The doc of the field `stream_starting_since`, which names `stop_for_refused_load`, says which paths read which flag. It becomes:

```rust
    /// When the stream that loads now went to the player; its first frame
    /// clears it. EPG fetching waits meanwhile (10 s at most): zapping first.
    /// An M3U channel that fails while it is set has shown no picture of its
    /// own unless it played since it was picked (`current_has_played`; a reload
    /// sets this one again). The `Event::EndOfFile` arm of `drain_events` and
    /// `note_start_failure` read both. `stop_failed_start` reads this one only
    /// (a refused load, played or not), and `check_stall` calls it only for a
    /// start that has not played.
    stream_starting_since: Option<Instant>,
```

Replace `fn stop_for_refused_load` with:

```rust
    /// The start of an M3U channel that cannot go on: mpv refused the load (a
    /// list's address or header holds what it cannot take, a NUL byte), or the
    /// stall watchdog gave it up. What mpv still plays, the stream before it or
    /// the load that hangs, stops, and its picture goes, as a dead channel's
    /// does; the toast names the channel and the app stays on it. It asks only
    /// whether the stream is still starting (`stream_starting_since`), never
    /// whether it has played (`current_has_played`): the `Event::Error` arm
    /// calls it for a refused load either way, and `check_stall` only for a
    /// start that has not played. `note_start_failure` then makes the channel a
    /// suspect unless it has played. True when it was dealt with so. (M3U only:
    /// an Xtream portal's loads go through the relay, with nothing in them mpv
    /// can refuse, and its watchdog reloads.)
    fn stop_failed_start(&mut self) -> bool {
        if self.catalog.kind() != SourceKind::M3u || self.stream_starting_since.is_none() {
            return false;
        }
        let Some(name) = self.current_name.clone() else {
            return false;
        };
        let _ = self.player.cmd_tx.send(Cmd::Stop);
        // Its picture goes too, and none of its frames comes back: the empty
        // state, never the channel before under this one's name. The fence stays
        // shut until the next load narrows it (as after `stop_playback`), so a
        // frame mpv renders before the stop lands is kept away as well.
        self.frame_fence.stop();
        self.video_tex = None;
        // Nothing plays to watch, and a reload would be refused again.
        self.stall_watchdog_armed = false;
        // A suspect; the start is over (this clears `stream_starting_since`).
        self.note_start_failure();
        self.stream_starting_since = None;
        self.set_toast(self.tr(T::StreamErrorOn, &[&name]));
        true
    }

    /// The marks of the M3U source in use, up to date with its catalog (spec
    /// §4.4, §4.5): read from the source's file when its catalog first loads in a
    /// catalog generation (a start, a switch to it, its address saved again); on
    /// every new copy of the list, marks older than seven days go. Three
    /// comparisons per frame otherwise, and nothing at all on an Xtream portal.
    fn refresh_defects(&mut self) {
        if self.catalog.kind() != SourceKind::M3u {
            return;
        }
        let key = (self.catalog_generation, self.catalog.revision());
        if self.bad_streams_for == Some(key) || !self.catalog.is_loaded() {
            return;
        }
        let read_for = self.bad_streams_for.map(|(generation, _)| generation);
        if self.bad_streams.is_none() || read_for != Some(self.catalog_generation) {
            self.bad_streams = Some(BadStreams::load(self.storage.bad_streams_path()));
            // What plays showed its picture before the marks were read (a resume
            // on a list that loaded late): it plays again, whichever way it came.
            if self.current_has_played {
                self.note_first_picture();
            }
        }
        self.bad_streams_for = Some(key);
        self.expire_defects();
    }

    /// Marks older than seven days go (spec §4.4), and the catalog gets the
    /// marks that are left. Runs when a copy of the list loads, and on F5.
    fn expire_defects(&mut self) {
        let Some(bad) = self.bad_streams.as_mut() else {
            return;
        };
        let gone = bad.expire(chrono::Utc::now().timestamp());
        if gone > 0 {
            tracing::info!("m3u: {} defect marks expired", gone);
        }
        self.catalog.set_defect(bad.marks().clone());
    }

    /// The start of the M3U channel that plays ended without a first picture
    /// (spec §4.1): mpv ended the file with an error or refused the load, or the
    /// app gave the start up (the reload budget, the stall watchdog). It becomes
    /// a suspect, or a marked one is marked anew. Not a channel that has played
    /// since it was picked (`current_has_played`): its reloads are a stream that
    /// dropped. Once per start: the start is over, so `stream_starting_since`
    /// goes.
    fn note_start_failure(&mut self) {
        if self.catalog.kind() != SourceKind::M3u || self.stream_starting_since.take().is_none() {
            return;
        }
        if self.current_has_played || !self.current_live {
            return;
        }
        let Some(sid) = self.current_stream_id else {
            return;
        };
        // A resumed channel has no name of its own yet: the list's.
        let Some(name) = self
            .current_name
            .clone()
            .or_else(|| self.catalog.live_name(sid))
        else {
            return;
        };
        let Some(bad) = self.bad_streams.as_mut() else {
            return;
        };
        let shown = crate::redact::redact(&name);
        tracing::info!("m3u: {} (stream_id {}) failed before its first picture", shown, sid);
        let failure = bad.failed(sid, &name, Instant::now(), chrono::Utc::now().timestamp());
        if failure == Failure::Renewed {
            // Marked already: no check, a new seven days (spec §4.4).
            tracing::info!("m3u: {} (stream_id {}) marked defect (failed again)", shown, sid);
            self.catalog.set_defect(bad.marks().clone());
        }
    }

    /// A first picture (spec §4.2, §4.4): the suspects that failed within two
    /// minutes before it are confirmed (the connection works), and their
    /// addresses are asked once more before any of them is marked
    /// (`drain_stream_checks` takes the answers in). One on the server of what
    /// plays is not asked (ruling 28b), so not marked. What plays loses its own
    /// suspicion, its mark and its check.
    fn note_first_picture(&mut self) {
        let Some(bad) = self.bad_streams.as_mut() else {
            return;
        };
        let sid = self.current_stream_id;
        let seen = bad.first_picture(sid, Instant::now());
        if seen.cleared {
            let name = self.current_name.as_deref().unwrap_or("?");
            tracing::info!(
                "m3u: {} (stream_id {}) plays again, mark cleared",
                crate::redact::redact(name),
                sid.unwrap_or_default()
            );
            self.catalog.set_defect(bad.marks().clone());
        }
        // It shows a picture: a check of its own address means nothing now.
        if let Some(sid) = sid {
            self.stream_checks.forget(sid);
        }
        for confirmed in seen.confirmed {
            let url = self.catalog.portal().live_stream_url(confirmed.stream_id);
            // Never the server of what plays (ruling 28b): on a paid list one
            // account serves every channel, and its portal allows one session.
            // The check would take the session of what plays, or be refused and
            // mark a channel that works. No check, no mark: the suspicion goes.
            if same_server(self.current_url.as_deref().unwrap_or(""), &url) {
                tracing::info!(
                    "m3u: {} (stream_id {}) not checked (the server of what plays), not marked",
                    crate::redact::redact(&confirmed.name),
                    confirmed.stream_id
                );
                continue;
            }
            self.stream_checks.start(Check {
                stream_id: confirmed.stream_id,
                name: confirmed.name,
                failed_unix: confirmed.failed_unix,
                url,
            });
        }
    }

    /// The verdicts of the checks before a mark (spec §4.2): an address that
    /// answers with a status of 400 or more, does not connect or does not
    /// answer in time gets the mark, with the time of its failure; one that
    /// answers again, or that could not be asked, is no suspect any more. The
    /// log names the channel and what its address did, never the address.
    /// Nothing while no check runs.
    fn drain_stream_checks(&mut self) {
        if self.stream_checks.in_flight() == 0 {
            return;
        }
        for checked in self.stream_checks.drain() {
            let shown = crate::redact::redact(&checked.name).into_owned();
            let sid = checked.stream_id;
            if checked.answer == Answer::Unchecked {
                // No check, no mark (spec §4.2, ruling 29).
                tracing::info!(
                    "m3u: {} (stream_id {}) not checked ({}), not marked",
                    shown,
                    sid,
                    checked.answer.kind()
                );
                continue;
            }
            if !checked.answer.is_dead() {
                tracing::info!("m3u: {} (stream_id {}) answers again, not marked", shown, sid);
                continue;
            }
            let Some(bad) = self.bad_streams.as_mut() else {
                continue;
            };
            bad.mark(sid, checked.failed_unix);
            let kind = checked.answer.kind();
            tracing::info!("m3u: {} (stream_id {}) marked defect ({})", shown, sid, kind);
            self.catalog.set_defect(bad.marks().clone());
        }
    }
```

9. In `check_stall`, after `if self.last_progress_at.elapsed() < self.health.stall_limit() { return; }`, add:

```rust
        // An M3U channel that has not played since it was picked is not
        // reloaded: the app gives its start up (spec 2026-10-15 §4.1), as when
        // mpv refuses a load, and it becomes a suspect. A reload would only start
        // the wait over, with a "herverbinden" toast each time. This is where
        // `current_has_played` is asked: `stop_failed_start` itself does not.
        let waited = self.last_progress_at.elapsed().as_secs();
        if !self.current_has_played && self.stop_failed_start() {
            tracing::warn!("m3u: no picture ~{}s after the start - given up", waited);
            return;
        }
```

10. In `update_ui`, after the block `if !self.catalog_noted && self.catalog.is_loaded() { … }`, add:

```rust
        // The M3U source's marks, once its catalog is there (after the catalog's
        // first word: a resumed channel knows its stream id by then), and the
        // verdicts of the checks before a mark.
        self.refresh_defects();
        self.drain_stream_checks();
```

11. In `handle_keys`, in the F5 branch `PortalState::Configured => { … }`, after `self.catalog.spawn_fetch();`, add:

```rust
                    // F5 lets old marks go too (spec §4.4), also when the list
                    // comes back the same and no new copy loads.
                    self.expire_defects();
```

12. In `install_portal`, after `self.adult_note = None;`, add:

```rust
        // The marks are the old source's: the new one's are read when its
        // catalog loads. Its checks go too: their answers are ignored (a delete
        // never touches the source in use, so only this drops them).
        self.bad_streams = None;
        self.bad_streams_for = None;
        self.stream_checks.reset();
```

13. In `paint_search`, in the loop over `self.search_hits`, after `let tag = match it.kind { … };`, add:

```rust
                            // A channel marked defect still shows, named so (spec §4.3).
                            let defect =
                                it.kind == ItemKind::Live && self.catalog.is_defect(it.id);
                            let text = if defect {
                                let label = crate::i18n::t(lang, T::DefectLabel);
                                format!("{} {} · {}", tag, it.name, label)
                            } else {
                                format!("{} {}", tag, it.name)
                            };
```

and pass `text` to `selectable_label` in place of `format!("{} {}", tag, it.name)`.

14. In `paint_favs`, after `let entries = shown_favorites(&self.favorites, &self.catalog);`, add:

```rust
        // Marked defect: still listed, named so (spec §4.3). No ✕: the row has
        // its x to remove the favorite already.
        let defect: Vec<bool> = entries
            .iter()
            .map(|e| self.catalog.is_defect(e.stream_id))
            .collect();
```

and make the loop:

```rust
                        for (entry, defect) in entries.into_iter().zip(defect) {
                            ui.horizontal(|ui| {
                                if ui.selectable_label(false, &entry.name).clicked() {
                                    zap_id = Some(entry.stream_id);
                                }
                                if defect {
                                    let label = crate::i18n::t(lang, T::DefectLabel);
                                    ui.label(
                                        egui::RichText::new(label)
                                            .small()
                                            .color(Color32::from_rgb(230, 120, 110)),
                                    );
                                }
                                if ui.small_button("x").clicked() {
                                    remove_id = Some(entry.stream_id);
                                }
                            });
                        }
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `ct --lib app::tests::`
Expected: all pass, the old M3U ones among them (`a_dead_m3u_channel_names_itself_and_the_app_stays_on_it`, `a_load_mpv_refuses_stops_the_stream_before_it_and_names_the_channel`, `a_dead_m3u_channel_takes_the_picture_of_the_channel_before_off_the_screen`, `an_m3u_channel_that_showed_its_own_picture_keeps_its_last_frame_when_it_errors`, `an_m3u_channel_that_keeps_ending_before_its_first_picture_takes_the_one_before_off` (the eof arm the hook of item 6 sits after), `a_refused_m3u_load_takes_the_picture_of_the_channel_before_off_the_screen` (the refused load, now `stop_failed_start`), `an_m3u_channel_that_has_played_keeps_its_own_last_frame_when_its_reloads_fail` and `an_m3u_pick_that_never_played_takes_off_the_picture_of_one_that_did` (both on `current_has_played`, which the marks now read and `stop_playback` clears too)).

- [ ] **Step 6: Format, lint, full suite**

- Run `fmt src/i18n.rs`. Expected: no output.
- Check the hunk count of `app.rs`: 7, as at the baseline (Task 1).
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes.

- [ ] **Step 7: Commit**

Message: `feat(rust): mark M3U channels that fail before their first picture, once another one plays and a check of their address fails too; zapping skips them, search and favorites name them`, followed by the two attribution lines.
Files: `rust/src/app.rs`, `rust/src/i18n.rs`.

---

### Task 6: The channel list's groups and selection

A pure module: the list's groups as positions in the catalog's channels, A–Z by natural order, and where the selection is. The catalog builds them with its own filters (the adult lock, the defect marks).

**Files:**
- Create: `rust/src/channel_list.rs`
- Modify: `rust/src/lib.rs` (`pub mod channel_list;` after `pub mod catalog;`)
- Modify: `rust/src/catalog.rs` (`CatalogStore::channel_groups`)
- Test: `rust/tests/test_channel_list.rs` (new)

**Interfaces:**
- Consumes (Task 2): `StoredCatalog.live: Arc<Vec<LiveChannel>>`, `CatalogStore::{defect_marks, adult_unlocked}`, `adult_live`.
- Produces:
  ```rust
  // channel_list.rs
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum GroupKey { All, Favorites, Named(String), NoGroup, Defect }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Group { pub key: GroupKey, pub members: Vec<u32> }
  pub struct ChannelGroups;
  impl ChannelGroups {
      pub fn build(live: Arc<Vec<LiveChannel>>, shows: impl Fn(&LiveChannel) -> bool,
          defect: &HashMap<i64, i64>, favorites: &HashSet<i64>) -> Self;
      pub fn groups(&self) -> &[Group];
      pub fn group_count(&self) -> usize;
      pub fn key(&self, group: usize) -> Option<&GroupKey>;
      pub fn rows(&self, group: usize) -> usize;
      pub fn channel(&self, group: usize, row: usize) -> Option<&LiveChannel>;
      pub fn row_of(&self, group: usize, stream_id: i64) -> Option<usize>;
      pub fn find(&self, key: &GroupKey) -> Option<usize>;
      pub fn channel_count(&self) -> usize;
      pub fn heap_bytes(&self) -> usize;
  }
  pub fn group_names(category: Option<&str>) -> impl Iterator<Item = &str>;
  pub fn natural_cmp(a: &str, b: &str) -> Ordering;
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct Cursor { pub group: usize, pub row: usize }
  impl Cursor {
      pub fn open(groups: &ChannelGroups, last: Option<&GroupKey>, playing: Option<i64>) -> Self;
      pub fn step(&mut self, groups: &ChannelGroups, delta: isize) -> bool;
      pub fn to_end(&mut self, groups: &ChannelGroups, last: bool) -> bool;
      pub fn step_group(&mut self, groups: &ChannelGroups, delta: isize, playing: Option<i64>) -> bool;
      pub fn select_group(&mut self, groups: &ChannelGroups, group: usize, playing: Option<i64>) -> bool;
      pub fn select_row(&mut self, groups: &ChannelGroups, row: usize) -> bool;
      pub fn follow(&mut self, old: &ChannelGroups, new: &ChannelGroups);
      pub fn channel<'a>(&self, groups: &'a ChannelGroups) -> Option<&'a LiveChannel>;
  }
  // catalog.rs
  impl CatalogStore { pub fn channel_groups(&self, favorites: &HashSet<i64>) -> Option<ChannelGroups>; }
  ```

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_channel_list.rs`:

```rust
//! The channel list's groups and selection (channel_list.rs): the order of the
//! groups and of their channels, the defect and favorite channels, the steps.

use bongo_player::catalog::CatalogStore;
use bongo_player::channel_list::{natural_cmp, ChannelGroups, Cursor, GroupKey};
use bongo_player::m3u::stream_id;
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::LiveChannel;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn ch(id: i64, name: &str, group: Option<&str>) -> LiveChannel {
    LiveChannel {
        stream_id: id,
        name: name.into(),
        category_id: group.map(Into::into),
        epg_channel_id: None,
        tv_archive: 0,
        tv_archive_duration: 0,
        is_adult: false,
        stream_icon: None,
    }
}

fn build(live: Vec<LiveChannel>, defect: &[i64], favorites: &[i64]) -> ChannelGroups {
    let defect: HashMap<i64, i64> = defect.iter().map(|&id| (id, 0)).collect();
    let favorites: HashSet<i64> = favorites.iter().copied().collect();
    ChannelGroups::build(Arc::new(live), |_| true, &defect, &favorites)
}

/// Each group as "<name> <count>"; the fixed ones by a word of their own.
fn summary(g: &ChannelGroups) -> Vec<String> {
    g.groups()
        .iter()
        .map(|group| {
            let name = match &group.key {
                GroupKey::All => "ALL",
                GroupKey::Favorites => "FAV",
                GroupKey::Named(name) => name.as_str(),
                GroupKey::NoGroup => "NONE",
                GroupKey::Defect => "DEFECT",
            };
            format!("{name} {}", group.members.len())
        })
        .collect()
}

fn names(g: &ChannelGroups, group: usize) -> Vec<String> {
    (0..g.rows(group))
        .map(|row| g.channel(group, row).unwrap().name.clone())
        .collect()
}

fn group_of(g: &ChannelGroups, name: &str) -> usize {
    g.find(&GroupKey::Named(name.into())).expect(name)
}

/// Sport and Nieuws, a channel in both, one in "nieuws", one without a group.
fn sample() -> Vec<LiveChannel> {
    vec![
        ch(1, "Sport 10", Some("Sport")),
        ch(2, "sport 2", Some("Sport")),
        ch(3, "Journaal", Some("Nieuws;Sport")),
        ch(4, "Zonder", None),
        ch(5, "Alpha", Some("nieuws")),
        ch(6, "Kids 1", Some("Kinderen")),
    ]
}

#[test]
fn the_groups_come_in_their_order_with_their_counts() {
    let g = build(sample(), &[], &[]);
    assert_eq!(
        summary(&g),
        ["ALL 6", "Kinderen 1", "Nieuws 1", "nieuws 1", "Sport 3", "NONE 1"]
    );
}

#[test]
fn channels_go_a_to_z_with_numbers_by_their_value() {
    let g = build(sample(), &[], &[]);
    assert_eq!(
        names(&g, 0),
        ["Alpha", "Journaal", "Kids 1", "sport 2", "Sport 10", "Zonder"]
    );
    assert_eq!(
        names(&g, group_of(&g, "Sport")),
        ["Journaal", "sport 2", "Sport 10"],
        "a channel of two groups is in both"
    );
}

#[test]
fn a_defect_channel_is_in_defect_channels_only() {
    let g = build(sample(), &[2], &[]);
    assert_eq!(
        summary(&g),
        ["ALL 5", "Kinderen 1", "Nieuws 1", "nieuws 1", "Sport 2", "NONE 1", "DEFECT 1"]
    );
    assert_eq!(names(&g, g.find(&GroupKey::Defect).unwrap()), ["sport 2"]);
    assert_eq!(g.channel_count(), 6, "the defect one too");
    // A mark of a channel the list does not have makes no group.
    assert!(build(sample(), &[99], &[]).find(&GroupKey::Defect).is_none());
}

#[test]
fn favorites_show_when_the_list_has_one() {
    let g = build(sample(), &[], &[3, 99]);
    assert_eq!(g.find(&GroupKey::Favorites), Some(1), "second, after All");
    assert_eq!(names(&g, 1), ["Journaal"]);
    assert!(build(sample(), &[], &[99]).find(&GroupKey::Favorites).is_none());
    // A defect favorite is in Defect channels only: here no Favorites, and its
    // only other group, Nieuws, holds nothing and goes.
    let g = build(sample(), &[3], &[3]);
    assert_eq!(
        summary(&g),
        ["ALL 5", "Kinderen 1", "nieuws 1", "Sport 2", "NONE 1", "DEFECT 1"]
    );
}

#[test]
fn no_group_shows_only_when_a_channel_has_none() {
    let g = build(vec![ch(1, "Een", Some("X"))], &[], &[]);
    assert_eq!(summary(&g), ["ALL 1", "X 1"]);
}

#[test]
fn untidy_group_titles_are_read_as_people_mean_them() {
    let g = build(
        vec![
            ch(1, "Een", Some(" Kinderen ; ;Kinderen")),
            ch(2, "Twee", Some("  ")),
            ch(3, "Drie", Some(";Sport;")),
        ],
        &[],
        &[],
    );
    assert_eq!(summary(&g), ["ALL 3", "Kinderen 1", "Sport 1", "NONE 1"]);
}

#[test]
fn channels_of_one_name_keep_the_files_order() {
    let g = build(
        vec![ch(5, "Een", None), ch(2, "een", None), ch(9, "Een", None)],
        &[],
        &[],
    );
    let ids: Vec<i64> = (0..3).map(|r| g.channel(0, r).unwrap().stream_id).collect();
    assert_eq!(ids, [5, 2, 9]);
}

#[test]
fn what_the_catalog_hides_is_nowhere() {
    let g = ChannelGroups::build(
        Arc::new(sample()),
        |c| c.stream_id != 1,
        &HashMap::new(),
        &HashSet::new(),
    );
    assert_eq!(names(&g, group_of(&g, "Sport")), ["Journaal", "sport 2"]);
    assert_eq!(g.rows(0), 5);
}

#[test]
fn natural_order_reads_like_people_read() {
    assert_eq!(natural_cmp("Sport 2", "Sport 10"), Ordering::Less);
    assert_eq!(natural_cmp("Kanaal 9", "Kanaal 10"), Ordering::Less);
    assert_eq!(natural_cmp("a", "B"), Ordering::Less);
    assert_eq!(natural_cmp("B", "a"), Ordering::Greater);
    assert_eq!(natural_cmp("M3U 01", "M3U DOOD"), Ordering::Less, "digits before letters");
    assert_eq!(natural_cmp("x01", "x1"), Ordering::Equal);
    assert_eq!(natural_cmp("abc", "ab"), Ordering::Greater);
    assert_eq!(natural_cmp("ÉÉN", "één"), Ordering::Equal);
    assert_eq!(
        natural_cmp("Zender 99999999999999999999", "Zender 100000000000000000000"),
        Ordering::Less,
        "longer than any integer"
    );
}

#[test]
fn the_list_opens_on_all_channels_on_the_channel_that_plays() {
    let g = build(sample(), &[], &[]);
    assert_eq!(Cursor::open(&g, None, None), Cursor { group: 0, row: 0 });
    assert_eq!(
        Cursor::open(&g, None, Some(1)),
        Cursor { group: 0, row: 4 },
        "Sport 10"
    );
}

#[test]
fn it_opens_in_the_group_used_last_on_what_plays_or_on_the_first_row() {
    let g = build(sample(), &[], &[]);
    let sport = GroupKey::Named("Sport".into());
    let at = group_of(&g, "Sport");
    assert_eq!(Cursor::open(&g, Some(&sport), Some(1)), Cursor { group: at, row: 2 });
    assert_eq!(
        Cursor::open(&g, Some(&sport), Some(4)),
        Cursor { group: at, row: 0 },
        "Zonder is not in Sport"
    );
    assert_eq!(
        Cursor::open(&g, Some(&GroupKey::Favorites), Some(1)).group,
        0,
        "a group that is gone: All"
    );
}

#[test]
fn rows_stop_at_both_ends() {
    let g = build(sample(), &[], &[]);
    let mut c = Cursor::open(&g, None, None);
    assert!(!c.step(&g, -1));
    assert!(c.step(&g, 1));
    assert_eq!(c.row, 1);
    assert!(c.step(&g, 10));
    assert_eq!(c.row, 5, "a page past the end stops on the last");
    assert!(!c.step(&g, 1));
    assert!(c.to_end(&g, false));
    assert_eq!(c.row, 0);
    assert!(!c.to_end(&g, false));
    assert!(c.to_end(&g, true));
    assert_eq!(c.row, 5);
    assert!(c.step(&g, -3));
    assert_eq!(c.row, 2);
}

#[test]
fn groups_step_and_stop_at_both_ends() {
    let g = build(sample(), &[], &[]);
    let mut c = Cursor::open(&g, None, Some(3));
    assert_eq!(c.row, 1, "Journaal");
    assert!(!c.step_group(&g, -1, Some(3)));
    assert!(c.step_group(&g, 1, Some(3)));
    assert_eq!((c.group, c.row), (1, 0), "Kinderen: Journaal is not in it");
    assert!(c.step_group(&g, 1, Some(3)));
    assert_eq!((c.group, c.row), (2, 0), "Nieuws, on Journaal");
    for _ in 0..10 {
        c.step_group(&g, 1, Some(3));
    }
    assert_eq!(c.group, 5);
    assert!(!c.step_group(&g, 1, Some(3)));
    let sport = group_of(&g, "Sport");
    assert!(c.select_group(&g, sport, Some(3)));
    assert_eq!((c.group, c.row), (sport, 0), "Sport, on Journaal");
    assert!(!c.select_group(&g, sport, Some(3)), "the group it is in");
    assert!(!c.select_group(&g, 99, None));
}

#[test]
fn a_click_selects_a_row_of_the_group() {
    let g = build(sample(), &[], &[]);
    let mut c = Cursor::open(&g, None, None);
    assert!(c.select_row(&g, 3));
    assert_eq!(c.row, 3);
    assert!(!c.select_row(&g, 3));
    assert!(!c.select_row(&g, 6), "past the last row");
    assert_eq!(c.channel(&g).map(|ch| ch.name.as_str()), Some("sport 2"));
}

#[test]
fn the_selection_follows_groups_made_anew() {
    let old = build(sample(), &[], &[]);
    let sport = GroupKey::Named("Sport".into());
    let mut c = Cursor::open(&old, Some(&sport), Some(2));
    assert_eq!(c.channel(&old).map(|ch| ch.name.as_str()), Some("sport 2"));
    // Another copy of the list, with a channel before it: the same channel.
    let mut more = sample();
    more.push(ch(7, "Aaa", Some("Sport")));
    let new = build(more.clone(), &[], &[]);
    c.follow(&old, &new);
    assert_eq!(c.channel(&new).map(|ch| ch.name.as_str()), Some("sport 2"));
    assert_eq!(c.row, 2);
    // It is marked: the same row, its neighbour now.
    let marked = build(more, &[2], &[]);
    c.follow(&new, &marked);
    assert_eq!(c.group, group_of(&marked, "Sport"));
    assert_eq!(c.channel(&marked).map(|ch| ch.name.as_str()), Some("Sport 10"));
    // Its group is gone: All, on the channel.
    let gone = build(vec![ch(1, "Sport 10", None)], &[], &[]);
    c.follow(&marked, &gone);
    assert_eq!(c, Cursor { group: 0, row: 0 });
}

#[test]
fn the_catalog_builds_the_groups_with_its_own_marks() {
    let list = "#EXTM3U\n\
#EXTINF:-1 group-title=\"Nieuws\",Een\nhttps://a.example/1.m3u8\n\
#EXTINF:-1 group-title=\"Nieuws\",Twee\nhttps://a.example/2.m3u8\n";
    let (portal, catalog) = M3uPortal::from_text(list).unwrap();
    let store = CatalogStore::from_catalog(Arc::new(portal), catalog);
    store.set_defect([(stream_id("Twee"), 0)].into());
    let g = store
        .channel_groups(&HashSet::from([stream_id("Een")]))
        .unwrap();
    assert_eq!(summary(&g), ["ALL 1", "FAV 1", "Nieuws 1", "DEFECT 1"]);
    let (portal, _) = M3uPortal::from_text(list).unwrap();
    let unloaded = CatalogStore::new(Arc::new(portal));
    assert!(unloaded.channel_groups(&HashSet::new()).is_none());
}

/// The spec's budget (§5): 20 ms for 11,000 channels in a release build, under
/// 1 MB kept. Prints the time; asserts the memory, which does not depend on the
/// machine. Task 10 runs it with `--release -- --nocapture`.
#[test]
fn eleven_thousand_channels_are_grouped_within_the_budget() {
    // iptv-org's shape: 29 groups, every seventh channel in two, numbers in names.
    let live: Vec<LiveChannel> = (0..11_000_i64)
        .map(|i| {
            let group = if i % 7 == 0 {
                format!("Groep {};Groep {}", i % 29, (i + 3) % 29)
            } else {
                format!("Groep {}", i % 29)
            };
            ch(i, &format!("Zender {} HD", (i * 7919) % 11_000), Some(&group))
        })
        .collect();
    let defect: HashMap<i64, i64> = (0..11_000_i64).step_by(37).map(|i| (i, 0)).collect();
    let favorites: HashSet<i64> = (0..11_000_i64).step_by(500).collect();
    let live = Arc::new(live);
    let t0 = std::time::Instant::now();
    let g = ChannelGroups::build(live, |_| true, &defect, &favorites);
    let took = t0.elapsed();
    let kb = g.heap_bytes() / 1024;
    println!(
        "grouping: 11000 channels, {} groups, {:.1} ms, {} KB kept",
        g.group_count(),
        took.as_secs_f64() * 1000.0,
        kb
    );
    assert!(g.heap_bytes() < 1024 * 1024, "{kb} KB");
    assert_eq!(g.rows(0) + defect.len(), 11_000);
    assert_eq!(g.channel_count(), 11_000);
}
```

Run `rustfmt --edition 2021 tests/test_channel_list.rs` once written.

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_channel_list`
Expected: compile errors: no module `channel_list`, no `channel_groups`.

- [ ] **Step 3: Write the module**

Create `rust/src/channel_list.rs`:

```rust
//! The channel list of an M3U source (`g`; spec 2026-10-15, §3): its groups,
//! A–Z, and where the selection is. Worked out from the catalog's channels when
//! the list opens (`CatalogStore::channel_groups`): positions in them, never
//! copies. Draws nothing; channel_list_ui.rs does.

use crate::portal::LiveChannel;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A group of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupKey {
    /// Every channel but the defect ones.
    All,
    /// The source's favorites that are in the list.
    Favorites,
    /// One of the list's own groups (`group-title`), spelled as the file has it.
    Named(String),
    /// The channels without a group.
    NoGroup,
    /// The channels marked defect, and only there.
    Defect,
}

/// One group: its channels A–Z, as positions in the catalog's channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub key: GroupKey,
    pub members: Vec<u32>,
}

/// The list's groups in their order (spec §3.2): All channels; Favorites when
/// the list holds one; the list's groups A–Z; No group when a channel has
/// none; Defect channels when a channel of the list is marked.
pub struct ChannelGroups {
    live: Arc<Vec<LiveChannel>>,
    groups: Vec<Group>,
}

impl ChannelGroups {
    /// The groups of `live`. `shows` says which channels the catalog shows at
    /// all (its adult filter); `defect` holds the marked ones, which are in
    /// Defect channels only (spec §4.3); `favorites` the source's favorites.
    /// One sort for every group: each takes its channels in that order.
    pub fn build(
        live: Arc<Vec<LiveChannel>>,
        shows: impl Fn(&LiveChannel) -> bool,
        defect: &HashMap<i64, i64>,
        favorites: &HashSet<i64>,
    ) -> Self {
        let mut keyed: Vec<(String, u32)> = live
            .iter()
            .enumerate()
            .filter(|(_, c)| shows(c))
            .map(|(i, c)| (c.name.to_lowercase(), i as u32))
            .collect();
        keyed.sort_by(|(a, i), (b, j)| compare_folded(a, b).then(i.cmp(j)));
        let (mut all, mut favs, mut none, mut broken) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut named: HashMap<&str, Vec<u32>> = HashMap::new();
        for &(_, i) in &keyed {
            let channel = &live[i as usize];
            if defect.contains_key(&channel.stream_id) {
                broken.push(i);
                continue;
            }
            all.push(i);
            if favorites.contains(&channel.stream_id) {
                favs.push(i);
            }
            let mut grouped = false;
            for name in group_names(channel.category_id.as_deref()) {
                grouped = true;
                let members = named.entry(name).or_default();
                // "A;A" puts the channel in A once.
                if members.last() != Some(&i) {
                    members.push(i);
                }
            }
            if !grouped {
                none.push(i);
            }
        }
        drop(keyed);
        let mut named: Vec<(&str, Vec<u32>)> = named.into_iter().collect();
        named.sort_by(|(a, _), (b, _)| natural_cmp(a, b).then(a.cmp(b)));
        let mut groups = vec![Group {
            key: GroupKey::All,
            members: all,
        }];
        if !favs.is_empty() {
            groups.push(Group {
                key: GroupKey::Favorites,
                members: favs,
            });
        }
        groups.extend(named.into_iter().map(|(name, members)| Group {
            key: GroupKey::Named(name.to_owned()),
            members,
        }));
        if !none.is_empty() {
            groups.push(Group {
                key: GroupKey::NoGroup,
                members: none,
            });
        }
        if !broken.is_empty() {
            groups.push(Group {
                key: GroupKey::Defect,
                members: broken,
            });
        }
        Self { live, groups }
    }

    /// The groups, in the list's order.
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    pub fn key(&self, group: usize) -> Option<&GroupKey> {
        self.groups.get(group).map(|g| &g.key)
    }

    /// How many channels group `group` holds.
    pub fn rows(&self, group: usize) -> usize {
        self.groups.get(group).map_or(0, |g| g.members.len())
    }

    /// The channel at `row` of group `group`.
    pub fn channel(&self, group: usize, row: usize) -> Option<&LiveChannel> {
        let &index = self.groups.get(group)?.members.get(row)?;
        self.live.get(index as usize)
    }

    /// The row of channel `stream_id` in group `group`.
    pub fn row_of(&self, group: usize, stream_id: i64) -> Option<usize> {
        self.groups.get(group)?.members.iter().position(|&i| {
            self.live
                .get(i as usize)
                .is_some_and(|c| c.stream_id == stream_id)
        })
    }

    /// The group with this key.
    pub fn find(&self, key: &GroupKey) -> Option<usize> {
        self.groups.iter().position(|g| g.key == *key)
    }

    /// How many channels the list shows, the defect ones too (for the log).
    pub fn channel_count(&self) -> usize {
        self.groups
            .iter()
            .filter(|g| matches!(g.key, GroupKey::All | GroupKey::Defect))
            .map(|g| g.members.len())
            .sum()
    }

    /// About how many bytes the groups keep on the heap: their positions and
    /// names. The channels are the catalog's.
    pub fn heap_bytes(&self) -> usize {
        let groups: usize = self
            .groups
            .iter()
            .map(|g| {
                let name = match &g.key {
                    GroupKey::Named(name) => name.capacity(),
                    _ => 0,
                };
                g.members.capacity() * std::mem::size_of::<u32>() + name
            })
            .sum();
        self.groups.capacity() * std::mem::size_of::<Group>() + groups
    }
}

/// The groups of a channel's `group-title`: its `;`-separated parts, trimmed,
/// the empty ones left out.
pub fn group_names(category: Option<&str>) -> impl Iterator<Item = &str> {
    category
        .unwrap_or("")
        .split(';')
        .map(str::trim)
        .filter(|g| !g.is_empty())
}

/// A–Z as people read it: case aside, a run of digits by its value ("Sport 2"
/// before "Sport 10"), digits before letters. Texts that differ only in case or
/// in leading zeros compare equal.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    compare_folded(&a.to_lowercase(), &b.to_lowercase())
}

/// [`natural_cmp`] of texts in lower case already. Bytewise, which for UTF-8 is
/// the order of the characters; an ASCII digit is never part of another one.
fn compare_folded(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (from_i, from_j) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let (x, y) = (without_zeros(&a[from_i..i]), without_zeros(&b[from_j..j]));
            let by_value = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if by_value != Ordering::Equal {
                return by_value;
            }
        } else if a[i] != b[j] {
            return a[i].cmp(&b[j]);
        } else {
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn without_zeros(digits: &[u8]) -> &[u8] {
    let first = digits
        .iter()
        .position(|&d| d != b'0')
        .unwrap_or(digits.len());
    &digits[first..]
}

/// Where the selection is: a group, and a row in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub group: usize,
    pub row: usize,
}

impl Cursor {
    /// Where the list opens (spec §3.3): in group `last` (the one open last this
    /// session; All when there is none, or it is gone), on channel `playing` when
    /// that group holds it, else on its first row.
    pub fn open(groups: &ChannelGroups, last: Option<&GroupKey>, playing: Option<i64>) -> Self {
        let group = last.and_then(|key| groups.find(key)).unwrap_or(0);
        Self {
            group,
            row: Self::landing(groups, group, playing),
        }
    }

    /// The row a group is entered on: `playing`'s when it is in it, else the first.
    fn landing(groups: &ChannelGroups, group: usize, playing: Option<i64>) -> usize {
        playing
            .and_then(|sid| groups.row_of(group, sid))
            .unwrap_or(0)
    }

    /// `delta` rows on (↑ ↓ PgUp PgDn), stopping at the first and the last row:
    /// the list does not wrap. Whether the selection moved.
    pub fn step(&mut self, groups: &ChannelGroups, delta: isize) -> bool {
        let last = groups.rows(self.group).saturating_sub(1);
        let to = self.row.saturating_add_signed(delta).min(last);
        let moved = to != self.row;
        self.row = to;
        moved
    }

    /// The first row (Home) or the last (End). Whether the selection moved.
    pub fn to_end(&mut self, groups: &ChannelGroups, last: bool) -> bool {
        let to = if last {
            groups.rows(self.group).saturating_sub(1)
        } else {
            0
        };
        let moved = to != self.row;
        self.row = to;
        moved
    }

    /// The group before (`delta` -1) or after (+1), stopping at the first and
    /// the last (←/→), entered on `playing` when it holds it, else on its first
    /// row. Whether the group changed.
    pub fn step_group(&mut self, groups: &ChannelGroups, delta: isize, playing: Option<i64>) -> bool {
        let last = groups.group_count().saturating_sub(1);
        let to = self.group.saturating_add_signed(delta).min(last);
        self.select_group(groups, to, playing)
    }

    /// Group `group` (a click), entered as `step_group` enters one. Whether the
    /// group changed.
    pub fn select_group(&mut self, groups: &ChannelGroups, group: usize, playing: Option<i64>) -> bool {
        if group == self.group || group >= groups.group_count() {
            return false;
        }
        self.group = group;
        self.row = Self::landing(groups, group, playing);
        true
    }

    /// Row `row` of the group (a click). Whether the selection moved.
    pub fn select_row(&mut self, groups: &ChannelGroups, row: usize) -> bool {
        if row >= groups.rows(self.group) || row == self.row {
            return false;
        }
        self.row = row;
        true
    }

    /// The same place in groups made anew while the list is open (another copy
    /// of the list, a channel marked or cleared): the same group by its key,
    /// else All; the same channel when the group still holds it, else the same
    /// row, as far as the group goes.
    pub fn follow(&mut self, old: &ChannelGroups, new: &ChannelGroups) {
        let selected = old.channel(self.group, self.row).map(|c| c.stream_id);
        let group = old
            .key(self.group)
            .and_then(|key| new.find(key))
            .unwrap_or(0);
        let row = selected
            .and_then(|sid| new.row_of(group, sid))
            .unwrap_or(self.row);
        self.group = group;
        self.row = row.min(new.rows(group).saturating_sub(1));
    }

    /// The selected channel; none in an empty group.
    pub fn channel<'a>(&self, groups: &'a ChannelGroups) -> Option<&'a LiveChannel> {
        groups.channel(self.group, self.row)
    }
}
```

`drop(keyed)` frees the sort keys (about 0.6 MB for 11,000 channels) before the groups are put together; the borrow checker accepts it because `named` borrows from `live`, not from `keyed`.

Run `rustfmt --edition 2021 src/channel_list.rs` once written.

In `rust/src/lib.rs`, add `pub mod channel_list;` after `pub mod catalog;`.

- [ ] **Step 4: The catalog's side**

In `rust/src/catalog.rs` (note the hunk count first): add `use crate::channel_list::ChannelGroups;` to the imports, and after `defect_revision` (Task 2) add:

```rust
    /// The channel list's groups ([`ChannelGroups::build`]) of the channels that
    /// show (the adult ones while locked do not), the defect ones in their own
    /// group, with `favorites` as the source's favorites. One read lock to take
    /// the channels (an `Arc`), the work outside it; `None` until a catalog has
    /// loaded.
    pub fn channel_groups(&self, favorites: &HashSet<i64>) -> Option<ChannelGroups> {
        let live = self.inner.read().as_ref()?.live.clone();
        let unlocked = self.adult_unlocked();
        let defect = self.defect_marks();
        Some(ChannelGroups::build(
            live,
            |c| unlocked || !adult_live(c),
            &defect,
            favorites,
        ))
    }
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `ct --test test_channel_list -- --nocapture`
Expected: all pass. The budget test prints its line (debug build: slower than the release budget; note it anyway).

- [ ] **Step 6: Format, lint, full suite**

- Run `fmt src/channel_list.rs tests/test_channel_list.rs`. Expected: no output.
- Check the hunk count of `catalog.rs`: 3, as at the baseline (Task 1).
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes.

- [ ] **Step 7: Commit**

Message: `feat(rust): the channel list's groups A-Z, as positions in the catalog's channels, and its selection`, followed by the two attribution lines.
Files: `rust/src/channel_list.rs`, `rust/src/lib.rs`, `rust/src/catalog.rs`, `rust/tests/test_channel_list.rs`.

---

### Task 7: The channel list on screen

A view in the guide's pattern: `ChannelListView::show` draws the list over the picture (the rows on screen only), reads its keys and the mouse, and returns `ListAction`s for app.rs.

**Files:**
- Create: `rust/src/channel_list_ui.rs`
- Modify: `rust/src/lib.rs` (`pub mod channel_list_ui;` after `pub mod channel_list;`)
- Modify: `rust/src/guide_ui.rs` (`single_line`, `play_mark` and `CLICK` shared)
- Modify: `rust/src/i18n.rs`

**Interfaces:**
- Consumes (Task 6): `ChannelGroups::{key, rows, channel, group_count, find}`, `Cursor`, `GroupKey`.
- Produces:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum ListAction {
      Preview { stream_id: i64, name: String, repeat: bool },
      Play { stream_id: i64, name: String },
      GroupChanged,
      Close,
  }
  pub struct ListData<'a> { pub lang: Lang, pub playing: Option<i64>,
      pub marks: &'a HashMap<i64, i64>, pub now_unix: i64 }
  pub struct ChannelListView;
  impl ChannelListView {
      pub fn new(groups: ChannelGroups, last: Option<&GroupKey>, playing: Option<i64>) -> Self;
      pub fn groups(&self) -> &ChannelGroups;
      pub fn cursor(&self) -> Cursor;
      pub fn group_key(&self) -> Option<&GroupKey>;
      pub fn replace_groups(&mut self, groups: ChannelGroups);
      pub fn show(&mut self, ctx: &egui::Context, data: &ListData) -> Vec<ListAction>;
  }
  pub(crate) struct Layout { pub(crate) panel: Rect, pub(crate) groups: Rect,
      pub(crate) rows: Rect, pub(crate) visible: usize }
  impl Layout { pub(crate) fn of(screen: Rect) -> Self; pub(crate) fn row(column: Rect, k: usize) -> Rect; }
  pub fn group_label(lang: Lang, key: &GroupKey) -> &str;
  pub fn failed_ago(lang: Lang, secs: i64) -> String;
  // guide_ui.rs
  pub(crate) fn single_line(text: &str, font: FontId, color: Color32, width: f32) -> egui::text::LayoutJob;
  pub(crate) fn play_mark(p: &egui::Painter, center: Pos2, color: Color32);
  pub(crate) const CLICK: Sense;   // a click target the keyboard never reaches
  ```
  i18n keys: `ChannelListTitle`, `ListAllChannels`, `ListNoGroup`, `ListDefect`, `ListEmptyGroup`, `ChannelListFooter`, `AgoJustNow`, `AgoMinutes`, `AgoHour`, `AgoHours`, `AgoDay`, `AgoDays`.

- [ ] **Step 1: Write the failing tests**

Create `rust/src/channel_list_ui.rs` holding only its test module for now (Step 4 writes the code above it):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_list::{ChannelGroups, Cursor, GroupKey};
    use crate::portal::LiveChannel;
    use egui::{Event, Key, Pos2, Rect, Vec2};
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    /// 15 October 2026, 00:00 UTC.
    const NOW: i64 = 1_792_022_400;

    fn ch(id: i64, name: &str, group: &str) -> LiveChannel {
        LiveChannel {
            stream_id: id,
            name: name.into(),
            category_id: Some(group.into()),
            epg_channel_id: None,
            tv_archive: 0,
            tv_archive_duration: 0,
            is_adult: false,
            stream_icon: None,
        }
    }

    /// Channels "Kanaal 001" to `n`, in "Groep", every third also in "Ander";
    /// `marked` of them defect. The groups: All, Ander, Groep (and Defect).
    fn groups(n: i64, marked: &[(i64, i64)]) -> ChannelGroups {
        let live: Vec<LiveChannel> = (1..=n)
            .map(|i| {
                let group = if i % 3 == 0 { "Groep;Ander" } else { "Groep" };
                ch(i, &format!("Kanaal {i:03}"), group)
            })
            .collect();
        let marks: HashMap<i64, i64> = marked.iter().copied().collect();
        ChannelGroups::build(Arc::new(live), |_| true, &marks, &HashSet::new())
    }

    fn data(playing: Option<i64>, marks: &HashMap<i64, i64>) -> ListData<'_> {
        ListData {
            lang: Lang::En,
            playing,
            marks,
            now_unix: NOW,
        }
    }

    fn press(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn preview(id: i64, repeat: bool) -> ListAction {
        ListAction::Preview {
            stream_id: id,
            name: format!("Kanaal {id:03}"),
            repeat,
        }
    }

    /// The list on a headless egui context, in a window of 1280 x 720, with a
    /// clock of its own: 50 ms a frame.
    struct Screen {
        ctx: egui::Context,
        time: f64,
    }

    impl Screen {
        fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
                time: 0.0,
            }
        }

        fn screen() -> Rect {
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))
        }

        fn layout() -> Layout {
            Layout::of(Self::screen())
        }

        /// A second goes by: the next click is no double click.
        fn pause(&mut self) {
            self.time += 1.0;
        }

        /// One frame with these events: the actions, and everything drawn.
        fn run(
            &mut self,
            v: &mut ChannelListView,
            d: &ListData,
            events: Vec<Event>,
        ) -> (Vec<ListAction>, Vec<egui::epaint::ClippedShape>) {
            self.time += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(Self::screen()),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut actions = Vec::new();
            let output = self.ctx.run(raw, |ctx| actions = v.show(ctx, d));
            // egui draws its complaint about two widgets with one id as text.
            let clash = output.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text().contains("use of widget ID") => {
                    Some(t.galley.text().to_string())
                }
                _ => None,
            });
            assert_eq!(clash, None, "two widgets share an id");
            (actions, output.shapes)
        }

        /// One frame with these events: the actions, and every text painted.
        fn frame(
            &mut self,
            v: &mut ChannelListView,
            d: &ListData,
            events: Vec<Event>,
        ) -> (Vec<ListAction>, Vec<String>) {
            let (actions, shapes) = self.run(v, d, events);
            let painted = shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
            (actions, painted)
        }

        /// One frame in which these keys are pressed: its actions.
        fn keys(&mut self, v: &mut ChannelListView, d: &ListData, keys: &[Key]) -> Vec<ListAction> {
            self.frame(v, d, keys.iter().map(|&k| press(k)).collect()).0
        }

        /// A click at `pos`, one frame per step: the actions of the release.
        fn click(&mut self, v: &mut ChannelListView, d: &ListData, pos: Pos2) -> Vec<ListAction> {
            let button = |pressed| Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(v, d, vec![Event::PointerMoved(pos)]);
            self.frame(v, d, vec![button(true)]);
            self.frame(v, d, vec![button(false)]).0
        }
    }

    /// A new view, and its first frame drawn: egui keeps a new area invisible
    /// in its first frame.
    fn open(n: i64, marked: &[(i64, i64)], last: Option<&GroupKey>, playing: Option<i64>, s: &mut Screen, d: &ListData) -> ChannelListView {
        let mut v = ChannelListView::new(groups(n, marked), last, playing);
        s.frame(&mut v, d, vec![]);
        v
    }

    #[test]
    fn down_previews_the_next_channel_and_up_stops_at_the_first() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowDown]), [preview(2, false)]);
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowUp]), [preview(1, false)]);
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowUp]), [], "the first row: it stops");
        assert_eq!(v.cursor().row, 0);
    }

    #[test]
    fn a_held_key_says_it_repeats_and_two_presses_in_a_frame_move_twice() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowDown]), [preview(2, false)]);
        // The key was never let go: egui calls the next press a repeat.
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowDown]), [preview(3, true)]);
        assert_eq!(
            s.keys(&mut v, &d, &[Key::ArrowDown, Key::ArrowDown]),
            [preview(4, true), preview(5, true)]
        );
    }

    #[test]
    fn page_keys_jump_a_screen_and_home_and_end_go_to_the_ends() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(100, &[], None, None, &mut s, &d);
        let page = Screen::layout().visible as i64;
        assert_eq!(s.keys(&mut v, &d, &[Key::PageDown]), [preview(1 + page, false)]);
        assert_eq!(s.keys(&mut v, &d, &[Key::End]), [preview(100, false)]);
        assert_eq!(s.keys(&mut v, &d, &[Key::End]), []);
        assert_eq!(s.keys(&mut v, &d, &[Key::Home]), [preview(1, false)]);
        assert_eq!(s.keys(&mut v, &d, &[Key::PageUp]), []);
    }

    #[test]
    fn left_and_right_change_the_group_and_stop_at_the_ends() {
        let marks = HashMap::new();
        let d = data(Some(4), &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, Some(4), &mut s, &d);
        assert_eq!(v.cursor(), Cursor { group: 0, row: 3 }, "on Kanaal 004");
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowLeft]), [], "the first group");
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowRight]), [ListAction::GroupChanged]);
        assert_eq!(v.group_key(), Some(&GroupKey::Named("Ander".into())));
        assert_eq!(v.cursor().row, 0, "004 is not in Ander");
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowRight]), [ListAction::GroupChanged]);
        assert_eq!(v.cursor(), Cursor { group: 2, row: 3 }, "Groep, on 004");
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowRight]), [], "the last group");
    }

    #[test]
    fn enter_plays_the_selection_and_esc_and_g_close() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        assert_eq!(
            s.keys(&mut v, &d, &[Key::Enter]),
            [ListAction::Play {
                stream_id: 1,
                name: "Kanaal 001".into()
            }]
        );
        assert_eq!(s.keys(&mut v, &d, &[Key::Escape]), [ListAction::Close]);
        assert_eq!(s.keys(&mut v, &d, &[Key::G]), [ListAction::Close]);
        assert_eq!(
            s.keys(&mut v, &d, &[Key::Escape, Key::ArrowDown]),
            [ListAction::Close],
            "what comes after a close is not the list's"
        );
    }

    #[test]
    fn an_empty_group_says_so_and_enter_only_closes() {
        let marks: HashMap<i64, i64> = [(1, NOW), (2, NOW)].into();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(2, &[(1, NOW), (2, NOW)], None, None, &mut s, &d);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(painted.iter().any(|t| t == "no channels in this group"), "{painted:?}");
        assert_eq!(s.keys(&mut v, &d, &[Key::Enter]), [ListAction::Close]);
    }

    #[test]
    fn the_list_paints_its_title_its_groups_with_their_counts_its_rows_and_its_keys() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        let footer = "Up / Down = choose  ·  Left / Right = group  ·  PgUp / PgDn, Home / End = jump  ·  Enter = play  ·  Esc or g = close";
        for text in [
            "Channel list",
            "All channels",
            "Ander",
            "Groep",
            "5",
            "1",
            "Kanaal 001",
            "Kanaal 005",
            footer,
        ] {
            assert!(painted.iter().any(|t| t == text), "{text}: {painted:?}");
        }
    }

    #[test]
    fn only_the_rows_on_screen_are_painted() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(200, &[], None, None, &mut s, &d);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(painted.iter().any(|t| t == "Kanaal 001"));
        assert!(!painted.iter().any(|t| t == "Kanaal 150"));
        let (_, painted) = s.frame(&mut v, &d, vec![press(Key::End)]);
        assert!(painted.iter().any(|t| t == "Kanaal 200"));
        assert!(!painted.iter().any(|t| t == "Kanaal 001"));
    }

    #[test]
    fn the_channel_that_plays_has_a_play_mark_on_its_row() {
        let marks = HashMap::new();
        let d = data(Some(3), &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, Some(3), &mut s, &d);
        let (_, shapes) = s.run(&mut v, &d, vec![]);
        let play_marks: Vec<Rect> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Path(path) if path.closed && path.fill == PLAYING => {
                    Some(Rect::from_points(&path.points))
                }
                _ => None,
            })
            .collect();
        assert_eq!(play_marks.len(), 1, "{play_marks:?}");
        assert!(Layout::row(Screen::layout().rows, 2).contains_rect(play_marks[0]));
    }

    #[test]
    fn broken_channels_say_how_long_ago_they_failed() {
        let failed = [(1, NOW - 200), (2, NOW - 2 * 3600 - 10), (3, NOW - 3 * 86_400)];
        let marks: HashMap<i64, i64> = failed.into();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &failed, Some(&GroupKey::Defect), None, &mut s, &d);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        for text in ["Broken channels", "3 min ago", "2 hours ago", "3 days ago"] {
            assert!(painted.iter().any(|t| t == text), "{text}: {painted:?}");
        }
    }

    #[test]
    fn failed_ago_reads_as_people_say_it() {
        for (secs, text) in [
            (-5, "just now"),
            (59, "just now"),
            (60, "1 min ago"),
            (3_599, "59 min ago"),
            (3_600, "1 hour ago"),
            (7_199, "1 hour ago"),
            (7_200, "2 hours ago"),
            (86_399, "23 hours ago"),
            (86_400, "1 day ago"),
            (172_799, "1 day ago"),
            (172_800, "2 days ago"),
            (604_800, "7 days ago"),
        ] {
            assert_eq!(failed_ago(Lang::En, secs), text, "{secs}");
        }
        assert_eq!(failed_ago(Lang::Nl, 120), "2 min geleden");
        assert_eq!(failed_ago(Lang::De, 7_200), "vor 2 Stunden");
    }

    #[test]
    fn a_long_name_is_cut_off() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let long = format!("Kanaal {}", "x".repeat(300));
        let live = vec![ch(1, &long, "Groep")];
        let groups = ChannelGroups::build(Arc::new(live), |_| true, &HashMap::new(), &HashSet::new());
        let mut v = ChannelListView::new(groups, None, None);
        let mut s = Screen::new();
        s.frame(&mut v, &d, vec![]);
        let (_, shapes) = s.run(&mut v, &d, vec![]);
        let elided = shapes.iter().any(|s| {
            matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == long && t.galley.elided)
        });
        assert!(elided, "the name ends in …");
    }

    #[test]
    fn a_click_on_a_group_opens_it_and_a_click_on_a_channel_starts_it() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        let layout = Screen::layout();
        let ander = Layout::row(layout.groups, 1).center();
        assert_eq!(s.click(&mut v, &d, ander), [ListAction::GroupChanged]);
        assert_eq!(v.group_key(), Some(&GroupKey::Named("Ander".into())));
        s.pause();
        let all = Layout::row(layout.groups, 0).center();
        assert_eq!(s.click(&mut v, &d, all), [ListAction::GroupChanged]);
        s.pause();
        let third = Layout::row(layout.rows, 2).center();
        assert_eq!(s.click(&mut v, &d, third), [preview(3, false)]);
        assert_eq!(v.cursor().row, 2);
    }

    #[test]
    fn a_double_click_starts_the_channel_and_then_closes_the_list() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        let second = Layout::row(Screen::layout().rows, 1).center();
        assert_eq!(s.click(&mut v, &d, second), [preview(2, false)]);
        assert_eq!(
            s.click(&mut v, &d, second),
            [ListAction::Play {
                stream_id: 2,
                name: "Kanaal 002".into()
            }]
        );
    }

    #[test]
    fn the_wheel_scrolls_without_moving_the_selection_or_starting_anything() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(100, &[], None, None, &mut s, &d);
        let over = Layout::row(Screen::layout().rows, 3).center();
        s.frame(&mut v, &d, vec![Event::PointerMoved(over)]);
        let (actions, painted) = s.frame(&mut v, &d, vec![Event::Scroll(Vec2::new(0.0, -90.0))]);
        assert!(actions.is_empty(), "{actions:?}");
        assert_eq!(v.cursor().row, 0, "the selection stays");
        assert!(!painted.iter().any(|t| t == "Kanaal 001"), "three rows down");
        assert!(painted.iter().any(|t| t == "Kanaal 004"));
        // The next key brings the selection back on screen.
        let (_, painted) = s.frame(&mut v, &d, vec![press(Key::ArrowDown)]);
        assert!(painted.iter().any(|t| t == "Kanaal 002"));
    }

    #[test]
    fn groups_made_anew_keep_the_selection_on_its_channel() {
        let mut v = ChannelListView::new(groups(5, &[]), None, Some(3));
        assert_eq!(v.cursor(), Cursor { group: 0, row: 2 });
        v.replace_groups(groups(5, &[(1, NOW)]));
        assert_eq!(v.cursor(), Cursor { group: 0, row: 1 }, "Kanaal 003, one up");
        let name = v.cursor().channel(v.groups()).map(|c| c.name.clone());
        assert_eq!(name.as_deref(), Some("Kanaal 003"));
    }
}
```

In `rust/src/lib.rs`, add `pub mod channel_list_ui;` after `pub mod channel_list;`.

- [ ] **Step 2: Run them to see them fail**

Run: `ct --lib channel_list_ui::`
Expected: compile errors: no `ChannelListView`, `ListAction`, `ListData`, `Layout`, `failed_ago`, `PLAYING`.

- [ ] **Step 3: The texts, and what the guide shares**

In `rust/src/i18n.rs`, after the block under `// Sources: an M3U list`, add:

```rust
    // The channel list (g on an M3U source)
    ChannelListTitle => ["Zenderlijst", "Channel list", "Senderliste"],
    ListAllChannels => ["Alle zenders", "All channels", "Alle Sender"],
    ListNoGroup => ["Zonder groep", "No group", "Ohne Gruppe"],
    ListDefect => ["Defecte zenders", "Broken channels", "Defekte Sender"],
    ListEmptyGroup => ["geen zenders in deze groep", "no channels in this group", "keine Sender in dieser Gruppe"],
    ChannelListFooter => ["Omhoog / Omlaag = kiezen  ·  Links / Rechts = groep  ·  PgUp / PgDn, Home / End = verder  ·  Enter = afspelen  ·  Esc of g = sluiten", "Up / Down = choose  ·  Left / Right = group  ·  PgUp / PgDn, Home / End = jump  ·  Enter = play  ·  Esc or g = close", "Hoch / Runter = wählen  ·  Links / Rechts = Gruppe  ·  PgUp / PgDn, Home / End = springen  ·  Enter = abspielen  ·  Esc oder g = schließen"],
    AgoJustNow => ["zojuist", "just now", "gerade eben"],
    AgoMinutes => ["{0} min geleden", "{0} min ago", "vor {0} Min."],
    AgoHour => ["1 uur geleden", "1 hour ago", "vor 1 Stunde"],
    AgoHours => ["{0} uur geleden", "{0} hours ago", "vor {0} Stunden"],
    AgoDay => ["1 dag geleden", "1 day ago", "vor 1 Tag"],
    AgoDays => ["{0} dagen geleden", "{0} days ago", "vor {0} Tagen"],
```

The footer names the keys in words, as the guide's and the news picker's footers do.

In `rust/src/guide_ui.rs` (clean at the base: `fmt src/guide_ui.rs` shows nothing before and after):
- `const CLICK: Sense = Sense {` becomes `pub(crate) const CLICK: Sense = Sense {`, and its doc "A click target the keyboard never reaches: the guide's keys are its own, and a focused widget would take Enter or Space for a click." becomes "A click target the keyboard never reaches: the guide's keys are its own (the channel list's too), and a focused widget would take Enter or Space for a click."
- `fn single_line(` becomes `pub(crate) fn single_line(`.
- `play_mark` takes its colour: its doc becomes "▶: a past programme that can be watched back (the guide); the channel that plays (the channel list)."; its signature `pub(crate) fn play_mark(p: &egui::Painter, center: Pos2, color: Color32)`; `REPLAY_BLUE` in its body becomes `color`; and its call in `paint_row` becomes `Playable::Replay => play_mark(p, Pos2::new(row.right() - 14.0, row.center().y), REPLAY_BLUE),`.

- [ ] **Step 4: The view**

Above the test module in `rust/src/channel_list_ui.rs`, write:

```rust
//! The channel list over the running picture (`g` on an M3U source; spec
//! 2026-10-15, §3): the groups on the left, the chosen group's channels on the
//! right. `ChannelListView` keeps the list's state while it is open; `show`
//! draws one frame, the rows on screen only, and hands what the user did to
//! app.rs as [`ListAction`]s, as guide_ui.rs does.

use crate::channel_list::{ChannelGroups, Cursor, GroupKey};
use crate::guide_ui::{play_mark, single_line, CLICK};
use crate::i18n::{self, Lang, T};
use egui::{Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::HashMap;

/// What the user did in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListAction {
    /// The selection moved by a key, or a channel was clicked: it starts at the
    /// zap tempo (`repeat`: a held key's repeat, which only moves on).
    Preview {
        stream_id: i64,
        name: String,
        repeat: bool,
    },
    /// Enter, or a double click: the list closes and the channel plays now,
    /// unless it plays already.
    Play { stream_id: i64, name: String },
    /// Another group: nothing starts, and a start that waits for the tempo is
    /// dropped.
    GroupChanged,
    /// Esc or g: the list closes; what plays goes on, a waiting start is dropped.
    Close,
}

/// What the list shows but app.rs owns.
pub struct ListData<'a> {
    pub lang: Lang,
    /// The channel that plays: its row has a ▶.
    pub playing: Option<i64>,
    /// When each defect channel failed (unix seconds), for "how long ago".
    pub marks: &'a HashMap<i64, i64>,
    pub now_unix: i64,
}

const ROW_H: f32 = 30.0;
const MARGIN: f32 = 16.0;
const TITLE_H: f32 = 44.0;
const FOOTER_H: f32 = 28.0;
const GROUPS_W: f32 = 280.0;
const MIN_W: f32 = 320.0;
const MAX_W: f32 = 940.0;
/// Room left under the list: the toast ("streamfout op …") shows there.
const TOAST_ROOM: f32 = 64.0;

// The list's colours: tokens of its own, as the guide has its own; some have
// the guide's values (the selected and the hovered row, the accent, the green).
const PANEL: Color32 = Color32::from_rgb(18, 18, 24);
const ROW_SELECTED: Color32 = Color32::from_rgb(40, 55, 80);
const ROW_HOVER: Color32 = Color32::from_rgb(30, 30, 38);
const GROUP_OPEN: Color32 = Color32::from_rgb(34, 34, 46);
const ACCENT: Color32 = Color32::from_rgb(255, 80, 140);
const PLAYING: Color32 = Color32::from_rgb(120, 230, 100);
const BROKEN: Color32 = Color32::from_rgb(230, 120, 110);

/// Where the parts of the list go in a window `screen`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Layout {
    pub(crate) panel: Rect,
    pub(crate) groups: Rect,
    pub(crate) rows: Rect,
    /// How many rows a column has room for: a page.
    pub(crate) visible: usize,
}

impl Layout {
    pub(crate) fn of(screen: Rect) -> Self {
        let width = (screen.width() - 2.0 * MARGIN).clamp(MIN_W, MAX_W);
        let height = (screen.height() - MARGIN - TOAST_ROOM).max(TITLE_H + FOOTER_H + ROW_H);
        let panel = Rect::from_min_size(
            Pos2::new(screen.center().x - width / 2.0, screen.top() + MARGIN),
            Vec2::new(width, height),
        );
        let (top, bottom) = (panel.top() + TITLE_H, panel.bottom() - FOOTER_H);
        let split = panel.left() + GROUPS_W.min(width * 0.4);
        let groups = Rect::from_min_max(Pos2::new(panel.left(), top), Pos2::new(split, bottom));
        let rows = Rect::from_min_max(Pos2::new(split, top), Pos2::new(panel.right(), bottom));
        let visible = ((rows.height() / ROW_H).floor() as usize).max(1);
        Self {
            panel,
            groups,
            rows,
            visible,
        }
    }

    /// The `k`th row on screen of a column.
    pub(crate) fn row(column: Rect, k: usize) -> Rect {
        Rect::from_min_size(
            Pos2::new(column.left() + 6.0, column.top() + k as f32 * ROW_H),
            Vec2::new(column.width() - 12.0, ROW_H - 2.0),
        )
    }
}

/// A click in the list, taken in after the frame is drawn.
enum Click {
    Group(usize),
    Row { row: usize, double: bool },
}

/// The list's state while it is open.
pub struct ChannelListView {
    groups: ChannelGroups,
    cursor: Cursor,
    /// The first group row and the first channel row on screen.
    group_top: usize,
    row_top: usize,
    /// How many channel rows the last frame had room for: a page.
    page: usize,
    /// The selection moved by a key or a click: it is brought on screen.
    follow: bool,
    /// The list opened, or the group changed: the selection goes to the middle.
    center: bool,
}

impl ChannelListView {
    /// The list as it opens (spec §3.3): see [`Cursor::open`]. Starts nothing.
    pub fn new(groups: ChannelGroups, last: Option<&GroupKey>, playing: Option<i64>) -> Self {
        let cursor = Cursor::open(&groups, last, playing);
        Self {
            groups,
            cursor,
            group_top: 0,
            row_top: 0,
            page: 10,
            follow: true,
            center: true,
        }
    }

    pub fn groups(&self) -> &ChannelGroups {
        &self.groups
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// The group the selection is in: the list opens there next time.
    pub fn group_key(&self) -> Option<&GroupKey> {
        self.groups.key(self.cursor.group)
    }

    /// Groups made anew while the list is open (another copy of the list, a
    /// channel marked or cleared): the selection keeps its place
    /// ([`Cursor::follow`]).
    pub fn replace_groups(&mut self, groups: ChannelGroups) {
        self.cursor.follow(&self.groups, &groups);
        self.groups = groups;
        self.follow = true;
    }

    /// One frame of the list over the window: its keys, the wheel, the rows on
    /// screen, the clicks. What the user did, in the order they did it.
    pub fn show(&mut self, ctx: &egui::Context, data: &ListData) -> Vec<ListAction> {
        let layout = Layout::of(ctx.screen_rect());
        self.page = layout.visible;
        let mut actions = Vec::new();
        let closing = self.keys(ctx, data.playing, &mut actions);
        self.wheel(ctx, &layout);
        self.bring_on_screen(layout.visible);
        let mut clicked = None;
        egui::Area::new(egui::Id::new("__channel_list__"))
            // Its own click sense would make the frame a Tab stop that shows nothing.
            .sense(Sense::hover())
            .fixed_pos(layout.panel.min)
            .show(ctx, |ui| {
                let _ = ui.allocate_exact_size(layout.panel.size(), Sense::hover());
                clicked = self.paint(ui, &layout, data);
            });
        if let Some(click) = clicked.filter(|_| !closing) {
            self.click(click, data.playing, &mut actions);
        }
        actions
    }

    /// The keys of this frame, in the order they came. True when one closed the
    /// list: what comes after it is not the list's.
    fn keys(&mut self, ctx: &egui::Context, playing: Option<i64>, actions: &mut Vec<ListAction>) -> bool {
        let presses: Vec<(Key, bool)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat,
                        ..
                    } => Some((*key, *repeat)),
                    _ => None,
                })
                .collect()
        });
        let page = self.page as isize;
        for (key, repeat) in presses {
            let moved = match key {
                Key::ArrowUp => self.cursor.step(&self.groups, -1),
                Key::ArrowDown => self.cursor.step(&self.groups, 1),
                Key::PageUp => self.cursor.step(&self.groups, -page),
                Key::PageDown => self.cursor.step(&self.groups, page),
                Key::Home => self.cursor.to_end(&self.groups, false),
                Key::End => self.cursor.to_end(&self.groups, true),
                Key::ArrowLeft | Key::ArrowRight => {
                    let delta = if key == Key::ArrowLeft { -1 } else { 1 };
                    if self.cursor.step_group(&self.groups, delta, playing) {
                        self.center = true;
                        actions.push(ListAction::GroupChanged);
                    }
                    false
                }
                Key::Enter => {
                    actions.push(match self.cursor.channel(&self.groups) {
                        Some(c) => ListAction::Play {
                            stream_id: c.stream_id,
                            name: c.name.clone(),
                        },
                        None => ListAction::Close,
                    });
                    return true;
                }
                Key::Escape | Key::G => {
                    actions.push(ListAction::Close);
                    return true;
                }
                _ => false,
            };
            if moved {
                self.follow = true;
                if let Some(c) = self.cursor.channel(&self.groups) {
                    actions.push(ListAction::Preview {
                        stream_id: c.stream_id,
                        name: c.name.clone(),
                        repeat,
                    });
                }
            }
        }
        false
    }

    /// The wheel scrolls the column under the pointer by whole rows; the
    /// selection stays and nothing starts.
    fn wheel(&mut self, ctx: &egui::Context, layout: &Layout) {
        let (pointer, delta) = ctx.input(|i| (i.pointer.hover_pos(), i.raw_scroll_delta.y));
        let Some(at) = pointer.filter(|_| delta != 0.0) else {
            return;
        };
        let rows = (-delta / ROW_H).round() as isize;
        // A small turn (a touchpad's) is still a row.
        let rows = if rows == 0 { -(delta.signum() as isize) } else { rows };
        if layout.rows.contains(at) {
            let most = self.groups.rows(self.cursor.group).saturating_sub(layout.visible);
            self.row_top = self.row_top.saturating_add_signed(rows).min(most);
        } else if layout.groups.contains(at) {
            let most = self.groups.group_count().saturating_sub(layout.visible);
            self.group_top = self.group_top.saturating_add_signed(rows).min(most);
        }
    }

    /// The selection on screen after a key or a click moved it (in the middle
    /// when the list opened or the group changed), the selected group too. A
    /// turn of the wheel alone leaves the rows where it put them.
    fn bring_on_screen(&mut self, visible: usize) {
        let rows_most = self.groups.rows(self.cursor.group).saturating_sub(visible);
        let groups_most = self.groups.group_count().saturating_sub(visible);
        let (row, group) = (self.cursor.row, self.cursor.group);
        if self.center {
            self.row_top = row.saturating_sub(visible / 2);
            self.group_top = group.saturating_sub(visible / 2);
        } else if self.follow {
            if row < self.row_top {
                self.row_top = row;
            } else if row >= self.row_top + visible {
                self.row_top = row + 1 - visible;
            }
            if group < self.group_top {
                self.group_top = group;
            } else if group >= self.group_top + visible {
                self.group_top = group + 1 - visible;
            }
        }
        self.row_top = self.row_top.min(rows_most);
        self.group_top = self.group_top.min(groups_most);
        self.center = false;
        self.follow = false;
    }

    /// Draws the frame: the rows on screen only. A click on a group or a row.
    fn paint(&self, ui: &egui::Ui, layout: &Layout, data: &ListData) -> Option<Click> {
        let lang = data.lang;
        let p = ui.painter();
        p.rect_filled(layout.panel, 8.0, PANEL);
        p.text(
            Pos2::new(layout.panel.left() + MARGIN, layout.panel.top() + TITLE_H / 2.0),
            Align2::LEFT_CENTER,
            i18n::t(lang, T::ChannelListTitle),
            FontId::proportional(20.0),
            Color32::WHITE,
        );
        p.line_segment(
            [layout.groups.right_top(), layout.groups.right_bottom()],
            Stroke::new(1.0, Color32::from_white_alpha(30)),
        );
        let mut clicked = None;
        // The groups, with their counts.
        let dim = Color32::from_white_alpha(130);
        let end = (self.group_top + layout.visible).min(self.groups.group_count());
        for (k, g) in (self.group_top..end).enumerate() {
            let Some(key) = self.groups.key(g) else {
                break;
            };
            let row = Layout::row(layout.groups, k);
            let response = ui.interact(row, egui::Id::new(("__list_group__", g)), CLICK);
            // The open group has a fill of its own; the selected row's
            // background, with the accent bar, is the channel column's alone.
            if g == self.cursor.group {
                p.rect_filled(row, 4.0, GROUP_OPEN);
            } else if response.hovered() {
                p.rect_filled(row, 4.0, ROW_HOVER);
            }
            let count = p.layout_no_wrap(self.groups.rows(g).to_string(), FontId::proportional(13.0), dim);
            let count_w = count.size().x;
            let count_at = Pos2::new(row.right() - 8.0 - count_w, row.center().y - count.size().y / 2.0);
            p.galley(count_at, count, dim);
            let color = if *key == GroupKey::Defect { BROKEN } else { Color32::WHITE };
            let width = (row.width() - 26.0 - count_w).max(10.0);
            let galley = p.layout_job(single_line(group_label(lang, key), FontId::proportional(15.0), color, width));
            p.galley(Pos2::new(row.left() + 10.0, row.center().y - galley.size().y / 2.0), galley, color);
            if response.clicked() {
                clicked = Some(Click::Group(g));
            }
        }
        // The channels of the open group.
        let g = self.cursor.group;
        let rows = self.groups.rows(g);
        if rows == 0 {
            p.text(
                layout.rows.center_top() + Vec2::new(0.0, 40.0),
                Align2::CENTER_CENTER,
                i18n::t(lang, T::ListEmptyGroup),
                FontId::proportional(15.0),
                Color32::from_white_alpha(140),
            );
        }
        let defect_group = self.groups.key(g) == Some(&GroupKey::Defect);
        let end = (self.row_top + layout.visible).min(rows);
        for (k, r) in (self.row_top..end).enumerate() {
            let Some(channel) = self.groups.channel(g, r) else {
                break;
            };
            let row = Layout::row(layout.rows, k);
            let response = ui.interact(row, egui::Id::new(("__list_row__", g, r)), CLICK);
            // The selected row's background: painted here, once.
            if r == self.cursor.row {
                p.rect_filled(row, 4.0, ROW_SELECTED);
                p.rect_filled(Rect::from_min_size(row.min, Vec2::new(3.0, row.height())), 0.0, ACCENT);
            } else if response.hovered() {
                p.rect_filled(row, 4.0, ROW_HOVER);
            }
            let playing = data.playing == Some(channel.stream_id);
            if playing {
                play_mark(p, Pos2::new(row.left() + 16.0, row.center().y), PLAYING);
            }
            let mut right = row.right() - 8.0;
            if defect_group {
                if let Some(&failed) = data.marks.get(&channel.stream_id) {
                    let ago = failed_ago(lang, data.now_unix.saturating_sub(failed));
                    let galley = p.layout_no_wrap(ago, FontId::proportional(12.0), BROKEN);
                    right -= galley.size().x;
                    p.galley(Pos2::new(right, row.center().y - galley.size().y / 2.0), galley, BROKEN);
                    right -= 12.0;
                }
            }
            let color = if playing { PLAYING } else { Color32::WHITE };
            let x = row.left() + 32.0;
            let galley = p.layout_job(single_line(&channel.name, FontId::proportional(16.0), color, (right - x).max(10.0)));
            p.galley(Pos2::new(x, row.center().y - galley.size().y / 2.0), galley, color);
            if response.double_clicked() {
                clicked = Some(Click::Row { row: r, double: true });
            } else if response.clicked() {
                clicked = Some(Click::Row { row: r, double: false });
            }
        }
        // The keys, one line.
        let color = Color32::from_white_alpha(110);
        let footer = single_line(
            i18n::t(lang, T::ChannelListFooter),
            FontId::proportional(11.0),
            color,
            layout.panel.width() - 2.0 * MARGIN,
        );
        let galley = p.layout_job(footer);
        let y = layout.panel.bottom() - FOOTER_H / 2.0 - galley.size().y / 2.0;
        p.galley(Pos2::new(layout.panel.left() + MARGIN, y), galley, color);
        clicked
    }

    /// A click, taken in after the frame was drawn: a group opens; a channel is
    /// selected and starts, like one press; a double click's second click
    /// closes the list as Enter does (its first started the channel already).
    fn click(&mut self, click: Click, playing: Option<i64>, actions: &mut Vec<ListAction>) {
        match click {
            Click::Group(g) => {
                if self.cursor.select_group(&self.groups, g, playing) {
                    self.center = true;
                    actions.push(ListAction::GroupChanged);
                }
            }
            Click::Row { row, double } => {
                self.cursor.select_row(&self.groups, row);
                let Some(c) = self.cursor.channel(&self.groups) else {
                    return;
                };
                let (stream_id, name) = (c.stream_id, c.name.clone());
                actions.push(if double {
                    ListAction::Play { stream_id, name }
                } else {
                    ListAction::Preview {
                        stream_id,
                        name,
                        repeat: false,
                    }
                });
            }
        }
    }
}

/// What a group is called: the list's own name, or one of the fixed ones.
pub fn group_label(lang: Lang, key: &GroupKey) -> &str {
    match key {
        GroupKey::All => i18n::t(lang, T::ListAllChannels),
        GroupKey::Favorites => i18n::t(lang, T::FavoritesTitle),
        GroupKey::Named(name) => name,
        GroupKey::NoGroup => i18n::t(lang, T::ListNoGroup),
        GroupKey::Defect => i18n::t(lang, T::ListDefect),
    }
}

/// How long ago a channel failed, `secs` seconds back: just now, minutes,
/// hours, days.
pub fn failed_ago(lang: Lang, secs: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    match secs {
        s if s < MINUTE => i18n::t(lang, T::AgoJustNow).to_owned(),
        s if s < HOUR => i18n::tr(lang, T::AgoMinutes, &[&(s / MINUTE).to_string()]),
        s if s < 2 * HOUR => i18n::t(lang, T::AgoHour).to_owned(),
        s if s < DAY => i18n::tr(lang, T::AgoHours, &[&(s / HOUR).to_string()]),
        s if s < 2 * DAY => i18n::t(lang, T::AgoDay).to_owned(),
        s => i18n::tr(lang, T::AgoDays, &[&(s / DAY).to_string()]),
    }
}
```

Run `rustfmt --edition 2021 src/channel_list_ui.rs` once written: some lines above are longer than rustfmt keeps them.

- [ ] **Step 5: Run the tests to see them pass**

Run: `ct --lib -- channel_list_ui:: guide_ui::`
Expected: all pass, the guide's own tests too (its play mark keeps its colour).

- [ ] **Step 6: Format, lint, full suite**

- Run `fmt src/channel_list_ui.rs src/i18n.rs src/guide_ui.rs`. Expected: no output (`guide_ui.rs` is clean at the base, and stays so).
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes.

- [ ] **Step 7: Commit**

Message: `feat(rust): the channel list on screen: two columns, the rows on screen only, keys and mouse as actions`, followed by the two attribution lines.
Files: `rust/src/channel_list_ui.rs`, `rust/src/lib.rs`, `rust/src/guide_ui.rs`, `rust/src/i18n.rs`.

---

### Task 8: App: `g` opens the channel list on an M3U source

`g` opens the list on an M3U source (Xtream keeps its guide). While it is open the list has the keyboard, no zap toast shows, and its moves start channels through the app's one pacer. The groups follow the catalog, the marks and the favorites while it is open; a closed list holds nothing but its last group's key.

**Files:**
- Modify: `rust/src/app.rs`, `rust/src/i18n.rs`

**Interfaces:**
- Consumes:
  - Task 3: `TvApp::{plays, zap_to}`, `zap_pacer: ZapPacer<(i64, String, Option<usize>)>`, the tests' `m3u_sid`, `ring_url`;
  - Task 5: the tests' `start_fails`, `picture_comes`, `checks_answered`;
  - Task 6: `CatalogStore::channel_groups`, `GroupKey`, `ChannelGroups::{rows, find, channel, channel_count, group_count}`;
  - Task 7: `ChannelListView::{new, show, replace_groups, group_key, cursor, groups}`, `ListAction`, `ListData`, `channel_list_ui::Layout::{of, row}`;
  - existing in `app.rs`: `open_guide`, `close_guide`, `handle_keys` (its `g` spends the key before `open_guide`), `update_ui`, `zap_to`, `toggle_favorite_current`, `paint_favs`, `use_data_of`, `close_panels_under_settings`, `install_portal`; the tests' `Harness`, `m3u_app`, `loads`, `key_press`, `click_at`, `guide_catalog`, `f_pressed`, `shift_f_pressed`, `wait_until`, `list_server`, `LogCapture`.
- Produces (`app.rs`):
  ```rust
  // TvApp fields
  channel_list: Option<ChannelListView>,
  channel_list_group: Option<GroupKey>,
  channel_list_for: Option<(u64, u64, u64, u64)>,
  favorites_revision: u64,
  // TvApp methods
  fn open_channel_list(&mut self);
  fn close_channel_list(&mut self);
  fn refresh_channel_list(&mut self);
  fn paint_channel_list(&mut self, ctx: &egui::Context);
  fn apply_list_action(&mut self, action: ListAction);
  fn channel_list_key(&self) -> (u64, u64, u64, u64);
  fn favorite_ids(&self) -> HashSet<i64>;
  ```
  Log lines: `channel list: opened (<n> channels, <m> groups)`, `channel list: closed`. i18n: `NoGuideForSource` goes; `HelpOpenGuide` names the list.

- [ ] **Step 1: Write the failing tests**

In `rust/src/app.rs`'s `mod tests`:

1. Delete the test `g_on_an_m3u_source_says_there_is_no_guide_yet`: its toast is gone.
2. After the section `// ---- defect marks ----` (Task 5), add:

```rust
    // ---- the channel list ----

    /// Four channels in two groups, out of A-Z order in the file: D, B, C, A.
    const LISTED: &str = "#EXTM3U\n\
#EXTINF:-1 group-title=\"Nieuws\",Kanaal D\nhttps://d.example/d.m3u8\n\
#EXTINF:-1 group-title=\"Sport\",Kanaal B\nhttps://b.example/b.m3u8\n\
#EXTINF:-1 group-title=\"Nieuws;Sport\",Kanaal C\nhttps://c.example/c.m3u8\n\
#EXTINF:-1,Kanaal A\nhttps://a.example/a.m3u8\n";

    /// `g` as a keyboard makes it, and the frame after it drawn: egui keeps
    /// the list's new area invisible in its first frame.
    fn open_list(h: &mut Harness) {
        h.frame_with(vec![key_press(Key::G), egui::Event::Text("g".into())]);
        h.frame(&[]);
    }

    fn list(h: &Harness) -> &ChannelListView {
        h.app.channel_list.as_ref().expect("the channel list is open")
    }

    /// The name of the channel the list's selection is on.
    fn selected(h: &Harness) -> Option<String> {
        let view = list(h);
        view.cursor().channel(view.groups()).map(|c| c.name.clone())
    }

    /// The middle of the `k`th channel row on screen, in the tests' window.
    fn list_row(k: usize) -> egui::Pos2 {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0));
        let layout = crate::channel_list_ui::Layout::of(screen);
        crate::channel_list_ui::Layout::row(layout.rows, k).center()
    }

    #[tokio::test]
    async fn g_on_an_m3u_source_opens_the_channel_list_on_the_channel_that_plays() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        h.sent();
        let log = LogCapture::start();
        open_list(&mut h);
        assert_eq!(list(&h).group_key(), Some(&GroupKey::All));
        assert_eq!(selected(&h).as_deref(), Some("Kanaal C"));
        assert!(loads(h.sent()).is_empty(), "opening starts nothing");
        assert!(!h.app.guide_open);
        for text in [
            "Channel list",
            "All channels",
            "Nieuws",
            "Sport",
            "No group",
            "Kanaal A",
            "Kanaal D",
        ] {
            assert!(h.painted.iter().any(|t| t == text), "{text}: {:?}", h.painted);
        }
        let text = log.text();
        assert!(text.contains("channel list: opened (4 channels, 4 groups)"), "{text}");
    }

    #[tokio::test]
    async fn g_on_an_xtream_portal_is_still_the_guide() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        h.press(&[Key::G]);
        assert!(h.app.guide_open);
        assert!(h.app.channel_list.is_none());
    }

    #[tokio::test]
    async fn g_while_the_list_loads_says_so_and_opens_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (portal, _) = M3uPortal::from_text(LISTED).unwrap();
        let store = Arc::new(CatalogStore::new(Arc::new(portal)));
        let mut h = Harness::start_with(
            dir.path(),
            store,
            PortalState::Configured,
            None,
            None,
            Sources::default(),
            None,
        );
        open_list(&mut h);
        assert!(h.app.channel_list.is_none());
        let said = "catalog still loading - press g again in a moment";
        assert!(h.painted.iter().any(|t| t == said), "{:?}", h.painted);
    }

    #[tokio::test]
    async fn down_starts_the_next_channel_at_once_and_a_held_key_only_where_it_stops() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        h.app.zap_pacer = ZapPacer::new(Duration::from_millis(300));
        open_list(&mut h);
        h.sent();
        // Nothing plays: the list opens on Kanaal A. One press: B at once.
        h.frame(&[Key::ArrowDown]);
        assert_eq!(loads(h.sent()), [ring_url('b')]);
        // Held (the harness never lets a key go): the repeats only move on.
        h.frame(&[Key::ArrowDown]);
        h.frame(&[Key::ArrowDown]);
        assert!(loads(h.sent()).is_empty());
        assert_eq!(selected(&h).as_deref(), Some("Kanaal D"));
        std::thread::sleep(Duration::from_millis(350));
        h.frame(&[]);
        assert_eq!(loads(h.sent()), [ring_url('d')], "where it stopped");
        assert!(h.app.channel_list.is_some(), "behind the list");
    }

    #[tokio::test]
    async fn left_and_right_change_the_group_and_start_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        open_list(&mut h);
        h.sent();
        let named = |name: &str| GroupKey::Named(name.into());
        h.press(&[Key::ArrowRight]);
        assert_eq!(list(&h).group_key(), Some(&named("Nieuws")));
        assert_eq!(selected(&h).as_deref(), Some("Kanaal C"), "C is in Nieuws");
        h.press(&[Key::ArrowRight]);
        assert_eq!(list(&h).group_key(), Some(&named("Sport")));
        assert_eq!(selected(&h).as_deref(), Some("Kanaal C"));
        h.press(&[Key::ArrowRight]);
        assert_eq!(list(&h).group_key(), Some(&GroupKey::NoGroup));
        assert_eq!(selected(&h).as_deref(), Some("Kanaal A"), "C is not in it");
        h.press(&[Key::ArrowRight]);
        assert_eq!(list(&h).group_key(), Some(&GroupKey::NoGroup), "the last group");
        h.press(&[Key::ArrowLeft]);
        assert_eq!(list(&h).group_key(), Some(&named("Sport")));
        assert!(loads(h.sent()).is_empty(), "nothing started");
    }

    #[tokio::test]
    async fn enter_closes_the_list_and_plays_the_channel_now_even_while_it_waits() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        h.app.zap_pacer = ZapPacer::new(Duration::from_millis(300));
        open_list(&mut h);
        h.sent();
        h.frame(&[Key::ArrowDown]); // B, at once
        h.frame(&[Key::ArrowDown]); // C, waiting for the tempo
        h.press(&[Key::Enter]);
        assert!(h.app.channel_list.is_none(), "closed");
        assert_eq!(loads(h.sent()), [ring_url('b'), ring_url('c')]);
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal C"));
        std::thread::sleep(Duration::from_millis(350));
        h.frame(&[]);
        assert!(loads(h.sent()).is_empty(), "the waiting start is gone");
    }

    #[tokio::test]
    async fn enter_on_the_channel_that_plays_only_closes() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        open_list(&mut h);
        h.sent();
        h.press(&[Key::Enter]);
        assert!(h.app.channel_list.is_none());
        assert!(loads(h.sent()).is_empty());
    }

    #[tokio::test]
    async fn enter_on_a_channel_that_failed_tries_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        start_fails(&mut h);
        open_list(&mut h);
        h.sent();
        assert_eq!(selected(&h).as_deref(), Some("Kanaal C"));
        h.press(&[Key::Enter]);
        assert!(h.app.channel_list.is_none());
        assert_eq!(loads(h.sent()), [ring_url('c')], "tried again");
    }

    #[tokio::test]
    async fn esc_and_g_close_the_list_keep_what_plays_and_drop_a_waiting_start() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        h.app.zap_pacer = ZapPacer::new(Duration::from_millis(300));
        open_list(&mut h);
        h.sent();
        h.frame(&[Key::ArrowDown]); // B, at once
        h.frame(&[Key::ArrowDown]); // C, waiting
        h.press(&[Key::Escape]);
        assert!(h.app.channel_list.is_none());
        std::thread::sleep(Duration::from_millis(350));
        h.frame(&[]);
        assert_eq!(loads(h.sent()), [ring_url('b')], "C never started");
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal B"));
        open_list(&mut h);
        assert!(h.app.channel_list.is_some());
        h.frame_with(vec![key_press(Key::G), egui::Event::Text("g".into())]);
        assert!(h.app.channel_list.is_none(), "g closes it too");
    }

    #[tokio::test]
    async fn while_the_list_is_open_no_zap_toast_shows_but_a_stream_error_does() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        // Kanaal A's zap toast makes the toast's area before the list opens.
        h.app.zap_by_id(m3u_sid("Kanaal A"));
        open_list(&mut h);
        h.press(&[Key::ArrowDown]);
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal B"));
        assert!(
            !h.painted.iter().any(|t| t == "[TV] Kanaal B"),
            "the list's play mark shows what plays: {:?}",
            h.painted
        );
        start_fails(&mut h);
        let named = "stream error on Kanaal B - zap up or down to retry";
        assert!(h.painted.iter().any(|t| t == named), "{:?}", h.painted);
    }

    #[tokio::test]
    async fn after_the_list_up_and_down_zap_on_in_the_files_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        open_list(&mut h);
        h.press(&[Key::End]); // Kanaal D, the last A-Z
        h.press(&[Key::Escape]);
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal D"));
        h.sent();
        h.press(&[Key::ArrowDown]);
        assert_eq!(loads(h.sent()), [ring_url('b')], "after D in the file comes B");
    }

    #[tokio::test]
    async fn the_list_opens_in_the_group_it_had_open_last() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        open_list(&mut h);
        h.press(&[Key::ArrowRight, Key::ArrowRight]); // Nieuws, Sport
        h.press(&[Key::Escape]);
        open_list(&mut h);
        assert_eq!(list(&h).group_key(), Some(&GroupKey::Named("Sport".into())));
        assert_eq!(selected(&h).as_deref(), Some("Kanaal B"), "nothing plays: the first");
    }

    #[tokio::test]
    async fn a_click_on_a_channel_starts_it_and_the_list_stays_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        open_list(&mut h);
        h.sent();
        click_at(&mut h, list_row(2)); // Kanaal C
        assert_eq!(loads(h.sent()), [ring_url('c')]);
        assert!(h.app.channel_list.is_some());
    }

    #[tokio::test]
    async fn a_double_click_on_a_channel_plays_it_and_closes_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        open_list(&mut h);
        h.sent();
        click_at(&mut h, list_row(3)); // Kanaal D
        click_at(&mut h, list_row(3));
        assert!(h.app.channel_list.is_none(), "closed");
        assert_eq!(
            loads(h.sent()),
            [ring_url('d')],
            "started by the first click, once"
        );
    }

    #[tokio::test]
    async fn a_channel_marked_while_the_list_is_open_moves_to_broken_channels() {
        // B's address answers 404, half a second late: the mark lands while the
        // list is open.
        let server = list_server(404, "", Duration::from_millis(500)).await;
        let dir = tempfile::tempdir().unwrap();
        let text = LISTED.replace(
            "https://b.example/b.m3u8",
            &format!("{}/b.m3u8", server.uri()),
        );
        let mut h = m3u_app(dir.path(), &text);
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        open_list(&mut h);
        h.press(&[Key::ArrowDown]); // B starts
        start_fails(&mut h); // and dies before its first picture
        h.press(&[Key::ArrowDown]); // C starts
        picture_comes(&mut h); // and plays: B's address is checked
        // The list does not wait for the check.
        h.press(&[Key::ArrowRight, Key::ArrowLeft]);
        assert_eq!(selected(&h).as_deref(), Some("Kanaal C"));
        assert!(
            list(&h).groups().find(&GroupKey::Defect).is_none(),
            "not marked before the answer"
        );
        checks_answered(&mut h).await;
        h.frame(&[]);
        let view = list(&h);
        assert_eq!(view.group_key(), Some(&GroupKey::All));
        assert_eq!(selected(&h).as_deref(), Some("Kanaal C"), "the selection stays");
        assert_eq!(view.groups().rows(0), 3, "B left All channels");
        let broken = view.groups().find(&GroupKey::Defect).expect("Broken channels");
        let first = view.groups().channel(broken, 0).map(|c| c.name.as_str());
        assert_eq!(first, Some("Kanaal B"));
        assert!(h.painted.iter().any(|t| t == "Broken channels"), "{:?}", h.painted);
    }

    #[tokio::test]
    async fn the_list_keeps_its_place_when_a_fresh_copy_of_the_list_lands() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, LISTED).unwrap();
        let portal = M3uPortal::new(1, &file.display().to_string(), dir.path().join("m3u-1.m3u"));
        let store = Arc::new(CatalogStore::new(Arc::new(portal)));
        store.spawn_fetch();
        wait_until("the list", || store.is_loaded()).await;
        let mut h = Harness::start_with(
            dir.path(),
            store.clone(),
            PortalState::Configured,
            None,
            None,
            Sources::default(),
            None,
        );
        h.app.zap_by_id(m3u_sid("Kanaal C"));
        open_list(&mut h);
        assert_eq!(list(&h).cursor().row, 2);
        // The fresh copy puts a channel before all the others, A-Z.
        let fresh = format!(
            "#EXTM3U\n#EXTINF:-1,Kanaal 0\nhttps://z.example/0.m3u8\n{}",
            &LISTED["#EXTM3U\n".len()..]
        );
        std::fs::write(&file, fresh).unwrap();
        store.spawn_fetch();
        wait_until("the fresh copy", || store.revision() == 2).await;
        h.frame(&[]);
        assert_eq!(selected(&h).as_deref(), Some("Kanaal C"), "the same channel");
        assert_eq!(list(&h).cursor().row, 3);
        assert_eq!(list(&h).groups().rows(0), 5);
    }

    #[tokio::test]
    async fn while_the_list_is_open_other_hotkeys_do_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        open_list(&mut h);
        h.frame_with(f_pressed());
        h.frame_holding(egui::Modifiers::SHIFT, shift_f_pressed());
        h.frame_with(vec![key_press(Key::O), egui::Event::Text("o".into())]);
        h.frame_with(vec![egui::Event::Text("?".into())]);
        h.frame_with(vec![egui::Event::Text("*".into())]);
        h.frame(&[Key::N, Key::D, Key::E]);
        h.frame(&[]);
        assert!(h.app.channel_list.is_some(), "the list stays open");
        assert!(!h.app.show_search && !h.app.show_favs && !h.app.show_help);
        assert!(h.app.settings_ui.is_none());
        assert!(!h.app.show_debug && !h.app.show_epg_strip && !h.app.news_fetching);
        assert_eq!(h.app.favorites.iter().count(), 0);
    }

    #[tokio::test]
    async fn opening_the_settings_closes_the_channel_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), LISTED);
        open_list(&mut h);
        h.app.open_settings();
        assert!(h.app.channel_list.is_none());
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --lib app::tests::`
Expected: compile errors: `TvApp` has no field `channel_list`.

- [ ] **Step 3: The texts**

In `rust/src/i18n.rs`:
- Delete the line `NoGuideForSource => […],`.
- `HelpOpenGuide` becomes:

```rust
    HelpOpenGuide => ["gids openen (M3U: zenderlijst)", "open guide (M3U: channel list)", "Programm öffnen (M3U: Senderliste)"],
```

- [ ] **Step 4: The list in the app**

In `rust/src/app.rs`:

1. Imports, inside the import block: `use crate::channel_list::GroupKey;`, `use crate::channel_list_ui::{ChannelListView, ListAction, ListData};` and `use std::collections::HashSet;`.

2. Fields, after `guide_view`:

```rust
    /// The channel list of an M3U source (`g`), while it is open (spec
    /// 2026-10-15 §3). Closed, it is `None` and costs nothing.
    channel_list: Option<ChannelListView>,
    /// The group the list had open when it closed last, this session: it opens
    /// there again.
    channel_list_group: Option<GroupKey>,
    /// What the open list's groups were made for: the catalog's generation and
    /// revision, the marks' revision, the favorites' revision.
    channel_list_for: Option<(u64, u64, u64, u64)>,
    /// Counts every change of the favorites, for the list's Favorites group.
    favorites_revision: u64,
```

and in `TvApp::start`'s struct literal, after `guide_view: …,`: `channel_list: None, channel_list_group: None, channel_list_for: None, favorites_revision: 0,` (one per line).

3. The favorites count their changes:
- in `toggle_favorite_current`, after `self.favorites.toggle(sid, &name);`: `self.favorites_revision += 1;`
- in `paint_favs`, in `if let Some(id) = remove_id { … }`, after `self.favorites.remove(id);`: `self.favorites_revision += 1;`
- in `use_data_of`, after the line that loads `self.favorites`: `self.favorites_revision += 1;`

4. In `zap_to`, the line near its end `self.set_toast(format!("[TV] {}", name));` (not the guard Task 3 gave it) becomes:

```rust
        // While the channel list is open its play mark shows what plays (spec §3.5).
        if self.channel_list.is_none() {
            self.set_toast(format!("[TV] {}", name));
        }
```

5. `open_guide` begins:

```rust
    fn open_guide(&mut self) {
        // An M3U source has no guide yet: `g` opens its channel list.
        if self.catalog.kind() == SourceKind::M3u {
            self.open_channel_list();
            return;
        }
```

(the rest of `open_guide` as it is; the toast `NoGuideForSource` goes with its old branch).

6. After `close_guide`, add:

```rust
    // ------------------------------------------------------------------
    // Channel list (`g` on an M3U source): channel_list_ui.rs draws it and
    // hands back what the user did.
    // ------------------------------------------------------------------

    /// What the open list's groups depend on. Read before the groups, so a
    /// change that lands in between is noticed next frame.
    fn channel_list_key(&self) -> (u64, u64, u64, u64) {
        (
            self.catalog_generation,
            self.catalog.revision(),
            self.catalog.defect_revision(),
            self.favorites_revision,
        )
    }

    /// The source's favorites, for the list's Favorites group.
    fn favorite_ids(&self) -> HashSet<i64> {
        self.favorites.iter().map(|e| e.stream_id).collect()
    }

    /// `g` on an M3U source (spec §3.1, §3.3): the channel list, in the group
    /// open last this session, on the channel that plays. Opening starts
    /// nothing. While the catalog loads, the guide's word for that.
    fn open_channel_list(&mut self) {
        if !self.catalog.is_loaded() {
            self.set_toast(self.t(T::GuideCatalogLoading));
            return;
        }
        let key = self.channel_list_key();
        let Some(groups) = self.catalog.channel_groups(&self.favorite_ids()) else {
            return;
        };
        tracing::info!(
            "channel list: opened ({} channels, {} groups)",
            groups.channel_count(),
            groups.group_count()
        );
        // Its rows take clicks: the two never show at once.
        self.show_favs = false;
        let last = self.channel_list_group.as_ref();
        let view = ChannelListView::new(groups, last, self.current_stream_id);
        self.channel_list = Some(view);
        self.channel_list_for = Some(key);
    }

    /// Closes the list. Only which group it had open stays.
    fn close_channel_list(&mut self) {
        if let Some(view) = self.channel_list.take() {
            self.channel_list_group = view.group_key().cloned();
            self.channel_list_for = None;
            tracing::info!("channel list: closed");
        }
    }

    /// The open list's groups made anew when the catalog (a fresh copy of the
    /// list, F5), the marks or the favorites changed; the selection keeps its
    /// place. Four comparisons per frame otherwise.
    fn refresh_channel_list(&mut self) {
        if self.channel_list.is_none() {
            return;
        }
        if self.catalog.kind() != SourceKind::M3u {
            self.close_channel_list();
            return;
        }
        let key = self.channel_list_key();
        if self.channel_list_for == Some(key) {
            return;
        }
        let Some(groups) = self.catalog.channel_groups(&self.favorite_ids()) else {
            self.close_channel_list();
            return;
        };
        if let Some(view) = self.channel_list.as_mut() {
            view.replace_groups(groups);
        }
        self.channel_list_for = Some(key);
    }

    /// The open list over the picture, and what the user did in it. Nothing at
    /// all while it is closed.
    fn paint_channel_list(&mut self, ctx: &egui::Context) {
        if self.channel_list.is_none() {
            return;
        }
        self.refresh_channel_list();
        let marks = self.catalog.defect_marks();
        let data = ListData {
            lang: self.lang,
            playing: self.current_stream_id,
            marks: &marks,
            now_unix: chrono::Utc::now().timestamp(),
        };
        let Some(view) = self.channel_list.as_mut() else {
            return;
        };
        let actions = view.show(ctx, &data);
        for action in actions {
            // An action that closed the list ends what this frame did in it.
            if self.channel_list.is_none() {
                break;
            }
            self.apply_list_action(action);
        }
    }

    /// What the list's keys and clicks mean (spec §3.4).
    fn apply_list_action(&mut self, action: ListAction) {
        match action {
            ListAction::Preview {
                stream_id,
                name,
                repeat,
            } => {
                // The zap tempo: a lone press starts at once; quick presses and a
                // held key only move the selection, and the channel the user stops
                // on starts. Not the one that plays: it would only start over.
                let target = (stream_id, name, None);
                if let Some((sid, name, idx)) = self.zap_pacer.press(Instant::now(), target, repeat) {
                    if !self.plays(sid) {
                        self.zap_to(sid, &name, idx);
                    }
                }
            }
            ListAction::Play { stream_id, name } => {
                // Enter or a double click: closed, and the channel plays now,
                // also when it still waited for the tempo.
                self.close_channel_list();
                self.zap_pacer.cancel();
                if !self.plays(stream_id) {
                    self.zap_to(stream_id, &name, None);
                }
            }
            // The selection left the channel that waited: nothing starts.
            ListAction::GroupChanged => self.zap_pacer.cancel(),
            ListAction::Close => {
                self.zap_pacer.cancel();
                self.close_channel_list();
            }
        }
    }
```

7. In `update_ui`:
- The keyboard: `} else if !self.guide_open || self.show_search {` becomes `} else if self.channel_list.is_none() && (!self.guide_open || self.show_search) {`, and the comment above the block gains: "The channel list reads its own keys in paint_channel_list, as the guide does."
- In the video branch, after `self.paint_debug_hud(ctx);`, add `self.paint_channel_list(ctx);` (last of the overlays: on top of them; the toast is drawn after it).

8. In `close_panels_under_settings`, add `self.close_channel_list();`.

9. In `install_portal`, after the marks' reset (Task 5), add:

```rust
        // Another source's groups mean nothing here.
        self.close_channel_list();
        self.channel_list_group = None;
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `ct --lib app::tests::`
Expected: all pass. `an_m3u_zap_asks_for_no_epg_and_the_guide_never_starts_fetching` presses `g` on an M3U source: the list opens now, and the guide still asks for nothing.

- [ ] **Step 6: Format, lint, full suite**

- Run `fmt src/i18n.rs`. Expected: no output.
- Check the hunk count of `app.rs`: 7, as at the baseline (Task 1).
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes. Report the total.

- [ ] **Step 7: Commit**

Message: `feat(rust): g on an M3U source opens the channel list: browse starts the channel behind it at the zap tempo, Enter plays, Esc keeps`, followed by the two attribution lines.
Files: `rust/src/app.rs`, `rust/src/i18n.rs`.

---

### Task 9: End-to-end: `M-list` and `M-defect`

Two scenarios in group M, against the mock's M3U list (its file order: M3U 01..18, M3U UA, M3U DOOD, M3U REF; all in group "Test"). `M-list` replaces `M-guide`, whose toast is gone. First the mock serves M3U DOOD under `localhost`, so that ruling 28b lets the check ask its address.

**The implementer writes the scripts and runs `-DryRun` only.** The controller asks the user before any run that opens windows (Task 10, last step).

**Files:**
- Modify: `rust/e2e/mock_portal.py`, `rust/e2e/regression.ps1`, `rust/e2e/run-scenario.ps1`, `rust/e2e/README.md`

**Interfaces:**
- Consumes the log lines of Tasks 3, 5 and 8: `zap: -> <name> (stream_id=<id>, idx=<idx>, prev=<prev>)`, `m3u: <name> (stream_id <id>) failed before its first picture`, `… marked defect (<what the check got>)`, `channel list: opened (<n> channels, <m> groups)`, `channel list: closed`; the debug line `toast: …`; and the marks file `bad_streams.json` in the config folder. The mock's `/m3uplay/dead.m3u8` answers the check 404 too, so M3U DOOD is still marked: from Step 1 on its address is under `localhost`, and M3U UA, which confirms it, plays from `127.0.0.1`, another server (ruling 28b, Task 4's `same_server`).
- Produces: `regression.ps1 -Group M` with `M-list` and `M-defect`; the mock's M3U DOOD under `localhost`.

- [ ] **Step 1: The mock serves M3U DOOD under `localhost`**

Every channel of the mock's list is on `127.0.0.1:18080`, and a check never asks the server of what plays (ruling 28b). M3U UA's picture would confirm M3U DOOD, its address would never be asked, and M-defect would mark nothing. `localhost` is another server by its host, and it still reaches the mock.

In `rust/e2e/mock_portal.py`:

1. In `m3u_list()`, M3U DOOD's line

```python
    lines += ['#EXTINF:-1 group-title="Test",M3U DOOD', f"http://127.0.0.1:{PORT}/m3uplay/dead.m3u8"]
```

becomes

```python
    # Under localhost, every other channel under 127.0.0.1: see MOCK_M3U above.
    lines += ['#EXTINF:-1 group-title="Test",M3U DOOD', f"http://localhost:{PORT}/m3uplay/dead.m3u8"]
```

2. In the docstring, at the end of the `MOCK_M3U=1` entry (after `/m3uplay/none/... are not playable: they answer 404 first.)`), add these lines, in the entry's own indent:

```
                     M3U DOOD's address is under localhost, every other one under
                     127.0.0.1: the check before a defect mark never asks the server of
                     the stream that plays, and localhost is another server by its host
                     that still reaches this mock. The mock listens on 127.0.0.1 only;
                     Windows resolves localhost to ::1 and then 127.0.0.1, and the ::1
                     attempt is refused, so mpv and the check go on to 127.0.0.1.
```

From `C:\Dropbox\Sources\tv\rust`, check that the file still parses (this writes nothing): `py -3 -c "import ast, sys; ast.parse(open(sys.argv[1], encoding='utf-8').read())" e2e/mock_portal.py`. Expected: no output.

M-dead's patterns (`zap: -> M3U DOOD`, `toast: streamfout op M3U DOOD`) name no host, and no other pattern in `regression.ps1`, the e2e README or this plan names DOOD's address: they all hold.

- [ ] **Step 2: The scenarios**

In `rust/e2e/regression.ps1`'s `Group-M`, replace the scenario `M-guide` and its comment line with:

```powershell
    # g on an M3U source: the channel list, A-Z (M3U 01 first). DOWN starts M3U 02 at once, with no zap toast while
    # the list is open; ENTER closes it, and M3U 02 plays on. Its last channel would be resumed on a re-run, so its
    # -Before makes the folder anew.
    $dl = IniDir 'm-list' $onlyList
    Scn 'M-list' @{ MockEnv = $M; NoPortal = $true; ConfigDir = $dl; Seconds = 14; Keys = 'SLEEP:1500;g;SLEEP:800;DOWN;SLEEP:2500;ENTER;SLEEP:1500'; Expect = 'channel list: opened \(21 channels, 2 groups\)', 'zap: -> M3U 02', 'loadfile http://127\.0\.0\.1:18080/m3uplay/2\.m3u8', 'channel list: closed'; Forbid = 'toast: \[TV\] M3U 02', 'guide: opened', 'zap: -> M3U (0[13-9]|1\d|UA|DOOD|REF)' } -Before { $null = IniDir 'm-list' $onlyList }
```

and after the scenario `M-dead`, add:

```powershell
    # A dead channel, then one that plays: the dead one is confirmed, the check finds its address 404 too, and it is
    # marked (bad_streams.json holds its id and time); zapping skips it. In the mock's file order M3U DOOD lies
    # between M3U UA and M3U REF, so DOWN from UA lands on REF. DOOD is under localhost and UA under 127.0.0.1: two
    # servers, so the check asks DOOD's address (it never asks the server of what plays).
    $dd = IniDir 'm-defect' $onlyList
    Scn 'M-defect' @{ MockEnv = $M; NoPortal = $true; ConfigDir = $dd; Seconds = 26; Keys = 'SLEEP:1500;f;SLEEP:400;TEXT:m3u dood;SLEEP:800;ENTER;SLEEP:4000;f;SLEEP:400;TEXT:m3u ua;SLEEP:800;ENTER;SLEEP:4000;DOWN;SLEEP:3000'; Expect = 'm3u: M3U DOOD \(stream_id \d+\) failed before its first picture', 'm3u: M3U DOOD \(stream_id \d+\) marked defect \(http 404\)', 'zap: -> M3U REF \(stream_id=\d+, idx=Some\(19\), prev=Some\("M3U UA"\)\)'; Forbid = 'zap: -> M3U DOOD \(stream_id=\d+, idx=[^,]*, prev=Some\("M3U UA"\)\)' } {
        (Get-Content -Raw (Join-Path $dd 'bad_streams.json')) -match '^\{"version":1,"marks":\{"\d+":\d+\}\}$'
    } -Before { $null = IniDir 'm-defect' $onlyList }
```

In the header's group list, `M  M3U sources (MOCK_M3U)` becomes `M  M3U sources and the channel list (MOCK_M3U)`.

In the comment above `function Group-M`, "M-zap, M-add, M-switch and M-getphp change what is in their folder" becomes "M-zap, M-add, M-switch, M-getphp, M-list and M-defect change what is in their folder".

- [ ] **Step 3: The harness's view of the log**

In `rust/e2e/run-scenario.ps1`, add `channel list: |` to the alternatives of the app-log filter that prints the log lines (after `guide: opened|`). It only changes what the output shows; `-Expect` and `-Forbid` read every line already.

- [ ] **Step 4: The README**

In `rust/e2e/README.md`, where it describes `MOCK_M3U=1`'s list, the sentence `"M3U DOOD" answers 404.` becomes:

> "M3U DOOD" answers 404; its address is under `localhost`, every other one under `127.0.0.1` (see `M-defect`).

In "## Regression set", after the sentence that ends "run the two together.", add:

> `M-list` opens the channel list with `g` and starts M3U 02 from it. `M-defect` makes M3U DOOD fail, plays M3U UA (which confirms DOOD; DOOD is under `localhost` and UA under `127.0.0.1`, two servers, so the check before the mark asks DOOD's address, finds `/m3uplay/dead.m3u8` 404 and marks it) and zaps down past DOOD to M3U REF; its check reads `bad_streams.json` in its folder, and its `-Before` makes the folder anew for a re-run, as the marks change it.

In the sentence after it, "`M-zap`, `M-add`, `M-switch` and `M-getphp` change what is in their folder" becomes "`M-zap`, `M-add`, `M-switch`, `M-getphp`, `M-list` and `M-defect` change what is in their folder".

- [ ] **Step 5: Dry run**

Run (from `C:\Dropbox\Sources\tv\rust`): `powershell -NoProfile -ExecutionPolicy Bypass -File e2e\regression.ps1 -Group M -DryRun`
Expected: `# M-list` and `# M-defect   (+ a check of the files afterwards)`, each followed by its `run-scenario.ps1` command; no `# M-guide`; no "has no parameter" error; one `# M-` line more than before. Opens no window.

Run: `powershell -NoProfile -ExecutionPolicy Bypass -File e2e\regression.ps1 -Group R,B,C,N,X,G -DryRun | Select-String '^# ' | Measure-Object`
Expected: the same count as before this task (those groups are unchanged).

- [ ] **Step 6: Commit**

Message: `test(e2e): M-list and M-defect: the channel list on g, a dead channel marked and skipped; the mock serves M3U DOOD under localhost`, followed by the two attribution lines.
Files: `rust/e2e/mock_portal.py`, `rust/e2e/regression.ps1`, `rust/e2e/run-scenario.ps1`, `rust/e2e/README.md`.

---

### Task 10: Docs, the release measurement, full verification

**Files:**
- Modify: `CLAUDE.md`, `rust/README.md`, `docs/VALIDATION.md`, `rust/build-zip.ps1`

**Interfaces:**
- Consumes everything above. The M3U branch's last commits rewrote parts of these docs: find each place by the bullet or heading named below, and keep its meaning if the wording differs.

- [ ] **Step 1: CLAUDE.md**

1. In the `catalog.rs` bullet, add at its end:

   > Up/down zap through a `ZapRing` (`CatalogStore::zap_ring`): positions in the catalog's one shared channel list (`Arc<Vec<LiveChannel>>`), worked out once per catalog, lock and marks, so a press copies nothing (`zap_channels()` is a copy, for tests). The catalog also holds an M3U source's defect marks (`set_defect`): the ring and the channel list's groups leave them out, the search and the favorites name them.

2. After the `zap.rs` bullet, add:

   > - **`bad_streams.rs`** — defect marks of an M3U source. A channel whose start ends without a first picture (an mpv error, a refused load, the reload budget, the stall watchdog; `TvApp::note_start_failure`) is a suspect; another channel's first picture within 2 minutes confirms it (`note_first_picture`), else the suspicion lapses. A confirmed suspect is marked only when the check of its address (`stream_check.rs`) finds it dead too (`drain_stream_checks`), so a Wi-Fi drop of any length marks no good channel. Its own picture clears it. A channel that has played since it was picked is never a suspect (`current_has_played`, the flag the M3U frame drop reads too). Marks last 7 days, per source in `bad_streams.json` / `bad_streams-<id>.json` (stream ids and unix times only), read when the source's catalog loads (`refresh_defects`); app.rs hands every change to the catalog.
   > - **`stream_check.rs`** — the check before a defect mark: one GET of a confirmed suspect's address with the channel's own User-Agent and Referer (libmpv's `libmpv` when it names none), as mpv asks it (no certificate check, header values as bytes), 5 s for the connection and the headers, the status only. 400 or more, no connection or no answer in time marks it; 2xx/3xx drops the suspicion; an address the check cannot ask is not marked. It never asks the server of the stream that plays (`same_server`: scheme, host, port): on a paid list one account serves every channel and its portal allows one session, so such a suspect is not marked. At most one check per channel and 4 at once (`StreamChecker`), on the tokio runtime with a client built on the first check; the answers come through a slot `update_ui` drains. A switch or save of the source drops the running checks. The address never reaches the log (`Check` has no `Debug`). This is the only network the defect marks use.
   > - **`channel_list.rs` / `channel_list_ui.rs`** — `g` on an M3U source: the channel list over the running picture. `channel_list.rs` builds the groups (`ChannelGroups::build`: All channels, Favorites, the list's `group-title`s A–Z by `natural_cmp`, No group, Defect channels) as positions in the catalog's channels (`CatalogStore::channel_groups`), and keeps the selection (`Cursor`); `channel_list_ui.rs` draws the rows on screen only and returns `ListAction`s. A move starts the channel behind the list through the app's one `ZapPacer`; Enter closes and plays, Esc/g close. The groups are built when the list opens and again while it is open when the catalog, the marks or the favorites change; a closed list holds nothing.

3. In the `zap.rs` bullet, add: "The channel list's moves are presses of the same pacer. A start that comes due for the channel that plays loads nothing (`TvApp::plays`)."

4. In the `guide/` / `guide_ui.rs` bullet, the sentence that says an M3U source has no guide and `g` says so becomes: "On an M3U source `g` opens the channel list (`channel_list_ui.rs`) instead."

5. In "Gotchas worth knowing":
   - At the end of the gotcha "The catalog is the only adult filter", add: "It is the only defect filter too: zapping, the channel list and the search read `CatalogStore::{zap_ring, channel_groups, is_defect}`, never a copy of the marks of their own."
   - The gotcha "A new M3U channel never shows the picture of the one before it": the M3U branch's last commits rewrote it, so edit it as it reads at this branch's base, keeping its words where they still hold. Afterwards it names `app.rs::stop_failed_start` (where it named `stop_for_refused_load`) and the arm of `Event::EndOfFile` for an error or an eof that the reloads did not get past; it says exactly which of them reads which flag (the `EndOfFile` arm reads `stream_starting_since` and `current_has_played`; `stop_failed_start` reads `stream_starting_since` only, for a refused load whether the channel played or not, and `check_stall` calls it only for a start that has not played); and among the starts that leave the screen black it counts one the stall watchdog gives up.
   - Add:

   > - **An M3U start the stall watchdog would reload is given up instead.** A channel of an M3U list that has not played since it was picked (`current_has_played`, which `check_stall` asks) stops with "streamfout op …" when the watchdog fires (`stop_failed_start`, as for a load mpv refuses; that function itself asks only `stream_starting_since`) and becomes a suspect; a reload would only start the 25 s wait over. One that has played is reloaded as before, and Xtream is unchanged.

- [ ] **Step 2: rust/README.md**

In the list under "## Where the app keeps its data", after the item on favorites, presets and the last channel per source, add:

```markdown
- Defect marks of an M3U source (channels that did not start, see `g`'s
  channel list): `bad_streams-<id>.json` in the config folder
  (`bad_streams.json` for a list an older ini has as source 1). They hold
  stream ids and times only, last 7 days, and go with their source. A channel
  is marked only when a check of its address finds it dead too, and that check
  never asks the server of the channel that plays: on a paid list one account
  serves every channel, and its portal allows one session.
```

- [ ] **Step 3: docs/VALIDATION.md**

In "## Bronnen (M3U)", the item that says `g` says "voor deze bron is er nog geen gids" now says that `g` opens the channel list (see Zenderlijst). After that section (else after "## Guide (g)"), add:

```markdown
## Zenderlijst (M3U, g)
- [ ] On an M3U source `g` opens the channel list over the picture, which keeps playing: groups on the left (Alle zenders, Favorieten when the source has favorites in the list, the list's groups A–Z, Zonder groep, Defecte zenders when channels are marked), each with its count; the group's channels A–Z on the right ("Sport 2" before "Sport 10"); a ▶ on the channel that plays; long names end in "…"; one line of keys at the bottom
- [ ] The list opens in the group used last this session (the first time: Alle zenders), on the channel that plays when that group holds it; opening starts nothing
- [ ] Up/Down: one press starts the channel behind the list at once; quick presses or a held key only move the selection, and the channel starts where you stop; at the first and the last row the selection stops; PgUp/PgDn a page, Home/End the ends, at the same tempo
- [ ] Left/Right: the group before or after, nothing starts. Enter: the list closes and the chosen channel plays (at once, also when it waited). Esc or g: the list closes, what plays goes on, a waiting start is dropped
- [ ] Mouse: a click on a group opens it; a click on a channel starts it; a double click starts it and closes the list; the wheel scrolls without starting anything
- [ ] While the list is open no "[TV] …" toast shows, but "streamfout op …" does; f, o, ?, n and * do nothing
- [ ] After closing, Up/Down zap on from the channel that plays, in the M3U file's order
- [ ] On an Xtream portal `g` is the guide, as before
- [ ] A dead channel, then within 2 minutes one that plays from another server: after the check of its address (a second or so) the dead one shows in "Defecte zenders" with how long ago it failed, zapping skips it, `f` and Shift+F show it with "defect"; the log says `m3u: <naam> (stream_id <id>) failed before its first picture` and `… marked defect (http 404)` (or `connect failed`, `timeout`), never an address; `bad_streams-<id>.json` (or `bad_streams.json` for a list that is source 1) in `%APPDATA%\BongoPlayer\config\` holds its id and time, no name and no address
- [ ] A dead channel, then within 2 minutes one that plays from the same server (a paid list: one account for every channel): the dead one is not marked and its server is not asked; the log says `… not checked (the server of what plays), not marked`
- [ ] Pull the network cable, zap through a few channels (they fail), wait more than 2 minutes, plug it back and zap: nothing is marked
- [ ] Pull the cable for less than 2 minutes, zap through a few good channels (they fail), plug it back and zap to one that plays: none of them is marked; the log says `… answers again, not marked` for each
- [ ] A marked channel played by a preset, the search, a favorite or the list plays; once it shows a picture its mark is gone (`… plays again, mark cleared`); while it plays, Up/Down go on from its place in the file
- [ ] Quit and start again: the marks are still there; a mark older than 7 days is gone after the start (`m3u: <n> defect marks expired`)
- [ ] A channel that never shows a picture and never errors: after about 25 s "streamfout op …", no "herverbinden"
- [ ] Delete the source in Bronnen: its bad_streams file goes too
- [ ] On iptv-org's list (about 11,000 channels) the list opens without a visible pause, and zapping is as fast as before
```

- [ ] **Step 4: The zip's README**

In `rust/build-zip.ps1`'s README text, the key line of `g` becomes:

```
  g                                  TV guide (Xtream) / channel list (M3U)
```

- [ ] **Step 5: Full verification**

- Run `ct`. Expected: all pass. Report the total and how many are new since Task 1's baseline.
- Run `cc`. Expected: the 2 old warnings only.
- Run `fmt src/bad_streams.rs src/stream_check.rs src/channel_list.rs src/channel_list_ui.rs src/storage.rs src/i18n.rs tests/test_bad_streams.rs tests/test_stream_check.rs tests/test_zap_ring.rs tests/test_channel_list.rs tests/test_storage.rs tests/test_mpv_smoke.rs tests/test_catalog_m3u.rs`. Expected: no output.
- Run `fmt src/guide_ui.rs`. Expected: no output, as at the baseline.
- The hunk counts of `app.rs` and `catalog.rs`: 7 and 3, as at Task 1's baseline.
- Run `bash rust/e2e/test-check-staged.sh`. Expected: every case `ok`.

- [ ] **Step 6: The grouping and a zap press in a release build**

Run: `cd /c/Dropbox/Sources/tv/rust && CARGO_TARGET_DIR='C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt' cargo test -j 4 --release --test test_channel_list eleven_thousand -- --nocapture`
Expected: it passes and prints `grouping: 11000 channels, <n> groups, <t> ms, <k> KB kept`. The spec's budget is 20 ms and under 1 MB. Write the line in your report. Over 20 ms: report it and change nothing; the controller decides.

Run: `cd /c/Dropbox/Sources/tv/rust && CARGO_TARGET_DIR='C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt' cargo test -j 4 --release --test test_catalog_m3u an_eleven_thousand -- --ignored --nocapture`
Expected: it passes and prints three lines, `guide_channels_of: …`, `zap_channels: …` and `zap_ring (a press): …` (first, best and mean, in ms). Write the three lines in your report: `zap_ring` is what a press costs now, `zap_channels` the copy a press made before.

- [ ] **Step 7: The release build**

Run without `CARGO_TARGET_DIR`: `cd /c/Dropbox/Sources/tv/rust && cargo build --release -j 4`. Then `--selftest` through `Start-Process -Wait -RedirectStandardOutput`; it must say it passed.
- If the exe is locked (the user runs it), say so and do not loop.
- Never open, print or touch `rust\target\release\BongoPlayer.ini`.

- [ ] **Step 8: The 0.6.1 baseline for the controller**

```powershell
$b = Join-Path $env:TEMP 'bongo-baseline-0.6.1'
if (-not (Test-Path $b)) { Expand-Archive 'C:\Dropbox\Sources\tv\rust\dist\BongoPlayer-v0.6.1.zip' -DestinationPath $b }
$exe = Get-ChildItem $b -Recurse -Filter BongoPlayer.exe | Select-Object -First 1
$o = Join-Path $env:TEMP 'bongo-baseline-version.txt'
Start-Process $exe.FullName -ArgumentList '--version' -Wait -NoNewWindow -RedirectStandardOutput $o
Get-Content $o
```

Expected: `Bongo Player 0.6.1`. Write the exe's path in the report.

- [ ] **Step 9: Commit**

Message: `docs: the channel list and defect marks in CLAUDE.md, the README, the zip README and VALIDATION`, followed by the two attribution lines.
Files:
- `CLAUDE.md`
- `rust/README.md`
- `docs/VALIDATION.md`
- `rust/build-zip.ps1`

- [ ] **Step 10 (the controller, with the user's consent): the windows and the startup A/B**

Ask the user first: these runs open the app's window many times.

```powershell
cd C:\Dropbox\Sources\tv\rust
powershell -NoProfile -ExecutionPolicy Bypass -File e2e\regression.ps1 -Group M
powershell -NoProfile -ExecutionPolicy Bypass -File e2e\regression.ps1 -Group R,B,C,N,X,G
powershell -NoProfile -ExecutionPolicy Bypass -File e2e\measure-startup.ps1 -Baseline <the 0.6.1 exe> -Runs 10
powershell -NoProfile -ExecutionPolicy Bypass -File e2e\measure-startup.ps1 -Baseline <the 0.6.1 exe> -Runs 10 -M3u
```

- A pass on re-run counts (see the e2e README on doubled characters).
- The startup A/B: the new exe's median and minimum are within ±10 % of the baseline's, or better, in both runs. A slower result stops the release; investigate before going on.
