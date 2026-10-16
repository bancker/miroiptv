# Thuisbioscoop, phase 1 (het fundament) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The data layer of the thuisbioscoop, with nothing new on screen but one log line per keystroke in `f`: the film and series lists' extra fields and the portal's categories; a compact title index (genres, labels, merged versions; search, filters with counts, four sorts, "meer zoals dit") built off the screen, handed out only behind the catalog's adult lock and kept between starts in a cache file; the watching store; the details and cover stores; the player's VOD load and its reports; and release timing tests with `f` as the baseline.

**Architecture:** A new `rust/src/vod/` holds the data and draws nothing. `genres.rs` and `versions.rs` read names (genres, labels, years) and merge a film's versions; `index.rs` keeps every title in a 20-byte record, with its name, search name and cover in shared text tables, and answers a query with its counts in one pass; `cache.rs` writes it to a file per portal. The catalog's fetch task asks the two category lists beside the three lists and puts the catalog as soon as the three lists are in, as before; the titles wait for the categories (15 s at most), then the index is built on the blocking pool from the stored lists, copying none of them, and put beside the catalog: the channels wait for none of it. The index leaves the store only as `Titles`, which apply the lock. `watching.rs`, `details.rs` and `covers.rs` are stores that phase 2 wires to pages; here they are built and tested on their own. The portal gains the categories and the details through trait methods with defaults (an M3U list answers with nothing), and the player a VOD load whose reports run only while one plays.

**Tech Stack:** Rust 2021, egui/eframe 0.27.2, tokio (the blocking pool), reqwest 0.12 (rustls), serde + serde_json, image 0.24 (png; jpeg added here), ring (SHA-256), parking_lot, chrono, libmpv (client API 2.5); tempfile and wiremock (tests).

**Spec:** `docs/superpowers/specs/2026-10-15-thuisbioscoop-design.md`: phase 1 of §9, with §4.1–§4.6, §5, §6 and §7 as far as phase 1 builds them.

**Where:** the main checkout `C:\Dropbox\Sources\tv`, branch `feat/thuisbioscoop`, which the controller rebases on the finished zenderlijst and on the rename (`APP_ID` "BongoIPTVPlayer") and checks out there before Task 1. The plan was written at c81d7b0: main with the M3U sources, plus Tasks 1–4 of the zenderlijst (`bad_streams.rs`, the zap ring, `stream_check.rs`); the zenderlijst's Tasks 5–10 and the rename come in between. So this plan touches `app.rs` in a few small places only (the `f` log line, reading the index cache after the first frame, the cache path of a new catalog, one arm of the player's events, two test literals) and names functions, never line numbers. When a function reads differently from what a step shows around a change, keep the step's meaning, make the change there, and say so in your report.

Every step's code below was built and run, task by task in this order, on a copy of c81d7b0, once as first written, again with the pre-flight rulings and once more with their follow-up rulings: each task's tests passed, the rustfmt hunk counts held, and clippy showed only the two old warnings.

## Global Constraints

- **Performance is binding.** From the spec (§5, §7), word for word:
  - "Index bouwen: Hooguit 150 ms extra op de laaddraad en hooguit 5 MB geheugen."
  - "Typen of een filter aan/uit → resultaten met aantallen: Binnen 16 ms (één frame), en nooit trager dan `f` nu op dezelfde machine."
  - "App starten: Niet trager dan nu. A/B met `rust/e2e/measure-startup.ps1 -Baseline`: minstens 10 runs, mediaan en minimum binnen ±10 %, met een Xtream-bron."
  - "Zappen: Ongewijzigd. Een gesloten thuisbioscoop kost per frame niets: geen werk, geen geheugen, geen netwerk."
  - "Geheugen: Cover-texturen hooguit 96 MB. Schijfcache voor covers hooguit 500 MB."
  - "Netwerk: Details alleen bij het openen van een pagina, hooguit 2 tegelijk. Covers hooguit 6 tegelijk, alleen voor wat in beeld komt. Bij het starten niets extra, behalve de twee categorielijsten in de bestaande catalogusronde."
  - "Draden: geen nieuwe thread. De index komt op de bestaande laaddraad. Details, covers en de trailer lopen op de bestaande tokio-runtime, en decoderen op de blocking pool."
  - "Haalt een fase een norm niet, dan gaat ze niet ter test naar de gebruiker. De getallen gaan eerst naar de gebruiker, met wat eraan te doen is."
- **Dependencies:** "`Cargo.toml`: `image` krijgt `jpeg` [...]. Geen nieuwe crate." Its JPEG decoder, `jpeg-decoder` 0.3.2, joins `Cargo.lock` (Task 9); no other crate. Tokio's `process` feature belongs to phase 3 (the trailer) and is not added now.
- **`f` stays as it is** ("`f` blijft precies zoals hij nu is"). Its only change is the log line `search: <n> results in <ms> ms`; "de regel zegt niets over wat er gezocht is".
- **Texts:** no UI text outside `i18n.rs`; every key has NL, EN and DE; log lines stay English. A version's labels ("4K HDR", "NL SUBS") are tokens that read the same in every language, as the search's `[LIVE]` tag does.
- **Credentials (spec §6):** "Nooit een adres met het account in een log, cache of `Debug`-uitvoer. Nieuwe types met zo'n adres krijgen `cfg_attr(test, derive(Debug))`."
  - Here that is `VodLoad`, and `vod::index::Title` (its cover).
  - The index cache holds "geen volwassen titels [...] en geen inloggegevens", and no cover address with a query either: `redact` does not mask a token there. A details file holds "alleen de velden die de pagina toont, zonder adressen met inloggegevens", without an image address with a query too (memory keeps it). A cover's file name is a hash, and "Een cover-adres met de inloggegevens van de portal erin wordt nooit gelogd."
  - Covers log hosts only (`guide::logos::host_of`), a VOD load's line its host only (`redact::url_host`), the details store titles by id. The errors of the new API calls name no address (`XtreamPortal::api_json`), so a failed category list is logged without one.
  - `Cmd`, `XtreamCreds` and `XtreamPortal` derive `Debug` in the crate's own tests only: the integration tests (`rust/tests/`) never format one with `{:?}`, nor compare one with `assert_eq!`.
- **The catalog is the only adult filter.** The index keeps adult titles with a flag and leaves the store only as `Titles`, whose every answer leaves them out while locked: "Er komt nergens anders een filter."
- **Nothing visible yet.** No store is wired to a screen in this phase. What the running app does new: the `f` log line, the two category lists asked beside the three lists, the catalog's `vod:` log line, the index built after the catalog is put and its categories are in (15 s at most) and written to its file, that file read after the first frame, and the covers folder (empty until phase 2) pruned in the background at a start.
- **Release profile** is `panic = "abort"`: nothing may rely on catching a panic, and nothing from outside (a name, an answer, a file) may cause one; the tests feed odd ones. A value read from a file that serves as an index or a bound is masked or validated where it is read (Ruling 51), with a test for each kind.
- **No window, no real network.** Every test talks to wiremock or a `TcpListener` on 127.0.0.1, never to a real portal, YouTube or TMDB. The mpv smoke tests (ignored) run libmpv without a window. This phase has no e2e scenario; the one window run is the controller's startup A/B at the end, with the user's consent. No real mouse or keyboard injection (no SetCursorPos, mouse_event, keybd_event, SendInput): the user works on this machine.
- **The real ini:** never open, print, copy or modify any `BongoPlayer.ini` or `BongoIPTVPlayer.ini`; never read or copy anything under a `rust\target` folder (copying the single exe, `BongoIPTVPlayer.exe` or 0.6.1's `BongoPlayer.exe`, is fine). `rust/e2e/check-staged.sh` reads the real ini next to the release exe itself and prints only counts.
- **Formatting:**
  - Every new file is rustfmt-clean (`rustfmt --check --edition 2021 <file>`). Run `rustfmt --edition 2021 <file>` once you have written one: the listings are rustfmt's layout, but a pasted line may still differ.
  - In `app.rs`, `catalog.rs`, `player/mod.rs` and `portal/xtream.rs` (not rustfmt-clean), format only the lines you add, and never run rustfmt on them without `--check`. The old hunks are as measured at the rebased base: Task 1 Step 1 records them, and every task keeps them. At c81d7b0 they were `app.rs` 7, `catalog.rs` 3, `player/mod.rs` 2, `portal/mod.rs` 1 (the one in `portal/xtream.rs`, which it declares), `portal/xtream.rs` 1, `tests/test_args.rs` 8, `tests/test_xtream.rs` 8, and every other file was clean; the steps name these numbers.
  - rustfmt on a file also checks the modules that file declares. Never pass `lib.rs` to it.
  - Files that are clean before you edit them stay clean: `storage.rs`, `i18n.rs`, `main.rs`, `portal/types.rs`, `player/events.rs`, `player/mpv.rs`, `player/mpv_sys.rs`, and the test files you touch.
- **Clippy:** `cargo clippy --all-targets` shows only the 2 old warnings (`portal/xtream.rs`: items after a test module; `main.rs`: a nul-terminated string built by hand).
- **Building and testing:** cargo runs from `C:\Dropbox\Sources\tv\rust` with `CARGO_TARGET_DIR=C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt-thb` (this plan's own target folder, apart from the zenderlijst's; it holds `libmpv-2.dll` in `debug\` and `release\` already), `-j 4` (memory is tight) and `--config 'profile.dev.package.bongo-player.debug=1'` (the linker otherwise hits "Permission denied" on the test exe). Task 1 notes how many tests pass before this plan; each task reports the new total. Task 1 Step 0 builds the base's release into `tgt-thb`; only Task 12 builds the release into `rust\target`.
- **Command shorthands used in the steps** (bash; shell functions do not survive between tool calls, so type the command out):
  - `ct ARGS` means `cd /c/Dropbox/Sources/tv/rust && CARGO_TARGET_DIR='C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt-thb' cargo test -j 4 --config 'profile.dev.package.bongo-player.debug=1' ARGS`.
  - `cc` means the same with `cargo clippy -j 4 --all-targets` in place of `cargo test …`.
  - `fmt FILES` means `cd /c/Dropbox/Sources/tv/rust && rustfmt --check --edition 2021 FILES`.
  - `hunks FILE` means the same `cd` and `rustfmt --check --edition 2021 FILE 2>&1 | grep -c '^Diff in'`.
- **Commits:**
  - On branch `feat/thuisbioscoop` only. Never push, never checkout, stash, rebase or reset.
  - Commit with `git add <files> && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- <files>`, run from the repo root `C:\Dropbox\Sources\tv`. Write the message file in the scratchpad.
  - The check must print `staged diff: username hits 0, password hits 0, token hits 0`. A `no … - nothing to check against` stops the commit and goes to the controller: the rename makes the script read `BongoIPTVPlayer.ini`, else `BongoPlayer.ini`, next to the release exe, and fail when there is neither.
  - Stage only the files the task names.
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
  - `libmpv-2.dll` is git-ignored. The tests find it in the target folder; a release build takes it from `rust\vendor\libmpv\` (`build.rs` copies it next to the exe). Should it be missing there, copy `tgt-thb\debug\libmpv-2.dll` in first.
  - PowerShell 5.1 does not wait for the GUI exe started with `&`. Use `Start-Process -Wait -RedirectStandardOutput` for `--version`/`--selftest`.
  - Task 9 fetches `jpeg-decoder` from crates.io once (the network is there).
- **The version is not bumped.** The user decides at release.

## Rulings

The spec is the authority. These settle what it leaves open, one line each: what, and why.

**The portal (§4.1)**

1'. **The categories come through a trait method, not in `Catalog`, and the channels wait for nothing:** `Portal::fetch_categories` (a default that answers none). The catalog's fetch task asks the two category lists beside the three lists, in a task of their own (one list and one category list in flight at a time), and puts the catalog as soon as the three lists are in, as before phase 1: zapping, the zap ring, `f` and the guide work from that moment and never wait for the categories. Then it awaits the categories, 15 s at most (`CATEGORIES_WAIT`): a refused or broken category list, or none by then, gives the titles none, with one warn line that names no address (`api_json` drops the URL from its errors), and a request still open stops; the titles are worth more than their genres. Only then does it build the title index on tokio's blocking pool (`spawn_blocking`, no new thread) and put it in the store through `put_titles`, which counts it in `titles_revision` (titles of a catalog that was replaced meanwhile are not put: the newer one's own come after it). The adult lock applies to the titles through the store, as to the lists. F5 does the same. `Catalog` keeps its three fields, so its many struct literals in `app.rs` and the tests stay as they are.
2. **`Movie` and `Series` gain the fields and derive `Default`;** the two literals that name every field (`app.rs`'s test `library_catalog`, `tests/test_catalog_adult.rs`'s helpers) take `..Default::default()`. A new field always breaks such a literal, and that is the smallest touch.
3. **Read tolerantly:** a rating ("7.3", 7.3, "7,3") becomes tenths in a `u8`; empty, null, a word, 0 and anything outside 0–10 mean none (spec: "een lege waarde of 0 betekent geen waardering"). Ids, times and category ids come as a number or a numeric string; anything else is 0, never a failed list.
4. **`releaseDate` and `release_date` are two fields,** read by `Series::released`: a serde alias fails the whole list on an entry that has both (a duplicate field), as `start` and `start_timestamp` once emptied the EPG.
5. **A series' "toegevoegd" is its `last_modified`:** `get_series` has no add date.
6. **The details are `portal::VodDetails`** (with `SeasonInfo`, `EpisodeInfo`), parsed tolerantly in `portal/vod_info.rs` from the shapes portals use (`plot` or `description`, `cast` or `actors`, a backdrop as a string or a list, episodes keyed by season or as lists). Field names: `plot, cast, director, genre, released, year, age, duration, rating, trailer, backdrop, tmdb, width, height, hdr, channels, seasons[number, name, cover], episodes[id, season, number, title, ext, duration, plot, still, added]`. That is what the pages of §3.4 and §3.5 show, nothing more (no `direct_source`).
7. **`fetch_series_episodes` stays as it is:** nothing calls it; a cleanup removes it, as the 0.5 shortcuts are kept until one.
8. **`PortalError::NoDetails`** is what the defaults answer: an M3U list has no films to tell about.
9. **An image address that `redact` would change holds an account,** and is dropped where it would be written: from the details (backdrop, season covers, stills) and from the index cache (covers). An address with a query is left out of both files as well, since `redact` does not mask a token there: a cover's tile shows the placeholder until the fresh lists come, and a page shows the details' image from memory. Covers stay in memory as the lists give them, and are only ever named by a hash or their host.

**Names (§4.2)**

10. **The 24 genres:** Actie, Avontuur, Animatie, Anime, Biografie, Kerst, Komedie, Misdaad, Documentaire, Drama, Familie, Fantasy, Geschiedenis, Horror, Kinderen, Muziek, Mysterie, Reality, Romantiek, Sciencefiction, Sport, Thriller, Oorlog, Western: TMDB's film and TV genres merged (Action & Adventure split, TV movie, News, Soap and Talk left out), plus Anime, Biografie, Kerst and Kinderen, which Dutch portals use as categories. Aliases are folded whole words in NL, EN and DE with their plurals ("Komedie", "Comedy", "Komödie"; "stand up" and "cabaret" are Komedie).
11. **A series whose genre field names no genre takes its category's,** as a film does; the spec names the field only.
12. **Labels and language markers count only where labels stand:** a prefix before the last `|` (whatever else it holds), a prefix before ` - ` that is labels only, a group in brackets that is labels only, and the words at the end. The "en" of "Romeo en Julia" is Dutch for "and"; read anywhere, "als los woord", it would make half the Dutch titles English.
13. **"NL SUBS" (also "NL SUB", "NLSUBS") is no Dutch marker:** it is Dutch subtitles, not Dutch speech. "Nederlands Gesproken" is one, beside the spec's "NL Gesproken". Dutch wins over English when both are named (the spec names it first).
14. **A category's name also says its language by the label words it starts with** ("Nederlands", "NL FILMS", "NL Gesproken Kids") (`genres::category_labels`): a portal names a section after its language, with or without a `|`; "Films en Series" and "Kids" say none. A title's name of one label keeps it as its title.
15. **The year of a name:** "(2024)" anywhere, else " 2024" at the end before the labels, and only up to the year after this one, so "Blade Runner 2049" keeps its title. At least one word stays a title: "Dual (2022)" is the film Dual, "1917" the film 1917.
16. **The fold** (search names, keys, aliases): lower case, the accents of the Latin scripts off by a fixed table ("Pokémon" → "pokemon", "ß" → "ss", "æ" → "ae"), every run of other characters one space; letters of other scripts stay, lower case.
17. **`parse_name` allocates nothing once warm** (a thread-local scratch), and the build allocates once per title, not per film or per series: it reads ~58,000 names per catalog. A first build with allocations per name took ~190 ms for Pandora's size; this one takes ~80 ms (the budget is 150 ms).

**Versions and titles (§4.2)**

18. **The default version:** the highest tier by `catalog::quality_rank`'s scale read from the labels (UHD 4, FHD 3, HD 2, unmarked 1, SD 0), HDR breaking a tie, then the lowest stream id, so a reload keeps the same default (and title id). A title's name is its default version's.
19. **What a film title takes from its versions:** the genres of all their categories, the languages of all (labels and categories), the first rating that is not 0 (the default's first), the earliest add day (a new 4K copy of an old film is no new title), the first cover there is, and adult when any version is (as a guide column is).
20. **A title's language is a set** (Dutch, English, both or neither): one with an NL and an EN version is found under both. "Andere" is neither, unknown included.

**The index (§4.2, §3.3)**

21. **A record is 20 bytes:** id `u32`, genres `u32`, versions end `u32`, year `u16`, add day `u16`, category `u16` (a place plus one), rating `u8`, flags `u8` (film, adult, trailer, Dutch, English). Name, search name and cover live in three text tables (one `String` and its ends each); a version is 8 bytes (`u32` stream id, `u16` labels, `u8` container code); the A-Z rank is a `u32` per title. Xtream ids are positive and below 4·10⁹: a title whose id does not fit is left out and counted (`Stats::skipped`). Measured on Pandora's size (53,057 titles): 4.16 MB.
22. **The covers are no part of the 5 MB:** §4.2's record holds "geen tekst behalve een verwijzing naar de naam", and §4.4 takes the covers "uit de lijsten". They sit beside the records in their own text table (`cover_bytes`, 4.02 MB for Pandora), which the lists give up before they are stored (`vod::index::Extras`, which `StoredCatalog::new` hands to the build in its `TitleInput`), so no address is held twice once the index has it. The log line prints both sizes; the timing test prints them and the list records', and the user gets all three numbers (spec §5) with the remedies: a prefix table for the cover addresses, narrower stored lists. Phase 1 builds neither: the numbers decide it in phase 2.
23. **Lookups:** records in (kind, id) order, so `find` is a binary search; `title_of_version` scans the versions (no map: it is asked once per playback, not per frame).
24. **`learn`:** what a title's details tell the shared index in place (atomics): a year where its name gave none (the name's year made the title's key), genres where none were named, and that it has a trailer. Nothing calls it before phase 2. What `learn` adds lives in that index only: the cache is written from a fresh index before anything is learned, and the next load builds anew. Phase 2 decides whether the filters keep it (carry it into a new index by `TitleId`, or learn again from the details files).
25. **Filters:** values of one group add up (Komedie or Drama), the groups narrow each other; a value's count leaves its own group out, so it says what choosing it gives. "Eerder" is a known year before 2000; an unknown year is in no bucket. ★ 8 and ★ 7 overlap. A query without letters or digits filters nothing.
26. **The sorts:** Relevantie: the title starts with the whole folded query, then ★, then A–Z. Best gewaardeerd: ★ high to low, none last, the newest on a tie (as the rows of §3.2). Nieuwst: the add day, unknown last, then ★. Titel: A–Z by the folded title. Every tie ends in A–Z.
27. **"Meer zoals dit":** titles that share a genre with it or sit in its category, series and films alike, best rated first (the newest on a tie), without it, without what is `watched` or `disliked`, adult ones only unlocked. The caller makes the two sets (phase 2 and 3: a film by its title id).

**The catalog (§4.2, §11)**

28. **The "laaddraad" is the catalog's fetch task;** the build runs there on the blocking pool once the catalog is put and its categories are in (`spawn_blocking` around `TitleInput::build`), so the ~80 ms never hold a tokio worker, where the relay runs, nor the screen, nor the channels. It reads the stored lists and copies none of them (Ruling 53). `from_catalog` (tests, tools) builds in place. The lists' parse stays where it was (the fetch's worker); its extra fields are timed by the timing test.
29. **`Titles` is the index and the lock state when asked:** a handle taken while locked stays locked; one taken unlocked shows adult titles until asked again (§6: "De rijen en de resultaten worden opnieuw berekend").
30. **The data quality line**, once per build that has titles: `vod: <n> titles (<s> series, <f> films in <v> versions, <k> left out), <c> categories - cover <a> %, rating <b> %, year <c> %, genre <d> %, Dutch <e> %, English <g> % - index <x> KB, covers <y> KB, built in <t> ms`. The language shares show whether a portal's "NL |" means Dutch speech or a Dutch section.

**The index cache (§4.6)**

31. **The file:** `cache\vod-<host>_<port>.json` (`vod::cache::portal_key`, the EPG cache's rule for the characters), JSON with `version` 1, `saved_at`, the categories as `[film, id, name]` and a title as an array `[film, id, name, cover, year, rating, genres, category, added, dutch, english, trailer, [[stream id, labels, container]]]`; the search names are built again when it is read. It is written as it is serialized, from the index's own text (`CachedTitleRef`), through a buffered temporary file: no copy of its ~10 MB is made.
32. **The store knows its file** (`CatalogStore::with_vod_cache`, set by `main` and `install_portal` from the portal's host and port; none for an M3U list or in tests), writes it after every build of the catalog's titles on the blocking pool, one write at a time, and reads it once, when asked. The file's titles are put only while the store has none, and the catalog's own take their place; titles that came first keep theirs.
33. **"After the first frame":** `update_ui` marks the first frame and asks the store from the second on; the store reads once, so later frames cost an atomic load.

**What was watched (§4.5)**

34. **Keys:** a film version `m:<stream id>`, an episode `e:<id>` (a number); the list and the thumbs `s:<id>`/`m:<id>`, a film by its title id (its default version's stream id when it was pressed).
35. **The rules:** under 60 s an item in progress goes, a seen one stays seen; at 60 s or more under 90 % a seen one is in progress again (watched again); the same whole second again is no change (pause writes nothing). The first write goes at once, the next 5 s later at the earliest, a stop at once; a failed write keeps the change for the next one and is logged once until a write works. Another version's file, or one that does not parse, is kept as `.bad`; entries it cannot read are left out. Written pretty, as the spec shows it.
36. **`remove_source_files` also removes the `.bad` copy:** it is that source's data.

**Details (§4.3)**

37. **Freshness counts from the file's own `fetched_at`** (a time from the future counts as now); the old file shows while the portal is asked again (`refreshing`). A failed refresh keeps the old details without a word; a failed first fetch is `Failed` until `retry` (Enter): opening the page again asks nothing.
38. **Waiting requests are kept to the 8 newest,** and 64 titles' details in memory; an older page is read from its file again. The file is read in `request` itself, on the screen's thread, so it shows "in hetzelfde frame" (spec §5); the portal and the write run on tokio.

**Covers (§4.4)**

39. **6 downloads and, beside them, 6 reads of kept files at once:** a slow server never holds up a kept cover ("Covers uit de schijfcache volgen binnen 300 ms"), and a fast scroll starts no swarm of blocking threads. A cover that a frame no longer asks for before it comes is dropped at `end_frame`; one without a picture is not asked again this session, as the logos.
40. **At most 8 textures are made per frame,** so a screenful read from disk spreads over a few frames instead of making one long one (§5: "geen frame boven 16 ms").
41. **Decoding:** JPEG or PNG by their first bytes, at most 16 MB a body and 8192 px a side, cut to fill the size at the middle (CatmullRom); the textures are named `cover <hash> WxH`. The texture LRU is generic over `CoverTexture` (its bytes) and the store over `Uploader` (egui's context in the app, a fake in the tests). One texture over the cap alone stays alone.
42. **The folder's 500 MB:** `covers::prune` removes the oldest files by their write time; `main` runs it in the background at every start (`prune_in_background`, spec §4.4: "Bij het starten ruimt de app op de achtergrond het oudste op"). Before phase 2 nothing writes covers, and a missing folder costs one failed `read_dir`.
43. **The placeholder's colour:** the hue from the first two bytes of `sha256(name)`, saturation 0.45 and value 0.35 as the screen shows it: muted and dark under white letters.

**The player (§7)**

44. **`Cmd::LoadVod(VodLoad { url, start, audio_file })`:** the URL byte for byte; `start=<s>` and `audio-file=%<n>%<url>` as options of that one file (`vod_loadfile_args`), and none at all for a start at 0 without a sound track, which is then the same plain `loadfile` a film from `f` gets. Its line names the host and the options only (`loadfile vod from h (start 4380 s)`): a trailer's address holds tokens `redact` does not know.
45. **`Cmd::SetAudio`/`SetSubtitles(Option<i64>)`** set `aid`/`sid` (none is `no`); **`Cmd::SetMute(bool)`** sets `mute`.
46. **One new event, `Event::Vod(VodEvent::{Position, Duration, Tracks})`:** `app.rs`'s exhaustive match gains one arm, which phase 2 fills.
47. **The VOD properties are observed only while a VOD load plays** (observed at `LoadVod`, unobserved at any other load or a stop, under `OBSERVE_VOD`, and a late change after that dropped), so live TV costs nothing. Positions and lengths are reported when their whole second changes: a stream that tells its length as it is read (measured: a lavfi source, ~12 a second) reports once a second. `track-list` is observed as a string, which the vendored libmpv gives as JSON (pinned by the smoke test); video tracks are left out. A VOD load after another keeps them observed: what comes from a load until mpv's `MPV_EVENT_START_FILE` of it is dropped (a change of the old file, read when it is delivered, would be taken as the new one's), and the gates reset at START_FILE.

**The baseline (§5)**

48. **`f`'s line:** `search: <n> results in <ms> ms` at info, `<n>` what `f` keeps (its best 12), `<ms>` with one decimal, one line per search `f` runs (each change of the query, and the catalog's arrival). The thuisbioscoop's page will log its own count beside it; the times are what the two compare.
49. **The timing test's catalog** has 13,000 live channels beside its 47,188 films and 10,589 series, as Pandora's: `f` searches them too. It asserts no speed; it prints. Its `f today` is `f` on phase 1's lists; `f` before phase 1 is timed at the base by Task 1 Step 0's `test_f_timing.rs` (0.6.1's fields only, so it is the same test before and after). Task 12 runs it again: its worst keystroke stays within ±10 % of the base's, or the numbers go to the user.
50. **The startup A/B's baseline is the 0.6.1 release,** as the zenderlijst's: it is the build the user runs now. It runs as before, and on a catalog of Pandora's size (`-VodBig`, Task 11b), where the new exe builds its index at every start and, from the second run on, reads its cache. The rebased base's own release (Task 1 Step 0) is a second baseline, for information: it shows phase 1's own share.

**After the pre-flight**

51. **What a file says is checked where it is read:** a value that serves as an index or a bound is masked or validated, each kind with its test. The index cache: genre bits past the 24 are dropped (they index the counts), a category's place past the file's categories names none, labels with stray bits name nothing new, and containers past the 255 codes have none. A details file's `fetched_at` from the far past or future only makes it stale or fresh. The release build aborts on a panic, and one odd file would take every start with it.
52. **One atomic writer in `vod/`:** `write_atomic_with` (a temporary file beside the file, renamed over it) writes the index cache, the watching file and the details files. Across modules the zenderlijst's Ruling 31 stands: `cache::portal_key` and `covers::file_stem` stay as they are, beside `guide::cache`'s and `guide::logos`' own.
53. **The title build copies no list:** the store shares its films and series through one `Arc` (`StoredCatalog.catalog`; its live channels stay apart in `live`), and the build reads them through a clone of it with what they gave up beside them (`TitleIndex::build_with`, `vod::index::Extras`). Beside the lists only the handles of what moved out are new, an `Option<String>` a film and 56 bytes a series (a series' release dates go, their year stays): ~1.7 MB for Pandora, from the catalog's put until its titles are built; the strings themselves move. The timing test prints the build's peak on the heap, what it keeps (the index) and what passes (its own working lists).

## Review Focus

The six inputs the spec implies but its test list does not name, most likely to bite first. Each has its test in the owning task.

1. **Odd names from the portal** (a no-break space before the year, an unclosed bracket, a name that is one label or only a year, an en dash or emoji at the end, a combining accent): `parse_name` never panics, since the release build aborts on a panic and one bad name would take every start with it, and it always keeps a title. → Task 2b, `odd_names_never_panic_and_keep_a_title`.
2. **Nulls and wrong types in a details answer** (`"info": null`, `"seasons": null`, `"episodes": {"1": null}`, an episode without an id, a rating that is an object, a video that is a string): fields stay empty, the answer is no error, and an episode that cannot play is left out. → Task 1, `nulls_and_odd_types_in_a_details_answer_leave_fields_empty`.
3. **A series entry with both `releaseDate` and `release_date`:** the whole series list still loads, and the date that has text wins. → Task 1, `the_series_list_carries_cover_rating_release_date_genre_category_and_change_time` (its third series).
4. **The category lists refused, busy or stalled beside the three lists** (the portal answers 503 or 509 while the lists come, or answers nothing): the catalog loads at once and its titles go without genres, 15 s later at most, instead of the thuisbioscoop losing everything or the channels waiting. → Task 5, `a_refused_category_list_leaves_the_titles_without_genres`, `a_stalled_category_list_holds_up_neither_the_catalog_nor_zapping` and `at_the_bound_the_titles_are_built_without_the_categories`.
5. **A query of punctuation or spaces only** (" ", "!!!", " - ", "★", what a user types before a word): it filters nothing and shows what an empty query shows, never no results. → Task 4, `a_query_without_letters_or_digits_is_no_text_filter`.
6. **A cache file that holds what no index writes** (genre bits past the 24, a category's place past its categories, labels with every bit set, more containers than codes): it is read without a panic and names nothing it does not have (Ruling 51). → Task 6a, `a_file_with_stray_genre_bits_counts_the_24_and_nothing_else` and the two tests after it.

Not a finding: the version labels ("4K HDR", "NL SUBS") are built outside `i18n.rs`; they are tokens, the same in every language (Global Constraints, Texts).

## File structure

| File | Responsibility |
|---|---|
| `rust/src/portal/types.rs` | the films' and series' extra fields, `Category`/`Categories`, `VodDetails`/`SeasonInfo`/`EpisodeInfo`, the tolerant readers |
| `rust/src/portal/vod_info.rs` (new) | `get_vod_info` and `get_series_info` answers read into `VodDetails`, tolerantly |
| `rust/src/portal/mod.rs`, `portal/xtream.rs` | `fetch_categories`, `fetch_vod_info`, `fetch_series_info` (defaults; Xtream's own, whose errors name no address); `PortalError::NoDetails` |
| `rust/src/vod/mod.rs` (new) | `Kind`, `TitleId`, `fold`, `write_atomic_with` |
| `rust/src/vod/genres.rs` (new) | the 24 genres, aliases, labels, language markers, `parse_name`, `category_labels` |
| `rust/src/vod/versions.rs` (new) | films merged into titles with versions |
| `rust/src/vod/index.rs` (new) | `TitleIndex`: records, text tables, lookups, `learn`, stats; search, filters, counts, sorts, `more_like`; `Extras` and `build_with`, the catalog's build |
| `rust/src/vod/cache.rs` (new) | the index cache file |
| `rust/src/vod/watching.rs` (new) | `Watching`: progress, Mijn lijst, thumbs, the file |
| `rust/src/vod/details.rs` (new) | `DetailStore` |
| `rust/src/vod/covers.rs` (new) | `CoverStore`, `TextureLru`, `decode`, `prune`, `placeholder_tint` |
| `rust/src/catalog.rs` | the catalog put as soon as the three lists are in; the categories asked beside them and awaited 15 s at most; then the index built from the stored lists, logged and put beside it; `Titles`; the cache's path, read and write |
| `rust/src/player/` | `Cmd::LoadVod`, `SetAudio`, `SetSubtitles`, `SetMute`; `Event::Vod`; observing the VOD properties |
| `rust/src/app.rs` | the `f` log line; the cache read after the first frame; the cache path of a new catalog; the `Event::Vod` arm; two test literals |
| `rust/src/main.rs` | the cache path of the first catalog; the covers folder pruned at a start |
| `rust/src/storage.rs` | `watching_path`; `remove_source_files` |
| `rust/src/i18n.rs` | the genres' names |
| `rust/src/lib.rs` | `pub mod vod;` |
| `rust/Cargo.toml`, `Cargo.lock` | `image` with `jpeg` |
| `rust/tests/test_vod_*.rs`, `test_player_vod.rs`, `test_f_timing.rs` (new); `test_storage.rs`, `test_catalog_adult.rs`, `test_mpv_smoke.rs` | tests |
| `rust/e2e/mock_portal.py`, `measure-startup.ps1`, `README.md` | the startup A/B on a catalog of Pandora's size (`MOCK_VOD_BIG`, `-VodBig`) |
| `CLAUDE.md`, `rust/README.md`, `docs/VALIDATION.md` | docs |

## Tasks

1. The portal: richer lists, the categories and the details (Step 0 times `f` before anything changes)
2. Names, in two tasks: 2a the fold and the 24 genres; 2b the labels, the year of a name and the versions
3. The title index: records, text tables and lookups
4. The title index: search, filters, counts, sorts and "meer zoals dit"
5. The catalog: the titles built after the catalog, off the screen, behind the lock
6. The index cache, in two tasks: 6a the file; 6b kept by the catalog, read after the first frame
7. What was watched
8. The details store
9. The cover store
10. The player: the VOD load and its reports
11. The baseline: `f`'s line and the timing test; then 11b, the A/B's catalog of Pandora's size
12. Docs, the release measurement, full verification

---

### Task 1: The portal: richer lists, the categories and the details

The film and series lists keep what the thuisbioscoop needs (spec §4.1): a cover, a rating read tolerantly into tenths, when the portal added it and its category; a series also its release date, genres and when it changed. Two small lists come in the catalog's round, the categories of films and of series. A film's and a series' details (`get_vod_info`, the full `get_series_info`) come through the `Portal` trait too, read tolerantly into `VodDetails`. Every new method has a default, so the M3U portal and the tests' portals compile as they are and answer with nothing.

**Files:**
- Create: `rust/src/portal/vod_info.rs`
- Modify: `rust/src/portal/types.rs`, `rust/src/portal/mod.rs`, `rust/src/portal/xtream.rs`
- Modify: `rust/src/app.rs` (the tests' `library_catalog`), `rust/tests/test_catalog_adult.rs` (`movie`, `series`)
- Test: `rust/tests/test_vod_portal.rs` (new), `rust/tests/test_f_timing.rs` (new, ignored; Step 0)

**Interfaces:**
- Consumes: `deser_id`, `deser_flag`, `deser_opt_string` (portal/types.rs); `XtreamPortal::{api_url, client}`; `crate::redact::redact`; for Step 0, `CatalogStore::{from_catalog, search}` as they are at the base.
- Produces:
  ```rust
  // portal/types.rs (all `Debug, Clone, Default, Serialize, Deserialize`)
  pub struct Movie { /* as before */ pub stream_icon: Option<String>, pub rating: u8, pub added: i64, pub category_id: i64 }
  pub struct Series { /* as before */ pub cover: Option<String>, pub rating: u8,
      pub release_date_camel: Option<String> /* "releaseDate" */, pub release_date: Option<String>,
      pub genre: Option<String>, pub category_id: i64, pub last_modified: i64 }
  impl Series { pub fn released(&self) -> Option<&str>; }
  pub struct Category { pub category_id: i64, pub category_name: String }      // + PartialEq, Eq
  pub struct Categories { pub vod: Vec<Category>, pub series: Vec<Category> }  // no serde
  pub struct VodDetails { pub plot: String, pub cast: String, pub director: String, pub genre: String,
      pub released: String, pub year: u16, pub age: String, pub duration: u32, pub rating: u8,
      pub trailer: String, pub backdrop: String, pub tmdb: i64, pub width: u32, pub height: u32,
      pub hdr: bool, pub channels: u8, pub seasons: Vec<SeasonInfo>, pub episodes: Vec<EpisodeInfo> }
  pub struct SeasonInfo { pub number: u32, pub name: String, pub cover: String }
  pub struct EpisodeInfo { pub id: String, pub season: u32, pub number: u32, pub title: String,
      pub ext: String, pub duration: u32, pub plot: String, pub still: String, pub added: i64 }
  pub(crate) fn whole_number(v: &serde_json::Value) -> i64;
  pub(crate) fn rating_tenths(v: &serde_json::Value) -> u8;
  // portal/vod_info.rs
  pub fn parse_vod_info(answer: &serde_json::Value) -> Result<VodDetails, PortalError>;
  pub fn parse_series_info(answer: &serde_json::Value) -> Result<VodDetails, PortalError>;
  pub fn year_of(date: &str) -> u16;
  // portal/xtream.rs (private): api_json, whose errors name no address (`reqwest::Error::without_url`)
  // tests/test_f_timing.rs (ignored): f_on_a_catalog_of_pandoras_size_timed
  // portal/mod.rs
  PortalError::NoDetails
  trait Portal {
      async fn fetch_categories(&self) -> Result<Categories, PortalError>;            // default: none
      async fn fetch_vod_info(&self, vod_id: i64) -> Result<VodDetails, PortalError>;  // default: NoDetails
      async fn fetch_series_info(&self, series_id: i64) -> Result<VodDetails, PortalError>;
  }
  ```

- [ ] **Step 0: `f` before phase 1**

Before anything else changes, a release test times `f` as it is (spec §2: "ik wil 100% zeker zijn dat het even snel is als optie f nu"; Ruling 49). It reads its catalog from JSON in 0.6.1's fields only, so the same file compiles and times `f` again in Task 12, after phase 1.

Create `rust/tests/test_f_timing.rs`:

```rust
//! `f` before the thuisbioscoop and after it (spec 2026-10-15 §2: "ik wil
//! 100% zeker zijn dat het even snel is als optie f nu"): the search of `f` per
//! keystroke on a made-up catalog of Pandora's size, 13,000 live channels,
//! 47,188 films and 10,589 series, read from JSON in 0.6.1's fields only, so
//! this test is the same before phase 1 and after it. The names are those of
//! `test_vod_timing.rs`. It asserts no speed (a busy machine would fail it):
//! the numbers go to the user.
//!
//! `cargo test --release --test test_f_timing -- --ignored --nocapture`

use bongo_player::args::XtreamCreds;
use bongo_player::catalog::CatalogStore;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::{Catalog, Portal};
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

const FILMS: usize = 47_188;
const SERIES: usize = 10_589;
const LIVE: usize = 13_000;

/// Words for made-up titles: with the numbers, about as long as a portal's.
const WORDS: [&str; 24] = [
    "the", "last", "night", "of", "dark", "matrix", "love", "story", "dead", "city", "secret",
    "man", "woman", "king", "lost", "house", "blood", "river", "star", "war", "pokémon", "zomer",
    "liefde", "nacht",
];

/// A number from `i`, the same on every run.
fn mix(i: usize) -> usize {
    let mut x = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x ^= x >> 29;
    (x.wrapping_mul(0xbf58_476d_1ce4_e5b9) >> 32) as usize
}

fn title_of(i: usize) -> String {
    let n = 2 + mix(i) % 3;
    let words: Vec<&str> = (0..n)
        .map(|k| WORDS[mix(i * 7 + k) % WORDS.len()])
        .collect();
    let mut t = words.join(" ");
    if let Some(first) = t.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    format!("{t} {}", i % 997)
}

/// The catalog, read from the lists as JSON with 0.6.1's fields: what `f`
/// searched before phase 1 kept more of them.
fn catalog() -> Catalog {
    let prefixes = ["", "NL | ", "EN | ", "4K | "];
    let labels = ["", " 1080p", " 4K", " NL SUBS", " [NL]", " HD"];
    let flag = |adult: bool| if adult { "1" } else { "0" };
    let films: Vec<serde_json::Value> = (0..FILMS)
        .map(|i| {
            let base = if i % 10 == 9 { i - 1 } else { i };
            let name = format!(
                "{}{} ({}){}",
                prefixes[mix(base) % 4],
                title_of(base),
                1950 + mix(base) % 77,
                labels[mix(i + 3) % labels.len()]
            );
            let ext = ["mkv", "mp4"][i % 2];
            serde_json::json!({
                "stream_id": 100_000 + i,
                "name": name,
                "container_extension": ext,
                "is_adult": flag(i % 97 == 0)
            })
        })
        .collect();
    let series: Vec<serde_json::Value> = (0..SERIES)
        .map(|i| {
            serde_json::json!({
                "series_id": 1 + i,
                "name": title_of(FILMS + i),
                "is_adult": flag(i % 89 == 0)
            })
        })
        .collect();
    let live: Vec<serde_json::Value> = (0..LIVE)
        .map(|i| {
            serde_json::json!({
                "stream_id": i,
                "name": format!("NL | Kanaal {i} HD"),
                "tv_archive": 0,
                "is_adult": "0"
            })
        })
        .collect();
    Catalog {
        live: serde_json::from_value(serde_json::Value::Array(live)).unwrap(),
        movies: serde_json::from_value(serde_json::Value::Array(films)).unwrap(),
        series: serde_json::from_value(serde_json::Value::Array(series)).unwrap(),
    }
}

#[test]
#[ignore]
fn f_on_a_catalog_of_pandoras_size_timed() {
    let portal: Arc<dyn Portal> = Arc::new(XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h".into(),
        port: 80,
    }));
    let store = CatalogStore::from_catalog(portal, catalog());
    println!("keystroke      | f (best of 5)");
    let typed = "the matrix pokemon";
    let mut worst = 0.0f64;
    for end in 1..=typed.len() {
        let text = &typed[..end];
        let mut found = 0;
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t0 = Instant::now();
            found = black_box(store.search(text)).len();
            best = best.min(t0.elapsed().as_secs_f64() * 1000.0);
        }
        worst = worst.max(best);
        println!("{:<14} | {best:6.2} ms {found:>3}", format!("{text:?}"));
    }
    println!("worst keystroke: f {worst:.2} ms");
}
```

Run: `ct --test test_f_timing`. Expected: it is ignored (1 ignored).

Run it in release, at the base: `cd /c/Dropbox/Sources/tv/rust && CARGO_TARGET_DIR='C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt-thb' cargo test -j 4 --release --test test_f_timing -- --ignored --nocapture`
Expected: a line per keystroke of `"the matrix pokemon"` (best of 5) and `worst keystroke: f … ms`. Write all of it in your report: Task 12 Step 5 compares with it.

Then the base's own release, for the startup A/B's second baseline (Ruling 50): `cd /c/Dropbox/Sources/tv/rust && CARGO_TARGET_DIR='C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt-thb' cargo build --release -j 4`, and keep the exe beside its DLL:

```powershell
$b = Join-Path $env:TEMP 'bongo-baseline-base'
New-Item -ItemType Directory -Force $b | Out-Null
$rel = 'C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt-thb\release'
Copy-Item (Join-Path $rel 'BongoIPTVPlayer.exe'), (Join-Path $rel 'libmpv-2.dll') $b
```

Expected: `%TEMP%\bongo-baseline-base\BongoIPTVPlayer.exe`. Write its path in the report. Nothing is committed yet: the test goes in with Step 10.

- [ ] **Step 1: The baseline**

Run: `ct`, then `cc`, then the old rustfmt hunks of every file but `lib.rs` (it prints only the files that have some): `cd /c/Dropbox/Sources/tv/rust && for f in $(find src tests -name '*.rs' | grep -v '^src/lib.rs$' | sort); do n=$(rustfmt --check --edition 2021 $f 2>&1 | grep -c '^Diff in'); [ "$n" = 0 ] || echo "$f: $n"; done`
Expected: everything passes; clippy shows the 2 old warnings; the hunks are those of the rebased base. At c81d7b0 they were these seven lines (Global Constraints, Formatting), and the zenderlijst and the rename may have moved them:

```
src/app.rs: 7
src/catalog.rs: 3
src/player/mod.rs: 2
src/portal/mod.rs: 1
src/portal/xtream.rs: 1
tests/test_args.rs: 8
tests/test_xtream.rs: 8
```

Note the passing tests, the warnings and the hunks for your report: every later task compares with them, and keeps the hunks as you recorded them.

- [ ] **Step 2: Write the failing tests**

Create `rust/tests/test_vod_portal.rs`:

```rust
//! What the thuisbioscoop takes from an Xtream portal (spec 2026-10-15 §4.1,
//! §4.3): the extra fields of the film and series lists, the two category
//! lists, and a film's and a series' details. Everything comes from a local
//! mock in Xtream's shape, read as tolerantly as portals need.

use bongo_player::args::XtreamCreds;
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::{xtream::XtreamPortal, Category, Movie, Portal, PortalError, Series};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn creds_for(server: &MockServer) -> XtreamCreds {
    let uri = url::Url::parse(&server.uri()).unwrap();
    XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: uri.host_str().unwrap().into(),
        port: uri.port().unwrap_or(80),
    }
}

/// Answers `action` (and, when given, `key`=`value`) with `body`, status 200.
async fn answer(server: &MockServer, action: &str, key: Option<(&str, &str)>, body: &str) {
    let mut mock = Mock::given(method("GET"))
        .and(path("/player_api.php"))
        .and(query_param("action", action));
    if let Some((k, v)) = key {
        mock = mock.and(query_param(k, v));
    }
    mock.respond_with(ResponseTemplate::new(200).set_body_string(body.to_owned()))
        .mount(server)
        .await;
}

/// A portal on a mock that answers the three lists with these bodies.
async fn portal_with_lists(vod: &str, series: &str) -> (MockServer, XtreamPortal) {
    let server = MockServer::start().await;
    answer(&server, "get_live_streams", None, "[]").await;
    answer(&server, "get_vod_streams", None, vod).await;
    answer(&server, "get_series", None, series).await;
    let portal = XtreamPortal::new(creds_for(&server));
    (server, portal)
}

fn movie(json: &str) -> Movie {
    serde_json::from_str(json).unwrap()
}

#[tokio::test]
async fn the_film_list_carries_cover_rating_added_and_category() {
    let vod = r#"[
        {"stream_id":5001,"name":"The Matrix (1999)","container_extension":"mkv","is_adult":"0",
         "stream_icon":"https://image.tmdb.org/t/p/w600/m.jpg","rating":"8.2","added":"1792000000",
         "category_id":"12","plot":"not kept","cast":"not kept"},
        {"stream_id":"5002","name":"Plain","stream_icon":null,"rating":7.3,"added":1792000100,"category_id":13}
    ]"#;
    let (_server, portal) = portal_with_lists(vod, "[]").await;
    let cat = portal.fetch_catalog().await.unwrap();
    let m = &cat.movies[0];
    assert_eq!(
        m.stream_icon.as_deref(),
        Some("https://image.tmdb.org/t/p/w600/m.jpg")
    );
    assert_eq!((m.rating, m.added, m.category_id), (82, 1_792_000_000, 12));
    let plain = &cat.movies[1];
    assert_eq!(plain.stream_icon, None);
    assert_eq!(
        (
            plain.stream_id,
            plain.rating,
            plain.added,
            plain.category_id
        ),
        (5002, 73, 1_792_000_100, 13)
    );
}

#[test]
fn a_rating_is_read_whatever_its_form_and_a_wrong_one_is_none() {
    for (rating, tenths) in [
        (r#""7.3""#, 73),
        ("7.3", 73),
        (r#""7,3""#, 73),
        ("10", 100),
        (r#""7.25""#, 73),
        (r#""""#, 0),
        ("null", 0),
        (r#""0""#, 0),
        (r#""11""#, 0),
        ("-1", 0),
        (r#""n/a""#, 0),
        ("[7]", 0),
    ] {
        let m = movie(&format!(
            r#"{{"stream_id":1,"name":"x","rating":{rating}}}"#
        ));
        assert_eq!(m.rating, tenths, "rating {rating}");
    }
    let none = movie(r#"{"stream_id":1,"name":"x"}"#);
    assert_eq!((none.rating, none.added, none.category_id), (0, 0, 0));
}

#[tokio::test]
async fn the_series_list_carries_cover_rating_release_date_genre_category_and_change_time() {
    let series = r#"[
        {"series_id":4321,"name":"Dark","cover":"https://img.example/dark.jpg","rating":"8.7",
         "releaseDate":"2017-12-01","genre":"Drama, Mystery","category_id":"7","last_modified":"1792001000",
         "plot":"not kept","cast":"not kept"},
        {"series_id":4322,"name":"Old Style","release_date":"2005-03-24","rating":""},
        {"series_id":4323,"name":"Both","releaseDate":"","release_date":"2010-01-01"}
    ]"#;
    let (_server, portal) = portal_with_lists("[]", series).await;
    let cat = portal.fetch_catalog().await.unwrap();
    let dark = &cat.series[0];
    assert_eq!(dark.cover.as_deref(), Some("https://img.example/dark.jpg"));
    assert_eq!(
        (dark.rating, dark.category_id, dark.last_modified),
        (87, 7, 1_792_001_000)
    );
    assert_eq!(dark.released(), Some("2017-12-01"));
    assert_eq!(dark.genre.as_deref(), Some("Drama, Mystery"));
    assert_eq!(cat.series[1].released(), Some("2005-03-24"));
    assert_eq!(cat.series[1].rating, 0);
    // Both names in one series must not fail the list (serde's duplicate
    // field, had they been one field with an alias).
    assert_eq!(cat.series[2].released(), Some("2010-01-01"));
}

#[test]
fn a_series_without_release_date_has_none() {
    let s: Series =
        serde_json::from_str(r#"{"series_id":1,"name":"x","releaseDate":" "}"#).unwrap();
    assert_eq!(s.released(), None);
}

#[tokio::test]
async fn the_category_lists_are_fetched_and_an_odd_entry_is_left_out() {
    let server = MockServer::start().await;
    answer(
        &server,
        "get_vod_categories",
        None,
        r#"[{"category_id":"12","category_name":"NL | Komedie","parent_id":0},"odd",
            {"category_id":13,"category_name":null}]"#,
    )
    .await;
    answer(
        &server,
        "get_series_categories",
        None,
        r#"[{"category_id":"7","category_name":"Series | Drama"}]"#,
    )
    .await;
    let portal = XtreamPortal::new(creds_for(&server));
    let categories = portal.fetch_categories().await.unwrap();
    let named = |id: i64, name: &str| Category {
        category_id: id,
        category_name: name.into(),
    };
    assert_eq!(categories.vod, [named(12, "NL | Komedie"), named(13, "")]);
    assert_eq!(categories.series, [named(7, "Series | Drama")]);
}

#[tokio::test]
async fn a_category_list_that_is_refused_or_no_list_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_categories"))
        .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
        .mount(&server)
        .await;
    let portal = XtreamPortal::new(creds_for(&server));
    assert!(matches!(
        portal.fetch_categories().await,
        Err(PortalError::Status(503))
    ));
    let other = MockServer::start().await;
    answer(&other, "get_vod_categories", None, r#"{"error":"x"}"#).await;
    let portal = XtreamPortal::new(creds_for(&other));
    assert!(matches!(
        portal.fetch_categories().await,
        Err(PortalError::Shape(_))
    ));
}

#[tokio::test]
async fn a_films_details_are_read_from_get_vod_info() {
    let server = MockServer::start().await;
    let body = r#"{"info":{
        "plot":"A hacker learns the truth.","actors":"Keanu Reeves, Carrie-Anne Moss",
        "director":"Lana Wachowski","genre":"Action, Science Fiction","releasedate":"1999-03-30",
        "age":"16","duration_secs":8160,"rating":"8.2","youtube_trailer":"vKQi3bBA1y8",
        "backdrop_path":["https://image.tmdb.org/t/p/w1280/b.jpg","https://x/2.jpg"],"tmdb_id":"603",
        "video":{"width":3840,"height":"2160","color_transfer":"smpte2084"},"audio":{"channels":6}},
        "movie_data":{"stream_id":5001,"container_extension":"mkv"}}"#;
    answer(&server, "get_vod_info", Some(("vod_id", "5001")), body).await;
    let portal = XtreamPortal::new(creds_for(&server));
    let d = portal.fetch_vod_info(5001).await.unwrap();
    assert_eq!(d.plot, "A hacker learns the truth.");
    assert_eq!(d.cast, "Keanu Reeves, Carrie-Anne Moss", "from `actors`");
    assert_eq!(d.director, "Lana Wachowski");
    assert_eq!(d.genre, "Action, Science Fiction");
    assert_eq!((d.released.as_str(), d.year), ("1999-03-30", 1999));
    assert_eq!((d.age.as_str(), d.duration, d.rating), ("16", 8160, 82));
    assert_eq!(d.trailer, "vKQi3bBA1y8");
    assert_eq!(
        d.backdrop, "https://image.tmdb.org/t/p/w1280/b.jpg",
        "the first"
    );
    assert_eq!(d.tmdb, 603);
    assert_eq!(
        (d.width, d.height, d.hdr, d.channels),
        (3840, 2160, true, 6)
    );
    assert!(d.seasons.is_empty() && d.episodes.is_empty());
}

#[tokio::test]
async fn a_films_length_comes_from_its_clock_when_it_has_no_seconds() {
    let server = MockServer::start().await;
    let body = r#"{"info":{"duration":"01:47:12","video":[],"audio":[],"backdrop_path":"https://x/one.jpg"}}"#;
    answer(&server, "get_vod_info", None, body).await;
    let d = XtreamPortal::new(creds_for(&server))
        .fetch_vod_info(1)
        .await
        .unwrap();
    assert_eq!(d.duration, 3600 + 47 * 60 + 12);
    assert_eq!(d.backdrop, "https://x/one.jpg", "one backdrop as a string");
    assert_eq!((d.width, d.height, d.hdr, d.channels), (0, 0, false, 0));
}

#[tokio::test]
async fn an_unknown_film_is_an_error_and_a_refusal_its_status() {
    let server = MockServer::start().await;
    answer(&server, "get_vod_info", Some(("vod_id", "1")), "[]").await;
    Mock::given(query_param("vod_id", "2"))
        .respond_with(ResponseTemplate::new(509))
        .mount(&server)
        .await;
    let portal = XtreamPortal::new(creds_for(&server));
    assert!(matches!(
        portal.fetch_vod_info(1).await,
        Err(PortalError::Shape(_))
    ));
    assert!(matches!(
        portal.fetch_vod_info(2).await,
        Err(PortalError::Status(509))
    ));
}

#[tokio::test]
async fn a_series_details_hold_its_seasons_and_episodes_in_order() {
    let server = MockServer::start().await;
    let body = r#"{
        "seasons":[{"season_number":2,"name":"Season 2","cover":"https://x/s2.jpg"},
                   {"season_number":1,"name":"Season 1","cover_big":"https://x/s1.jpg"},
                   {"season_number":0,"name":"Specials"}],
        "info":{"name":"Dark","plot":"Kids vanish.","cast":"Louis Hofmann","genre":"Drama",
                "releaseDate":"2017-12-01","rating":"8.7","episode_run_time":"55",
                "backdrop_path":["https://x/back.jpg"],"youtube_trailer":"rrwycJ08PSA"},
        "episodes":{
          "2":[{"id":"987655","episode_num":"1","title":"Beginnings","container_extension":"mkv",
                "info":{"duration_secs":2820,"plot":"Two.","movie_image":"https://x/e21.jpg"},"added":"1792089400"}],
          "1":[{"id":"987651","episode_num":2,"title":"Lies","container_extension":"mp4","info":{"duration":"00:47:00"}},
               {"id":"987650","episode_num":1,"title":"Secrets","container_extension":"mp4","info":[]},
               {"episode_num":3,"title":"no id: not playable"}]
        }}"#;
    answer(
        &server,
        "get_series_info",
        Some(("series_id", "4321")),
        body,
    )
    .await;
    let portal = XtreamPortal::new(creds_for(&server));
    let d = portal.fetch_series_info(4321).await.unwrap();
    assert_eq!(
        (d.plot.as_str(), d.cast.as_str()),
        ("Kids vanish.", "Louis Hofmann")
    );
    assert_eq!((d.year, d.rating, d.duration), (2017, 87, 55 * 60));
    assert_eq!(d.backdrop, "https://x/back.jpg");
    assert_eq!(d.trailer, "rrwycJ08PSA");
    let seasons: Vec<(u32, &str, &str)> = d
        .seasons
        .iter()
        .map(|s| (s.number, s.name.as_str(), s.cover.as_str()))
        .collect();
    assert_eq!(
        seasons,
        [
            (0, "Specials", ""),
            (1, "Season 1", "https://x/s1.jpg"),
            (2, "Season 2", "https://x/s2.jpg")
        ]
    );
    let episodes: Vec<(&str, u32, u32, &str, &str, u32)> = d
        .episodes
        .iter()
        .map(|e| {
            let (id, title, ext) = (e.id.as_str(), e.title.as_str(), e.ext.as_str());
            (id, e.season, e.number, title, ext, e.duration)
        })
        .collect();
    assert_eq!(
        episodes,
        [
            ("987650", 1, 1, "Secrets", "mp4", 0),
            ("987651", 1, 2, "Lies", "mp4", 47 * 60),
            ("987655", 2, 1, "Beginnings", "mkv", 2820),
        ]
    );
    let last = &d.episodes[2];
    assert_eq!(
        (last.plot.as_str(), last.still.as_str()),
        ("Two.", "https://x/e21.jpg")
    );
    assert_eq!(last.added, 1_792_089_400);
}

#[tokio::test]
async fn episodes_in_lists_and_a_series_without_info_are_read_too() {
    let server = MockServer::start().await;
    let body = r#"{"episodes":[[{"id":11,"episode_num":1,"season":1,"title":"A"}],
                                [{"id":"21","episode_num":1,"season":"2","title":"B"}]]}"#;
    answer(&server, "get_series_info", None, body).await;
    let d = XtreamPortal::new(creds_for(&server))
        .fetch_series_info(9)
        .await
        .unwrap();
    let got: Vec<(&str, u32, u32)> = d
        .episodes
        .iter()
        .map(|e| (e.id.as_str(), e.season, e.number))
        .collect();
    assert_eq!(got, [("11", 1, 1), ("21", 2, 1)]);
    assert!(d.plot.is_empty() && d.seasons.is_empty());
}

#[tokio::test]
async fn an_image_address_with_the_account_in_it_is_dropped() {
    let server = MockServer::start().await;
    let body = r#"{"info":{"backdrop_path":"http://h:80/movie/u/p/5001.jpg","plot":"kept"}}"#;
    answer(&server, "get_vod_info", None, body).await;
    let d = XtreamPortal::new(creds_for(&server))
        .fetch_vod_info(5001)
        .await
        .unwrap();
    assert_eq!(d.backdrop, "", "never kept, never shown");
    assert_eq!(d.plot, "kept");
}

#[tokio::test]
async fn an_m3u_list_has_no_categories_and_no_details() {
    let (portal, _) =
        M3uPortal::from_text("#EXTM3U\n#EXTINF:-1,Een\nhttp://a.example/1\n").unwrap();
    let categories = portal.fetch_categories().await.unwrap();
    assert!(categories.vod.is_empty() && categories.series.is_empty());
    assert!(matches!(
        portal.fetch_vod_info(1).await,
        Err(PortalError::NoDetails)
    ));
    assert!(matches!(
        portal.fetch_series_info(1).await,
        Err(PortalError::NoDetails)
    ));
}

#[tokio::test]
async fn nulls_and_odd_types_in_a_details_answer_leave_fields_empty() {
    let server = MockServer::start().await;
    let series = r#"{"info":null,"seasons":null,
        "episodes":{"1":null,"2":[null,{"id":null},{"id":5,"episode_num":"x","title":7,"info":"none"}]}}"#;
    answer(&server, "get_series_info", None, series).await;
    let film = r#"{"info":{"plot":null,"rating":{},"duration_secs":"12x","video":"none","audio":null,
        "backdrop_path":[null,""],"tmdb_id":"n/a"}}"#;
    answer(&server, "get_vod_info", None, film).await;
    let portal = XtreamPortal::new(creds_for(&server));
    let d = portal.fetch_series_info(1).await.unwrap();
    assert!(d.plot.is_empty() && d.seasons.is_empty());
    let got: Vec<(&str, u32, u32, &str)> = d
        .episodes
        .iter()
        .map(|e| (e.id.as_str(), e.season, e.number, e.title.as_str()))
        .collect();
    assert_eq!(
        got,
        [("5", 2, 0, "7")],
        "an episode without an id plays nothing"
    );
    let f = portal.fetch_vod_info(1).await.unwrap();
    assert_eq!(f, bongo_player::portal::VodDetails::default());
}
```

- [ ] **Step 3: Run them to see them fail**

Run: `ct --test test_vod_portal`
Expected: compile errors: `Movie` has no field `stream_icon`, there is no `Category`, and `fetch_categories` is no method of `XtreamPortal`.

- [ ] **Step 4: The types**

In `rust/src/portal/types.rs`, replace the structs `Movie` and `Series` (from `Movie`'s derive line to the closing `}` of `Series`) with the richer ones, `Series::released`, the category types and the details types. Replace

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Movie {
    #[serde(deserialize_with = "deser_id")]
    pub stream_id: i64,
    pub name: String,
    #[serde(default)]
    pub container_extension: Option<String>,
    /// The portal's adult flag (`is_adult`): the only thing that marks adult
    /// content (see `catalog::adult_live`).
    #[serde(default, deserialize_with = "deser_flag")]
    pub is_adult: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Series {
    #[serde(deserialize_with = "deser_id")]
    pub series_id: i64,
    pub name: String,
    /// The portal's adult flag (`is_adult`): the only thing that marks adult
    /// content (see `catalog::adult_live`).
    #[serde(default, deserialize_with = "deser_flag")]
    pub is_adult: bool,
}

```

with

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Movie {
    #[serde(deserialize_with = "deser_id")]
    pub stream_id: i64,
    pub name: String,
    #[serde(default)]
    pub container_extension: Option<String>,
    /// The portal's adult flag (`is_adult`): the only thing that marks adult
    /// content (see `catalog::adult_live`).
    #[serde(default, deserialize_with = "deser_flag")]
    pub is_adult: bool,
    /// The cover (`stream_icon`). The title index takes it when the catalog
    /// is stored: a stored catalog's films have none.
    #[serde(default, deserialize_with = "deser_opt_string")]
    pub stream_icon: Option<String>,
    /// The portal's rating in tenths (73: ★ 7.3); 0: none.
    #[serde(default, deserialize_with = "deser_rating")]
    pub rating: u8,
    /// When the portal added it, unix seconds; 0: unknown.
    #[serde(default, deserialize_with = "deser_i64_or_zero")]
    pub added: i64,
    /// Its category (`get_vod_categories`); 0: none.
    #[serde(default, deserialize_with = "deser_i64_or_zero")]
    pub category_id: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Series {
    #[serde(deserialize_with = "deser_id")]
    pub series_id: i64,
    pub name: String,
    /// The portal's adult flag (`is_adult`): the only thing that marks adult
    /// content (see `catalog::adult_live`).
    #[serde(default, deserialize_with = "deser_flag")]
    pub is_adult: bool,
    /// The cover. The title index takes it, as a film's.
    #[serde(default, deserialize_with = "deser_opt_string")]
    pub cover: Option<String>,
    /// The portal's rating in tenths; 0: none.
    #[serde(default, deserialize_with = "deser_rating")]
    pub rating: u8,
    /// `releaseDate` ("2019-05-03"); see [`Series::released`]. The title index
    /// takes it.
    #[serde(default, rename = "releaseDate", deserialize_with = "deser_opt_string")]
    pub release_date_camel: Option<String>,
    /// `release_date`, which some portals send instead, or as well: a field of
    /// its own, since a serde alias fails the whole list on a series that has
    /// both (a duplicate field, as `start` and `start_timestamp` once did to
    /// the EPG). The title index takes it.
    #[serde(default, deserialize_with = "deser_opt_string")]
    pub release_date: Option<String>,
    /// The genres as the portal names them ("Drama, Comedy"). The title index
    /// takes it.
    #[serde(default, deserialize_with = "deser_opt_string")]
    pub genre: Option<String>,
    /// Its category (`get_series_categories`); 0: none.
    #[serde(default, deserialize_with = "deser_i64_or_zero")]
    pub category_id: i64,
    /// When the portal last changed it, unix seconds; 0: unknown.
    #[serde(default, deserialize_with = "deser_i64_or_zero")]
    pub last_modified: i64,
}

impl Series {
    /// Its release date: `releaseDate`, else `release_date`; none when both are
    /// missing or empty.
    pub fn released(&self) -> Option<&str> {
        [&self.release_date_camel, &self.release_date]
            .into_iter()
            .flatten()
            .map(|s| s.trim())
            .find(|s| !s.is_empty())
    }
}

/// One of the portal's categories (`get_vod_categories`,
/// `get_series_categories`): its id and name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    #[serde(default, deserialize_with = "deser_i64_or_zero")]
    pub category_id: i64,
    #[serde(default, deserialize_with = "deser_string_or_empty")]
    pub category_name: String,
}

/// The portal's categories of films and of series: two id spaces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Categories {
    pub vod: Vec<Category>,
    pub series: Vec<Category>,
}

/// What a film's or a series' page shows that the lists do not hold
/// (`get_vod_info`, `get_series_info`; spec 2026-10-15 §4.3): only those
/// fields, and no image address with an account in it. `vod::details` keeps
/// it as a file per title.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VodDetails {
    pub plot: String,
    pub cast: String,
    pub director: String,
    /// The genres as the portal names them.
    pub genre: String,
    /// The release date as the portal gives it ("1999-03-30").
    pub released: String,
    /// The year of `released`; 0: unknown.
    pub year: u16,
    /// The age rating as the portal gives it ("16", "PG-13").
    pub age: String,
    /// A film's length, a series' usual episode length: seconds; 0: unknown.
    pub duration: u32,
    /// ★ in tenths; 0: none.
    pub rating: u8,
    /// The YouTube code (`youtube_trailer`) as the portal gives it; the
    /// trailer checks it before it is used.
    pub trailer: String,
    /// The backdrop image (`backdrop_path`, the first one).
    pub backdrop: String,
    /// The id at TMDB; 0: none.
    pub tmdb: i64,
    /// The picture as the portal measured it: pixels, and whether it is HDR.
    pub width: u32,
    pub height: u32,
    pub hdr: bool,
    /// Sound channels: 6 is 5.1; 0: unknown.
    pub channels: u8,
    /// A series' seasons, by number; a film has none.
    pub seasons: Vec<SeasonInfo>,
    /// A series' episodes, by season and number; a film has none.
    pub episodes: Vec<EpisodeInfo>,
}

/// A season of a series: its number (0: the extras), name and cover.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeasonInfo {
    pub number: u32,
    pub name: String,
    pub cover: String,
}

/// An episode of a series, as its page lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EpisodeInfo {
    /// Its id as the portal gives it: what `Portal::series_stream_url` plays.
    pub id: String,
    pub season: u32,
    pub number: u32,
    pub title: String,
    /// Its container (`container_extension`): "mkv", "mp4".
    pub ext: String,
    /// Seconds; 0: unknown.
    pub duration: u32,
    pub plot: String,
    /// Its still (`movie_image`).
    pub still: String,
    /// When the portal added it, unix seconds; 0: unknown.
    pub added: i64,
}

```

In `rust/src/portal/types.rs`, the three tolerant readers and the two helpers they share, before `pub struct LiveChannel`. Before

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveChannel {
```

add:

```rust
/// Tolerant string: a JSON string as it is; anything else (null, a number, a
/// missing field) is empty.
fn deser_string_or_empty<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(deser_opt_string(d)?.unwrap_or_default())
}

/// Tolerant whole number: see [`whole_number`].
fn deser_i64_or_zero<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = Option::<serde_json::Value>::deserialize(d).unwrap_or(None);
    Ok(v.as_ref().map_or(0, whole_number))
}

/// A whole number as portals send one: a JSON number (a fraction is cut off)
/// or a numeric string; 0 for anything else (null, "", a word).
pub(crate) fn whole_number(v: &serde_json::Value) -> i64 {
    match v {
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        serde_json::Value::String(s) => s.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

/// A rating as portals send one ("7.3", 7.3, "7,3", "" or null) in tenths, 0
/// to 100; 0 (no rating) for anything else, and for a value outside 0 to 10.
pub(crate) fn rating_tenths(v: &serde_json::Value) -> u8 {
    let x = match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().replace(',', ".").parse::<f64>().ok(),
        _ => None,
    };
    match x {
        Some(x) if x.is_finite() && (0.0..=10.0).contains(&x) => (x * 10.0).round() as u8,
        _ => 0,
    }
}

/// Tolerant rating: see [`rating_tenths`].
fn deser_rating<'de, D>(d: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = Option::<serde_json::Value>::deserialize(d).unwrap_or(None);
    Ok(v.as_ref().map_or(0, rating_tenths))
}

```

- [ ] **Step 5: The trait and the details reader**

In `rust/src/portal/mod.rs`, declare the details reader with the other modules. Replace

```rust
pub mod m3u;
pub mod types;
pub mod xtream;
```

with

```rust
pub mod m3u;
pub mod types;
pub mod vod_info;
pub mod xtream;
```

In `rust/src/portal/mod.rs`, at the end of `enum PortalError`, add the variant of a source without details. Replace

```rust
    /// An M3U file that cannot be read: its path and the system's words.
    #[error("cannot read {0}")]
    File(String),
}
```

with

```rust
    /// An M3U file that cannot be read: its path and the system's words.
    #[error("cannot read {0}")]
    File(String),
    /// This source has no films or series to tell about (an M3U list).
    #[error("no film or series details on this source")]
    NoDetails,
}
```

In `rust/src/portal/mod.rs`, in `trait Portal`, three methods with defaults (an M3U list and the tests' portals keep compiling as they are): after the line `async fn fetch_series_episodes(&self, series_id: i64) -> Result<Vec<Episode>, PortalError>;`, add:

```rust
    /// The portal's categories of films and of series (spec 2026-10-15 §4.1),
    /// fetched in the catalog's round, beside its lists. A source without
    /// films or series has none.
    async fn fetch_categories(&self) -> Result<Categories, PortalError> {
        Ok(Categories::default())
    }
    /// What a film's page shows (`get_vod_info`). A source without films has
    /// nothing to tell.
    async fn fetch_vod_info(&self, _vod_id: i64) -> Result<VodDetails, PortalError> {
        Err(PortalError::NoDetails)
    }
    /// What a series' page shows: its info, seasons and episodes
    /// (`get_series_info`). A source without series has nothing to tell.
    async fn fetch_series_info(&self, _series_id: i64) -> Result<VodDetails, PortalError> {
        Err(PortalError::NoDetails)
    }
```

Create `rust/src/portal/vod_info.rs`:

```rust
//! A film's and a series' details as Xtream portals send them
//! (`get_vod_info`, `get_series_info`), read into [`VodDetails`]: only what a
//! page shows. Portals differ in names and types (a number as a string, one
//! backdrop or a list of them, episodes by season or as lists), so every field
//! is read tolerantly and an odd one is left empty, never an error. An image
//! address that `redact` would change holds an account: it is dropped.

use super::types::{rating_tenths, whole_number, EpisodeInfo, SeasonInfo, VodDetails};
use super::PortalError;
use serde_json::Value;

/// A film's details from its `get_vod_info` answer. An answer without an
/// `info` object (portals answer `[]` for an id they do not know) is an error.
pub fn parse_vod_info(answer: &Value) -> Result<VodDetails, PortalError> {
    let info = answer
        .get("info")
        .filter(|i| i.is_object())
        .ok_or_else(|| PortalError::Shape("no film info".into()))?;
    let mut details = common(info);
    details.duration = seconds(info);
    if let Some(video) = info.get("video") {
        details.width = clamp_u32(whole_number(video.get("width").unwrap_or(&Value::Null)));
        details.height = clamp_u32(whole_number(video.get("height").unwrap_or(&Value::Null)));
        let transfer = text(video, &["color_transfer"]).to_ascii_lowercase();
        details.hdr = matches!(transfer.as_str(), "smpte2084" | "arib-std-b67");
    }
    if let Some(audio) = info.get("audio") {
        let channels = whole_number(audio.get("channels").unwrap_or(&Value::Null));
        details.channels = channels.clamp(0, u8::MAX as i64) as u8;
    }
    Ok(details)
}

/// A series' details from its `get_series_info` answer: its info, seasons and
/// episodes. An answer that is no object is an error; one without info, seasons
/// or episodes is a series with none of them.
pub fn parse_series_info(answer: &Value) -> Result<VodDetails, PortalError> {
    if !answer.is_object() {
        return Err(PortalError::Shape("no series info".into()));
    }
    let info = answer.get("info").filter(|i| i.is_object());
    let mut details = info.map(common).unwrap_or_default();
    if let Some(info) = info {
        // A series names its usual episode length in minutes.
        let minutes = whole_number(info.get("episode_run_time").unwrap_or(&Value::Null));
        details.duration = clamp_u32(minutes.saturating_mul(60));
    }
    details.seasons = answer
        .get("seasons")
        .and_then(Value::as_array)
        .map(|seasons| seasons.iter().filter_map(season).collect())
        .unwrap_or_default();
    details.seasons.sort_by_key(|s| s.number);
    details.seasons.dedup_by_key(|s| s.number);
    details.episodes = episodes(answer.get("episodes"));
    details.episodes.sort_by_key(|e| (e.season, e.number));
    Ok(details)
}

/// What films and series share in their `info`.
fn common(info: &Value) -> VodDetails {
    let released = text(info, &["releasedate", "release_date", "releaseDate"]);
    VodDetails {
        plot: text(info, &["plot", "description"]),
        cast: text(info, &["cast", "actors"]),
        director: text(info, &["director"]),
        genre: text(info, &["genre"]),
        year: year_of(&released),
        released,
        age: text(info, &["age", "mpaa_rating"]),
        rating: info.get("rating").map_or(0, rating_tenths),
        trailer: text(info, &["youtube_trailer", "trailer"]),
        backdrop: image(info, &["backdrop_path", "backdrop"]),
        tmdb: info.get("tmdb_id").map_or(0, whole_number),
        ..VodDetails::default()
    }
}

/// A season from the `seasons` list; none for anything that is no object.
fn season(v: &Value) -> Option<SeasonInfo> {
    v.is_object().then(|| SeasonInfo {
        number: clamp_u32(number(v, &["season_number", "number"])),
        name: text(v, &["name"]),
        cover: image(v, &["cover", "cover_big"]),
    })
}

/// The episodes of `episodes`: an object of lists keyed by season number (the
/// common shape) or a list of lists. An episode that names its season keeps
/// it; else it is in the season of its key.
fn episodes(episodes: Option<&Value>) -> Vec<EpisodeInfo> {
    let lists: Vec<(u32, &Value)> = match episodes {
        Some(Value::Object(by_season)) => by_season
            .iter()
            .map(|(key, list)| (key.trim().parse().unwrap_or(0), list))
            .collect(),
        Some(Value::Array(lists)) => lists.iter().map(|list| (0, list)).collect(),
        _ => Vec::new(),
    };
    lists
        .into_iter()
        .filter_map(|(season, list)| Some((season, list.as_array()?)))
        .flat_map(|(season, list)| list.iter().filter_map(move |e| episode(e, season)))
        .collect()
}

/// An episode; none without an id, which is what plays it.
fn episode(e: &Value, season_of_key: u32) -> Option<EpisodeInfo> {
    let id = text(e, &["id"]);
    if id.is_empty() {
        return None;
    }
    let info = e.get("info").unwrap_or(&Value::Null);
    let season = clamp_u32(number(e, &["season"]));
    Some(EpisodeInfo {
        id,
        season: if season > 0 { season } else { season_of_key },
        number: clamp_u32(number(e, &["episode_num"])),
        title: text(e, &["title"]),
        ext: text(e, &["container_extension"]),
        duration: seconds(info),
        plot: text(info, &["plot", "description"]),
        still: image(info, &["movie_image"]),
        added: e.get("added").map_or(0, whole_number),
    })
}

/// Seconds from `duration_secs`, else from `duration` ("01:47:12").
fn seconds(info: &Value) -> u32 {
    let secs = number(info, &["duration_secs"]);
    if secs > 0 {
        return clamp_u32(secs);
    }
    let clock = text(info, &["duration"]);
    let parts: Vec<i64> = clock
        .split(':')
        .map(|p| p.trim().parse::<i64>().unwrap_or(-1))
        .collect();
    match parts.as_slice() {
        [h, m, s] if *h >= 0 && *m >= 0 && *s >= 0 => clamp_u32(h * 3600 + m * 60 + s),
        [m, s] if *m >= 0 && *s >= 0 => clamp_u32(m * 60 + s),
        _ => 0,
    }
}

/// The first of `keys` that holds a string with text in it (or a number),
/// trimmed; empty when none does.
fn text(v: &Value, keys: &[&str]) -> String {
    keys.iter()
        .filter_map(|k| match v.get(*k)? {
            Value::String(s) => Some(s.trim().to_owned()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

/// The first of `keys` with a whole number in it; 0 when none has.
fn number(v: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .filter_map(|k| v.get(*k))
        .map(whole_number)
        .find(|n| *n != 0)
        .unwrap_or(0)
}

/// An image address from the first of `keys` that has one: a string, or the
/// first string of a list. Dropped when `redact` would change it: it holds an
/// account, and the details are kept in a file.
fn image(v: &Value, keys: &[&str]) -> String {
    let found = keys.iter().find_map(|k| match v.get(*k)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Array(list) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map(str::to_owned),
        _ => None,
    });
    match found {
        Some(url) if crate::redact::redact(&url) == url.as_str() => url,
        _ => String::new(),
    }
}

/// The year a date starts with ("1999-03-30", "1999"); 0 when it starts with
/// none from 1900 to 2100.
pub fn year_of(date: &str) -> u16 {
    let digits: String = date.trim().chars().take(4).collect();
    match digits.parse::<u16>() {
        Ok(y) if (1900..=2100).contains(&y) && digits.len() == 4 => y,
        _ => 0,
    }
}

fn clamp_u32(n: i64) -> u32 {
    n.clamp(0, u32::MAX as i64) as u32
}
```

- [ ] **Step 6: Xtream's side**

In `rust/src/portal/xtream.rs`, in `impl XtreamPortal` (the first one, with `new`), after `fn base`, add two helpers. After

```rust
    fn base(&self) -> String {
        format!("http://{}:{}", self.creds.host, self.creds.port)
    }
```

add:

```rust

    /// The JSON answer to an API call; a refusal is its status. Its error
    /// names no address: the URL holds the account, and the error is logged.
    async fn api_json(&self, params: &[(&str, &str)]) -> Result<serde_json::Value, PortalError> {
        let no_url = |e: reqwest::Error| PortalError::Http(e.without_url());
        let url = self.api_url(params);
        let answer = self.client.get(url).send().await.map_err(no_url)?;
        if !answer.status().is_success() {
            return Err(PortalError::Status(answer.status().as_u16()));
        }
        answer.json().await.map_err(no_url)
    }

    /// One of the two category lists. An entry that is no object is left out;
    /// an answer that is no list is an error.
    async fn category_list(&self, action: &str) -> Result<Vec<Category>, PortalError> {
        let answer = self.api_json(&[("action", action)]).await?;
        let list = answer
            .as_array()
            .ok_or_else(|| PortalError::Shape(format!("{action}: no list")))?;
        Ok(list
            .iter()
            .filter(|v| v.is_object())
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect())
    }
```

In `rust/src/portal/xtream.rs`, in `impl Portal for XtreamPortal`, between `fetch_series_episodes` and `live_stream_url`, add the three new methods. Replace

```rust
        Ok(out)
    }

    fn live_stream_url(&self, stream_id: i64) -> String {
```

with

```rust
        Ok(out)
    }

    async fn fetch_categories(&self) -> Result<Categories, PortalError> {
        Ok(Categories {
            vod: self.category_list("get_vod_categories").await?,
            series: self.category_list("get_series_categories").await?,
        })
    }

    async fn fetch_vod_info(&self, vod_id: i64) -> Result<VodDetails, PortalError> {
        let id = vod_id.to_string();
        let answer = self
            .api_json(&[("action", "get_vod_info"), ("vod_id", &id)])
            .await?;
        super::vod_info::parse_vod_info(&answer)
    }

    async fn fetch_series_info(&self, series_id: i64) -> Result<VodDetails, PortalError> {
        let id = series_id.to_string();
        let answer = self
            .api_json(&[("action", "get_series_info"), ("series_id", &id)])
            .await?;
        super::vod_info::parse_series_info(&answer)
    }

    fn live_stream_url(&self, stream_id: i64) -> String {
```

- [ ] **Step 7: The two literals that name every field**

In `rust/src/app.rs`, in the tests' `fn library_catalog`, the film and the series of the catalog take the new fields' defaults. Replace

```rust
                container_extension: Some("mp4".into()),
                is_adult: false,
            }],
            series: vec![crate::portal::Series {
                series_id: 71,
                name: "Zebra Serie".into(),
                is_adult: false,
            }],
```

with

```rust
                container_extension: Some("mp4".into()),
                is_adult: false,
                ..Default::default()
            }],
            series: vec![crate::portal::Series {
                series_id: 71,
                name: "Zebra Serie".into(),
                is_adult: false,
                ..Default::default()
            }],
```

In `rust/tests/test_catalog_adult.rs`, in `fn movie`, the new fields take their defaults. Replace

```rust
        container_extension: Some("mp4".into()),
        is_adult: adult,
    }
}
```

with

```rust
        container_extension: Some("mp4".into()),
        is_adult: adult,
        ..Movie::default()
    }
}
```

In `rust/tests/test_catalog_adult.rs`, in `fn series`, likewise. Replace

```rust
        series_id: id,
        name: name.into(),
        is_adult: adult,
    }
}
```

with

```rust
        series_id: id,
        name: name.into(),
        is_adult: adult,
        ..Series::default()
    }
}
```

- [ ] **Step 8: Run the tests to see them pass**

Run: `ct --test test_vod_portal --test test_catalog_adult --test test_xtream --test test_m3u_portal`
Expected: all pass (14 in `test_vod_portal`); `fetch_catalog_combines_all_three_lists` still asks for the three lists only.

- [ ] **Step 9: Format, lint, full suite**

- Run `fmt src/portal/types.rs src/portal/vod_info.rs tests/test_vod_portal.rs tests/test_catalog_adult.rs tests/test_f_timing.rs`. Expected: no output.
- Run `hunks src/portal/xtream.rs`. Expected: the count Task 1 Step 1 recorded (`1` at c81d7b0).
- Run `hunks src/portal/mod.rs`. Expected: the count Task 1 Step 1 recorded (`1` at c81d7b0).
- Run `hunks src/app.rs`. Expected: the count Task 1 Step 1 recorded (`7` at c81d7b0).
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 10: Commit**

Message file (in the scratchpad):
```
feat(rust): the thuisbioscoop's lists: covers, ratings, add days and categories of films and series, the two category lists, and the details through the portal; f timed before phase 1

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/portal/types.rs rust/src/portal/vod_info.rs rust/src/portal/mod.rs rust/src/portal/xtream.rs rust/src/app.rs rust/tests/test_catalog_adult.rs rust/tests/test_vod_portal.rs rust/tests/test_f_timing.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/portal/types.rs rust/src/portal/vod_info.rs rust/src/portal/mod.rs rust/src/portal/xtream.rs rust/src/app.rs rust/tests/test_catalog_adult.rs rust/tests/test_vod_portal.rs rust/tests/test_f_timing.rs`

---

### Task 2a: Names: the fold and the 24 genres

The first half of what a name says (spec §4.2), pure and with no I/O: the fold the search compares with, and the fixed list of 24 genres with their names in `i18n.rs` and the aliases portals use. Task 2b adds the labels, the year of a name and the versions.

**Files:**
- Create: `rust/src/vod/mod.rs`, `rust/src/vod/genres.rs` (the genres, up to `genres_of`)
- Modify: `rust/src/lib.rs` (`pub mod vod;` after `pub mod stream_check;`), `rust/src/i18n.rs` (the genres' names)
- Test: `rust/tests/test_vod_genres.rs` (new; 5 tests)

**Interfaces:**
- Consumes: `crate::i18n::T`.
- Produces:
  ```rust
  // vod/mod.rs
  pub enum Kind { Series, Film }                    // Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord
  pub struct TitleId { pub kind: Kind, pub id: i64 } // the same; a film by its default version's stream id
  pub fn fold(text: &str) -> String;
  pub fn fold_into(text: &str, out: &mut String);
  // vod/genres.rs
  pub enum Genre { Action, Adventure, Animation, Anime, Biography, Christmas, Comedy, Crime, Documentary,
      Drama, Family, Fantasy, History, Horror, Kids, Music, Mystery, Reality, Romance, ScienceFiction,
      Sport, Thriller, War, Western }                // Debug, Clone, Copy, PartialEq, Eq, Hash
  impl Genre { pub const ALL: [Genre; 24]; pub fn bit(self) -> u32; pub fn text(self) -> T;
      pub fn aliases(self) -> &'static [&'static str]; }
  pub fn genres_in(text: &str) -> u32;
  pub fn genres_of(mask: u32) -> impl Iterator<Item = Genre>;
  // i18n.rs
  T::GenreAction, T::GenreAdventure, … T::GenreWestern   // one per Genre
  ```

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_vod_genres.rs`:

```rust
//! What names say (vod/genres.rs, spec 2026-10-15 §4.2): the fold the search
//! compares with, and the 24 genres and their aliases.

use bongo_player::i18n::{t, Lang};
use bongo_player::vod::fold;
use bongo_player::vod::genres::{genres_in, genres_of, Genre};

#[test]
fn fold_lowers_the_case_drops_accents_and_makes_punctuation_one_space() {
    assert_eq!(fold("Pokémon: De Film!"), "pokemon de film");
    assert_eq!(fold("  Amélie  "), "amelie");
    assert_eq!(fold("Straße"), "strasse");
    assert_eq!(fold("Ça va? Ærø"), "ca va aero");
    assert_eq!(fold("Poke\u{301}mon"), "pokemon", "a combining accent");
    assert_eq!(fold("Spider-Man: No Way Home"), "spider man no way home");
    assert_eq!(fold("Пётр"), "пётр", "another script stays, in lower case");
    assert_eq!(fold("--"), "");
}

#[test]
fn there_are_24_genres_each_named_in_every_language() {
    assert_eq!(Genre::ALL.len(), 24);
    let all = Genre::ALL.iter().fold(0u32, |mask, g| {
        assert_eq!(mask & g.bit(), 0, "{g:?} shares a bit");
        mask | g.bit()
    });
    assert_eq!(all.count_ones(), 24);
    for g in Genre::ALL {
        for lang in Lang::ALL {
            assert!(!t(lang, g.text()).is_empty(), "{g:?} in {lang:?}");
        }
    }
    assert_eq!(t(Lang::Nl, Genre::Comedy.text()), "Komedie");
    assert_eq!(t(Lang::De, Genre::Comedy.text()), "Komödie");
}

#[test]
fn comedy_komedie_and_komoedie_are_one_genre() {
    let comedy = Genre::Comedy.bit();
    assert_eq!(genres_in("Comedy"), comedy);
    assert_eq!(genres_in("Komedie"), comedy);
    assert_eq!(genres_in("Komödie"), comedy);
}

#[test]
fn every_alias_is_written_folded_and_names_its_genre() {
    for g in Genre::ALL {
        for alias in g.aliases() {
            assert_eq!(fold(alias), *alias, "{g:?}: write {alias:?} folded");
            assert_ne!(genres_in(alias) & g.bit(), 0, "{g:?}: {alias:?}");
        }
    }
}

#[test]
fn a_category_or_genre_field_names_its_genres_as_whole_words() {
    use Genre::*;
    let mask = |gs: &[Genre]| gs.iter().fold(0, |m, g| m | g.bit());
    assert_eq!(genres_in("NL | Komedie"), mask(&[Comedy]));
    assert_eq!(genres_in("Action & Adventure"), mask(&[Action, Adventure]));
    assert_eq!(
        genres_in("Sci-Fi & Fantasy"),
        mask(&[ScienceFiction, Fantasy])
    );
    assert_eq!(genres_in("Drama, Mystery"), mask(&[Drama, Mystery]));
    assert_eq!(genres_in("Kids & Familie"), mask(&[Kids, Family]));
    assert_eq!(genres_in("Horror/Thriller"), mask(&[Horror, Thriller]));
    assert_eq!(genres_in("Star Wars"), 0, "no war in wars");
    assert_eq!(genres_in("NL | Nieuw"), 0);
    assert_eq!(genres_in(""), 0);
    let named: Vec<Genre> = genres_of(mask(&[Western, Action])).collect();
    assert_eq!(named, [Action, Western], "in the fixed order");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_vod_genres`
Expected: compile errors: there is no module `vod`.

- [ ] **Step 3: The module, the fold and the genres**

Create `rust/src/vod/mod.rs`:

```rust
//! The thuisbioscoop's data (spec 2026-10-15 §7): titles, genres, versions,
//! details, covers and what was watched. It draws nothing.

pub mod genres;

/// What a title is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Series,
    Film,
}

/// A title of the thuisbioscoop: a series by its `series_id`, a film by the
/// `stream_id` of its default version (`versions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TitleId {
    pub kind: Kind,
    pub id: i64,
}

/// `text` as the search compares it: lower case, accents off ("Pokémon" is
/// "pokemon", "ß" is "ss"), every run of other characters one space, no space
/// at either end. Letters of other scripts stay, in lower case.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    fold_into(text, &mut out);
    out
}

/// [`fold`], appended to `out`.
pub fn fold_into(text: &str, out: &mut String) {
    let start = out.len();
    let mut space = true;
    for c in text.chars() {
        if c.is_ascii() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
                space = false;
            } else if !space {
                out.push(' ');
                space = true;
            }
            continue;
        }
        for lower in c.to_lowercase() {
            match plain(lower) {
                Some(ascii) => {
                    out.push_str(ascii);
                    space = false;
                }
                None if lower.is_alphanumeric() => {
                    out.push(lower);
                    space = false;
                }
                // A combining accent (a decomposed é) belongs to the letter
                // before it: it adds nothing.
                None if is_combining(lower) => {}
                None if !space => {
                    out.push(' ');
                    space = true;
                }
                None => {}
            }
        }
    }
    if out.len() > start && out.ends_with(' ') {
        out.pop();
    }
}

/// The plain letters of a lower-case letter with an accent; none for any other.
fn plain(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' | 'ĝ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' | 'ț' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// Unicode's combining diacritical marks (U+0300 to U+036F).
fn is_combining(c: char) -> bool {
    ('\u{0300}'..='\u{036f}').contains(&c)
}
```

Create `rust/src/vod/genres.rs`:

```rust
//! What the names of titles and categories say (spec 2026-10-15 §4.2): the
//! fixed list of 24 genres with the words portals use for them, the language
//! markers, the quality and language labels of a film's version, and the year
//! at the end of a name.
//!
//! Everything is compared [`fold`]ed (lower case, no accents) and as whole
//! words. Labels and language markers count only where labels stand: a prefix
//! before the last `|`, a prefix before ` - ` that holds nothing else, a group
//! in brackets, and the words at the end; in a category's name also the words
//! it starts with ("NL FILMS"). So the "en" of "Romeo en Julia" is Dutch for
//! "and", no English marker, and the "Dual" of "Dual (2022)" is its title.

use super::fold;
use crate::i18n::T;

/// One of the fixed genres. Its name is an i18n text ([`Genre::text`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Genre {
    Action,
    Adventure,
    Animation,
    Anime,
    Biography,
    Christmas,
    Comedy,
    Crime,
    Documentary,
    Drama,
    Family,
    Fantasy,
    History,
    Horror,
    Kids,
    Music,
    Mystery,
    Reality,
    Romance,
    ScienceFiction,
    Sport,
    Thriller,
    War,
    Western,
}

impl Genre {
    /// Every genre, in the order of their bits.
    pub const ALL: [Genre; 24] = [
        Genre::Action,
        Genre::Adventure,
        Genre::Animation,
        Genre::Anime,
        Genre::Biography,
        Genre::Christmas,
        Genre::Comedy,
        Genre::Crime,
        Genre::Documentary,
        Genre::Drama,
        Genre::Family,
        Genre::Fantasy,
        Genre::History,
        Genre::Horror,
        Genre::Kids,
        Genre::Music,
        Genre::Mystery,
        Genre::Reality,
        Genre::Romance,
        Genre::ScienceFiction,
        Genre::Sport,
        Genre::Thriller,
        Genre::War,
        Genre::Western,
    ];

    /// Its bit in a genre mask.
    pub fn bit(self) -> u32 {
        1 << self as u32
    }

    /// Its name, in the user's language.
    pub fn text(self) -> T {
        match self {
            Genre::Action => T::GenreAction,
            Genre::Adventure => T::GenreAdventure,
            Genre::Animation => T::GenreAnimation,
            Genre::Anime => T::GenreAnime,
            Genre::Biography => T::GenreBiography,
            Genre::Christmas => T::GenreChristmas,
            Genre::Comedy => T::GenreComedy,
            Genre::Crime => T::GenreCrime,
            Genre::Documentary => T::GenreDocumentary,
            Genre::Drama => T::GenreDrama,
            Genre::Family => T::GenreFamily,
            Genre::Fantasy => T::GenreFantasy,
            Genre::History => T::GenreHistory,
            Genre::Horror => T::GenreHorror,
            Genre::Kids => T::GenreKids,
            Genre::Music => T::GenreMusic,
            Genre::Mystery => T::GenreMystery,
            Genre::Reality => T::GenreReality,
            Genre::Romance => T::GenreRomance,
            Genre::ScienceFiction => T::GenreScienceFiction,
            Genre::Sport => T::GenreSport,
            Genre::Thriller => T::GenreThriller,
            Genre::War => T::GenreWar,
            Genre::Western => T::GenreWestern,
        }
    }

    /// The words portals use for it, folded: Dutch, English and German, and
    /// the plurals they use.
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Genre::Action => &[
                "action",
                "actie",
                "actiefilm",
                "actiefilms",
                "actionfilm",
                "actionfilme",
                "martial arts",
                "superhero",
                "superheroes",
                "superheld",
                "superhelden",
                "marvel",
            ],
            Genre::Adventure => &[
                "adventure",
                "adventures",
                "avontuur",
                "avonturen",
                "abenteuer",
            ],
            Genre::Animation => &[
                "animation",
                "animatie",
                "animated",
                "animatiefilm",
                "animatiefilms",
                "tekenfilm",
                "tekenfilms",
                "cartoon",
                "cartoons",
                "zeichentrick",
                "pixar",
            ],
            Genre::Anime => &["anime"],
            Genre::Biography => &["biography", "biografie", "biographie", "biopic", "biopics"],
            Genre::Christmas => &[
                "christmas",
                "kerst",
                "kerstmis",
                "kerstfilm",
                "kerstfilms",
                "weihnachten",
                "weihnachtsfilm",
                "xmas",
                "x mas",
            ],
            Genre::Comedy => &[
                "comedy", "comedies", "komedie", "komedies", "komodie", "komodien", "comedie",
                "humor", "stand up", "standup", "cabaret", "kabarett",
            ],
            Genre::Crime => &[
                "crime",
                "misdaad",
                "krimi",
                "gangster",
                "detective",
                "detectives",
            ],
            Genre::Documentary => &[
                "documentary",
                "documentaries",
                "documentaire",
                "documentaires",
                "docu",
                "docus",
                "dokumentation",
                "dokumentarfilm",
                "doku",
            ],
            Genre::Drama => &["drama", "dramas"],
            Genre::Family => &[
                "family",
                "familie",
                "familien",
                "familiefilm",
                "familiefilms",
                "familienfilm",
            ],
            Genre::Fantasy => &["fantasy"],
            Genre::History => &[
                "history",
                "historical",
                "historisch",
                "historie",
                "geschiedenis",
                "geschichte",
            ],
            Genre::Horror => &["horror", "horrorfilm", "horrorfilms", "horrorfilme"],
            Genre::Kids => &[
                "kids",
                "kinderen",
                "kinder",
                "kinderfilm",
                "kinderfilms",
                "kinderfilme",
                "kinderserie",
                "kinderseries",
                "jeugd",
                "children",
            ],
            Genre::Music => &[
                "music",
                "muziek",
                "musik",
                "musical",
                "musicals",
                "concert",
                "concerten",
                "konzert",
            ],
            Genre::Mystery => &["mystery", "mysterie", "mysteries"],
            Genre::Reality => &["reality", "realityshow", "reality tv"],
            Genre::Romance => &[
                "romance",
                "romantiek",
                "romantisch",
                "romantische",
                "romantic",
                "romantik",
                "romcom",
            ],
            Genre::ScienceFiction => {
                &["science fiction", "sciencefiction", "sci fi", "scifi", "sf"]
            }
            Genre::Sport => &["sport", "sports", "sporten"],
            Genre::Thriller => &["thriller", "thrillers", "suspense", "spanning"],
            Genre::War => &[
                "war",
                "oorlog",
                "oorlogsfilm",
                "oorlogsfilms",
                "krieg",
                "kriegsfilm",
            ],
            Genre::Western => &["western", "westerns"],
        }
    }
}

/// The genres named anywhere in `text` (a series' genre field, a category's
/// name) as a mask of [`Genre::bit`]s: every alias found as whole words.
pub fn genres_in(text: &str) -> u32 {
    let padded = format!(" {} ", fold(text));
    let mut mask = 0;
    for genre in Genre::ALL {
        let named = genre.aliases().iter().any(|alias| {
            padded.match_indices(alias).any(|(at, _)| {
                padded.as_bytes()[at - 1] == b' ' && padded.as_bytes()[at + alias.len()] == b' '
            })
        });
        if named {
            mask |= genre.bit();
        }
    }
    mask
}

/// The genres of a mask, in the fixed order.
pub fn genres_of(mask: u32) -> impl Iterator<Item = Genre> {
    Genre::ALL.into_iter().filter(move |g| mask & g.bit() != 0)
}
```

In `rust/src/lib.rs`, declare the module, in order: after the line `pub mod stream_check;`, add:

```rust
pub mod vod;
```

In `rust/src/i18n.rs`, at the end of the `texts!` table, after the line of `NewsPatternsHelp`, add the genres' names. After

```rust
    NewsPatternsHelp => ["* = willekeurige tekst   ;   ! = uitsluiten   ;   ; = scheiding", "* = any text   ;   ! = exclude   ;   ; = separator", "* = beliebiger Text   ;   ! = ausschließen   ;   ; = Trennung"],
```

add:

```rust
    // The thuisbioscoop's genres (vod/genres.rs)
    GenreAction => ["Actie", "Action", "Action"],
    GenreAdventure => ["Avontuur", "Adventure", "Abenteuer"],
    GenreAnimation => ["Animatie", "Animation", "Animation"],
    GenreAnime => ["Anime", "Anime", "Anime"],
    GenreBiography => ["Biografie", "Biography", "Biografie"],
    GenreChristmas => ["Kerst", "Christmas", "Weihnachten"],
    GenreComedy => ["Komedie", "Comedy", "Komödie"],
    GenreCrime => ["Misdaad", "Crime", "Krimi"],
    GenreDocumentary => ["Documentaire", "Documentary", "Dokumentation"],
    GenreDrama => ["Drama", "Drama", "Drama"],
    GenreFamily => ["Familie", "Family", "Familie"],
    GenreFantasy => ["Fantasy", "Fantasy", "Fantasy"],
    GenreHistory => ["Geschiedenis", "History", "Geschichte"],
    GenreHorror => ["Horror", "Horror", "Horror"],
    GenreKids => ["Kinderen", "Kids", "Kinder"],
    GenreMusic => ["Muziek", "Music", "Musik"],
    GenreMystery => ["Mysterie", "Mystery", "Mystery"],
    GenreReality => ["Reality", "Reality", "Reality"],
    GenreRomance => ["Romantiek", "Romance", "Romantik"],
    GenreScienceFiction => ["Sciencefiction", "Science fiction", "Science-Fiction"],
    GenreSport => ["Sport", "Sport", "Sport"],
    GenreThriller => ["Thriller", "Thriller", "Thriller"],
    GenreWar => ["Oorlog", "War", "Krieg"],
    GenreWestern => ["Western", "Western", "Western"],
```

The i18n tests (`every_translation_keeps_the_same_placeholders`, `no_text_is_empty`) cover the new keys.

- [ ] **Step 4: Run the tests to see them pass**

Run: `ct --test test_vod_genres`
Expected: all pass (5).

- [ ] **Step 5: Format, lint, full suite**

- Run `fmt src/vod/mod.rs src/i18n.rs tests/test_vod_genres.rs`. Expected: no output.
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 6: Commit**

Message file (in the scratchpad):
```
feat(rust): what the thuisbioscoop's names say, part 1: the fold and the 24 genres with their aliases

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/vod/mod.rs rust/src/vod/genres.rs rust/src/lib.rs rust/src/i18n.rs rust/tests/test_vod_genres.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/vod/mod.rs rust/src/vod/genres.rs rust/src/lib.rs rust/src/i18n.rs rust/tests/test_vod_genres.rs`

---

### Task 2b: Names: the labels, the year of a name and the versions

The rest of what a name says (spec §4.2), pure and with no I/O: the language markers and the quality and language labels where labels stand, the label words a category's name starts with, the year at the end of a name, and films merged into titles with versions. `parse_name` reads ~58,000 names per catalog: it reuses a thread-local scratch, so it allocates nothing once warm (Ruling 17).

**Files:**
- Create: `rust/src/vod/versions.rs`
- Modify: `rust/src/vod/genres.rs` (from `Labels` to `category_labels`), `rust/src/vod/mod.rs` (`pub mod versions;`)
- Test: `rust/tests/test_vod_genres.rs` (6 more), `rust/tests/test_vod_versions.rs` (new)

**Interfaces:**
- Consumes: Task 2a's `fold`, `fold_into`; `crate::portal::Movie` (Task 1).
- Produces:
  ```rust
  // vod/genres.rs
  pub struct Labels(pub u16);                       // Debug, Clone, Copy, PartialEq, Eq, Hash, Default
  impl Labels { pub const UHD, FHD, HD, SD, HDR, THREE_D, NL, NL_SUBS, MULTI, DUAL, EN: Labels;
      pub fn has(self, label: Labels) -> bool; pub fn with(self, label: Labels) -> Labels;
      pub fn rank(self) -> u8; pub fn language(self) -> Language; pub fn text(self) -> String; }
  pub enum Language { Unknown /* default */, Dutch, English }
  pub struct NameParts<'a> { pub title: &'a str, pub year: u16, pub labels: Labels }
  pub fn parse_name(name: &str, max_year: u16) -> NameParts<'_>;
  pub fn category_labels(name: &str) -> Labels;     // also the label words a name starts with ("NL FILMS")
  // vod/versions.rs
  pub struct FilmVersion { pub index: u32, pub labels: Labels }
  pub struct FilmTitle<'a> { pub title: &'a str, pub key: String, pub year: u16, pub versions: Vec<FilmVersion> }
  pub fn merge(movies: &[Movie], max_year: u16) -> Vec<FilmTitle<'_>>;
  ```

- [ ] **Step 1: Write the failing tests**

In `rust/tests/test_vod_genres.rs`, its doc and imports: what the names say, and the labels and years. Replace

```rust
//! What names say (vod/genres.rs, spec 2026-10-15 §4.2): the fold the search
//! compares with, and the 24 genres and their aliases.

use bongo_player::i18n::{t, Lang};
use bongo_player::vod::fold;
use bongo_player::vod::genres::{genres_in, genres_of, Genre};

```

with

```rust
//! What names say (vod/genres.rs, spec 2026-10-15 §4.2): the fold the search
//! compares with, the 24 genres and their aliases, the language markers, the
//! labels of a version and the year of a name.

use bongo_player::i18n::{t, Lang};
use bongo_player::vod::fold;
use bongo_player::vod::genres::{genres_in, genres_of, parse_name, Genre, Labels, Language};

```

In `rust/tests/test_vod_genres.rs`, at the end of the file, add the tests of names, labels and years:

```rust
/// The latest year a name may give in these tests: 2026's next.
const MAX_YEAR: u16 = 2027;

/// The title, year and labels `parse_name` reads from `name`.
fn parsed(name: &str) -> (&str, u16, Labels) {
    let p = parse_name(name, MAX_YEAR);
    (p.title, p.year, p.labels)
}

fn labels(ls: &[Labels]) -> Labels {
    ls.iter().fold(Labels::default(), |all, l| all.with(*l))
}

#[test]
fn a_name_gives_its_title_year_and_labels() {
    use bongo_player::vod::genres::Labels as L;
    let cases: &[(&str, &str, u16, Labels)] = &[
        (
            "NL | The Matrix (1999) [4K HDR]",
            "The Matrix",
            1999,
            labels(&[L::NL, L::UHD, L::HDR]),
        ),
        ("The Matrix 1999 1080p", "The Matrix", 1999, L::FHD),
        ("4K - The Matrix", "The Matrix", 0, L::UHD),
        (
            "Mission: Impossible - Fallout (2018)",
            "Mission: Impossible - Fallout",
            2018,
            L::default(),
        ),
        (
            "Toy Story 4 (2019) NL Gesproken",
            "Toy Story 4",
            2019,
            L::NL,
        ),
        ("The Matrix NL SUBS", "The Matrix", 0, L::NL_SUBS),
        (
            "Pokémon: De Film (2000) [NL]",
            "Pokémon: De Film",
            2000,
            L::NL,
        ),
        ("EN | Breaking Bad", "Breaking Bad", 0, L::EN),
        ("Frozen II 2019", "Frozen II", 2019, L::default()),
        ("The Movie (Extended) 4K", "The Movie (Extended)", 0, L::UHD),
        ("Title –", "Title", 0, L::default()),
    ];
    for (name, title, year, ls) in cases {
        assert_eq!(parsed(name), (*title, *year, *ls), "{name}");
    }
}

#[test]
fn a_word_that_is_a_title_stays_a_title() {
    assert_eq!(parsed("Dual (2022)"), ("Dual", 2022, Labels::default()));
    assert_eq!(parsed("1917"), ("1917", 0, Labels::default()));
    assert_eq!(parsed("1917 (2019)"), ("1917", 2019, Labels::default()));
    assert_eq!(
        parsed("Romeo en Julia"),
        ("Romeo en Julia", 0, Labels::default()),
        "en is Dutch for and"
    );
    assert_eq!(
        parsed("Blade Runner 2049"),
        ("Blade Runner 2049", 0, Labels::default()),
        "a year to come is no year"
    );
    assert_eq!(parsed(""), ("", 0, Labels::default()));
}

#[test]
fn labels_rank_as_channel_qualities_and_say_the_language() {
    assert_eq!(Labels::UHD.rank(), 4);
    assert_eq!(Labels::FHD.rank(), 3);
    assert_eq!(Labels::HD.rank(), 2);
    assert_eq!(
        Labels::default().rank(),
        1,
        "unmarked, as quality_rank has it"
    );
    assert_eq!(Labels::SD.rank(), 0);
    assert_eq!(labels(&[Labels::UHD, Labels::SD]).rank(), 4);
    assert_eq!(Labels::NL.language(), Language::Dutch);
    assert_eq!(Labels::EN.language(), Language::English);
    assert_eq!(
        labels(&[Labels::NL, Labels::EN]).language(),
        Language::Dutch,
        "Dutch first"
    );
    assert_eq!(
        Labels::NL_SUBS.language(),
        Language::Unknown,
        "Dutch subtitles, not Dutch speech"
    );
}

#[test]
fn labels_are_named_as_the_version_choice_shows_them() {
    assert_eq!(labels(&[Labels::UHD, Labels::HDR]).text(), "4K HDR");
    assert_eq!(
        labels(&[Labels::FHD, Labels::NL_SUBS]).text(),
        "1080p NL SUBS"
    );
    assert_eq!(Labels::NL.text(), "NL");
    assert_eq!(Labels::default().text(), "");
}

#[test]
fn odd_names_never_panic_and_keep_a_title() {
    // Portals send what they send; the release build aborts on a panic.
    for name in [
        "Amélie\u{a0}(2001)",
        "Film (2001",
        "[4K Film",
        "Title –",
        "NL |",
        "|",
        "4K",
        "(2001)",
        "🎬 Film 🎬 (2020)",
        "A\u{301}",
        "((()))",
        "NL | (2020) [4K]",
    ] {
        let p = parse_name(name, MAX_YEAR);
        assert!(!p.title.is_empty(), "{name:?}: {p:?}");
    }
    assert_eq!(
        parsed("Amélie\u{a0}(2001)"),
        ("Amélie", 2001, Labels::default()),
        "a no-break space"
    );
    assert_eq!(
        parsed("Film (2001"),
        ("Film", 2001, Labels::default()),
        "an unclosed bracket is punctuation"
    );
    assert_eq!(
        parsed("4K"),
        ("4K", 0, Labels::default()),
        "a name of one label is its title"
    );
    assert_eq!(parsed("  "), ("", 0, Labels::default()));
}

#[test]
fn a_category_named_by_labels_alone_says_its_language() {
    use bongo_player::vod::genres::category_labels;
    assert_eq!(category_labels("Nederlands").language(), Language::Dutch);
    assert_eq!(category_labels("NL Gesproken").language(), Language::Dutch);
    assert_eq!(category_labels("NL | Komedie").language(), Language::Dutch);
    assert_eq!(category_labels("English").language(), Language::English);
    // A section named after its language, without a `|`.
    assert_eq!(category_labels("NL FILMS").language(), Language::Dutch);
    assert_eq!(
        category_labels("NL Gesproken Kids").language(),
        Language::Dutch
    );
    assert_eq!(category_labels("UK Series").language(), Language::English);
    assert_eq!(
        category_labels("Films en Series").language(),
        Language::Unknown
    );
    assert_eq!(category_labels("Kids").language(), Language::Unknown);
}
```

Create `rust/tests/test_vod_versions.rs`:

```rust
//! Films merged into titles with versions (vod/versions.rs, spec 2026-10-15
//! §4.2): one key per name and year, the highest quality the default.

use bongo_player::portal::Movie;
use bongo_player::vod::genres::Labels;
use bongo_player::vod::versions::{merge, FilmTitle};

fn film(stream_id: i64, name: &str) -> Movie {
    Movie {
        stream_id,
        name: name.into(),
        ..Movie::default()
    }
}

/// Each title's text, year and the stream ids of its versions, default first.
fn shape<'a>(movies: &'a [Movie], titles: &[FilmTitle<'a>]) -> Vec<(&'a str, u16, Vec<i64>)> {
    titles
        .iter()
        .map(|t| {
            let ids = t
                .versions
                .iter()
                .map(|v| movies[v.index as usize].stream_id)
                .collect();
            (t.title, t.year, ids)
        })
        .collect()
}

#[test]
fn the_versions_of_a_film_are_one_title_with_the_highest_quality_first() {
    let movies = [
        film(1, "The Matrix (1999) 1080p"),
        film(2, "NL | The Matrix (1999) [4K]"),
        film(3, "The Matrix (1999) NL SUBS"),
        film(4, "The Matrix Reloaded (2003)"),
    ];
    let titles = merge(&movies, 2027);
    assert_eq!(
        shape(&movies, &titles),
        [
            ("The Matrix", 1999, vec![2, 1, 3]),
            ("The Matrix Reloaded", 2003, vec![4])
        ]
    );
    assert_eq!(titles[0].key, "the matrix");
    let first = titles[0].versions[0].labels;
    assert!(first.has(Labels::UHD) && first.has(Labels::NL));
    assert!(titles[0].versions[2].labels.has(Labels::NL_SUBS));
}

#[test]
fn another_year_is_another_title_and_no_year_is_one_too() {
    let movies = [
        film(1, "Dune (1984)"),
        film(2, "Dune (2021)"),
        film(3, "Dune"),
    ];
    let titles = merge(&movies, 2027);
    let years: Vec<u16> = titles.iter().map(|t| t.year).collect();
    assert_eq!(years, [1984, 2021, 0]);
}

#[test]
fn a_tie_in_quality_goes_to_hdr_then_to_the_lowest_stream_id() {
    let movies = [
        film(9, "X (2020) 4K"),
        film(5, "X (2020) 4K"),
        film(7, "X (2020) 4K HDR"),
    ];
    let titles = merge(&movies, 2027);
    assert_eq!(shape(&movies, &titles), [("X", 2020, vec![7, 5, 9])]);
}

#[test]
fn names_that_differ_in_case_accents_or_punctuation_are_one_title() {
    let movies = [
        film(1, "Amélie (2001)"),
        film(2, "AMELIE (2001) HD"),
        film(3, "Spider-Man (2002)"),
        film(4, "Spider Man (2002)"),
    ];
    let titles = merge(&movies, 2027);
    assert_eq!(
        shape(&movies, &titles),
        [
            ("AMELIE", 2001, vec![2, 1]),
            ("Spider-Man", 2002, vec![3, 4])
        ],
        "the title shown is the default version's, as its name has it"
    );
}

#[test]
fn titles_keep_the_order_of_their_first_film() {
    let movies = [
        film(1, "B (2001)"),
        film(2, "A (2001)"),
        film(3, "B (2001) 4K"),
    ];
    let titles = merge(&movies, 2027);
    let keys: Vec<&str> = titles.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(keys, ["b", "a"]);
}

#[test]
fn no_films_no_titles() {
    assert!(merge(&[], 2027).is_empty());
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_vod_genres --test test_vod_versions`
Expected: compile errors: there is no `parse_name`, `Labels` or module `vod::versions`.

- [ ] **Step 3: The labels, the year and the versions**

In `rust/src/vod/genres.rs`, the imports: the labels read names with `fold_into` too. Replace

```rust
use super::fold;
```

with

```rust
use super::{fold, fold_into};
```

In `rust/src/vod/genres.rs`, at the end of the file, after `genres_of`, add the labels, the language, the year of a name and `category_labels`:

```rust
/// The quality and language labels of a name: a set of bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Labels(pub u16);

impl Labels {
    /// 4K, UHD, 2160p.
    pub const UHD: Labels = Labels(1);
    /// 1080p, FHD, Full HD.
    pub const FHD: Labels = Labels(1 << 1);
    /// 720p, HD.
    pub const HD: Labels = Labels(1 << 2);
    pub const SD: Labels = Labels(1 << 3);
    pub const HDR: Labels = Labels(1 << 4);
    pub const THREE_D: Labels = Labels(1 << 5);
    /// Dutch: NL, Nederlands, Dutch, NL Gesproken.
    pub const NL: Labels = Labels(1 << 6);
    /// Dutch subtitles: no Dutch marker.
    pub const NL_SUBS: Labels = Labels(1 << 7);
    pub const MULTI: Labels = Labels(1 << 8);
    pub const DUAL: Labels = Labels(1 << 9);
    /// English: EN, English, UK.
    pub const EN: Labels = Labels(1 << 10);

    pub fn has(self, label: Labels) -> bool {
        self.0 & label.0 == label.0 && label.0 != 0
    }

    pub fn with(self, label: Labels) -> Labels {
        Labels(self.0 | label.0)
    }

    /// Its quality tier, as `catalog::quality_rank` ranks a channel's: UHD 4,
    /// FHD 3, HD 2, none 1, SD 0.
    pub fn rank(self) -> u8 {
        if self.has(Labels::UHD) {
            4
        } else if self.has(Labels::FHD) {
            3
        } else if self.has(Labels::HD) {
            2
        } else if self.has(Labels::SD) {
            0
        } else {
            1
        }
    }

    /// The language the labels say: Dutch over English (the spec names it
    /// first), else unknown.
    pub fn language(self) -> Language {
        if self.has(Labels::NL) {
            Language::Dutch
        } else if self.has(Labels::EN) {
            Language::English
        } else {
            Language::Unknown
        }
    }

    /// How a version's choice names it: "4K HDR", "1080p NL SUBS"; empty
    /// without labels. These tokens are the same in every language.
    pub fn text(self) -> String {
        let quality = match self.rank() {
            4 => Some("4K"),
            3 => Some("1080p"),
            2 => Some("720p"),
            0 => Some("SD"),
            _ => None,
        };
        let rest = [
            (Labels::HDR, "HDR"),
            (Labels::THREE_D, "3D"),
            (Labels::NL, "NL"),
            (Labels::NL_SUBS, "NL SUBS"),
            (Labels::EN, "EN"),
            (Labels::MULTI, "MULTI"),
            (Labels::DUAL, "DUAL"),
        ];
        quality
            .into_iter()
            .chain(rest.iter().filter(|(l, _)| self.has(*l)).map(|(_, t)| *t))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The spoken language of a title, when its labels say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Language {
    #[default]
    Unknown,
    Dutch,
    English,
}

/// The labels, folded, as phrases of one or two words: the two-word ones are
/// tried first.
const LABELS: &[(&str, Labels)] = &[
    ("nl gesproken", Labels::NL),
    ("nederlands gesproken", Labels::NL),
    ("nl subs", Labels::NL_SUBS),
    ("nl sub", Labels::NL_SUBS),
    ("full hd", Labels::FHD),
    ("ultra hd", Labels::UHD),
    ("dolby vision", Labels::HDR),
    ("multi audio", Labels::MULTI),
    ("multi sub", Labels::MULTI),
    ("multi subs", Labels::MULTI),
    ("dual audio", Labels::DUAL),
    ("4k", Labels::UHD),
    ("uhd", Labels::UHD),
    ("2160p", Labels::UHD),
    ("2160", Labels::UHD),
    ("1080p", Labels::FHD),
    ("1080", Labels::FHD),
    ("fhd", Labels::FHD),
    ("720p", Labels::HD),
    ("720", Labels::HD),
    ("hd", Labels::HD),
    ("sd", Labels::SD),
    ("480p", Labels::SD),
    ("576p", Labels::SD),
    ("hdr", Labels::HDR),
    ("hdr10", Labels::HDR),
    ("3d", Labels::THREE_D),
    ("nl", Labels::NL),
    ("nederlands", Labels::NL),
    ("dutch", Labels::NL),
    ("nlsubs", Labels::NL_SUBS),
    ("nlsub", Labels::NL_SUBS),
    ("multi", Labels::MULTI),
    ("multisub", Labels::MULTI),
    ("dual", Labels::DUAL),
    ("en", Labels::EN),
    ("english", Labels::EN),
    ("uk", Labels::EN),
];

/// The label a folded word or phrase is.
fn label_of(phrase: &str) -> Option<Labels> {
    LABELS.iter().find(|(w, _)| *w == phrase).map(|(_, l)| *l)
}

/// The label the folded words `a b` are together, without building the phrase.
fn pair_of(a: &str, b: &str) -> Option<Labels> {
    LABELS
        .iter()
        .find(|(w, _)| {
            w.len() == a.len() + 1 + b.len()
                && w.starts_with(a)
                && w.as_bytes()[a.len()] == b' '
                && w.ends_with(b)
        })
        .map(|(_, l)| *l)
}

/// The labels among the words of `folded` (a folded text), two-word labels
/// first. With `every`: none unless every word is one, and none for no words.
fn labels_of_words(folded: &str, every: bool) -> Option<Labels> {
    let mut labels = Labels::default();
    let mut words = folded.split(' ').filter(|w| !w.is_empty()).peekable();
    let mut any = false;
    while let Some(w) = words.next() {
        any = true;
        if let Some(l) = words.peek().and_then(|next| pair_of(w, next)) {
            labels = labels.with(l);
            words.next();
        } else if let Some(l) = label_of(w) {
            labels = labels.with(l);
        } else if every {
            return None;
        }
    }
    (any || !every).then_some(labels)
}

/// A year from 1900 to `max_year` written as four digits; none for anything
/// else.
fn as_year(word: &str, max_year: u16) -> Option<u16> {
    if word.len() != 4 || !word.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: u16 = word.parse().ok()?;
    (1900..=max_year).contains(&year).then_some(year)
}

/// What a title's name says beside its title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameParts<'a> {
    /// The name without its prefix, and without the labels and the year at its
    /// end: what a tile shows. At least one word stays.
    pub title: &'a str,
    /// "(2024)" anywhere, else " 2024" at the end (before the labels); 0 when
    /// neither, or when it is later than `max_year`.
    pub year: u16,
    pub labels: Labels,
}

/// What a group in brackets holds.
#[derive(Clone, Copy)]
enum Group {
    Year(u16),
    Labels(Labels),
    Other,
}

/// A piece of a name: a word, or a group in brackets; where it starts and
/// ends in the name.
#[derive(Clone, Copy)]
struct Token {
    start: usize,
    end: usize,
    group: Option<Group>,
}

/// What `parse_name` reuses from one name to the next: ~58,000 names are read
/// per catalog, and none of them allocates once these have grown.
#[derive(Default)]
struct Scratch {
    tokens: Vec<Token>,
    last: String,
    before: String,
}

thread_local! {
    static SCRATCH: std::cell::RefCell<Scratch> = std::cell::RefCell::new(Scratch::default());
}

/// Reads a film's, a series' or a category's name: the prefix before its last
/// `|` (whatever it holds), a prefix before ` - ` that is labels only, the
/// groups in brackets that hold a year or labels only, and the labels and the
/// year at its end. `max_year` is the latest year a name may give, the year
/// after this one: "Blade Runner 2049" is no film of 2049.
pub fn parse_name(name: &str, max_year: u16) -> NameParts<'_> {
    SCRATCH.with(|s| read_name(name, max_year, &mut s.borrow_mut()))
}

fn read_name<'a>(name: &'a str, max_year: u16, s: &mut Scratch) -> NameParts<'a> {
    let mut labels = Labels::default();
    let mut year = 0u16;
    let mut rest = name.trim();
    if let Some((before, after)) = rest.rsplit_once('|') {
        if !after.trim().is_empty() {
            s.before.clear();
            fold_into(before, &mut s.before);
            labels = labels.with(labels_of_words(&s.before, false).unwrap_or_default());
            rest = after.trim();
        }
    }
    if let Some((before, after)) = rest.split_once(" - ") {
        s.before.clear();
        fold_into(before, &mut s.before);
        if !after.trim().is_empty() {
            if let Some(found) = labels_of_words(&s.before, true) {
                labels = labels.with(found);
                rest = after.trim();
            }
        }
    }
    tokens(rest, max_year, s);
    // A group anywhere gives its year or its labels.
    for t in &s.tokens {
        match t.group {
            Some(Group::Year(y)) if year == 0 => year = y,
            Some(Group::Labels(l)) => labels = labels.with(l),
            _ => {}
        }
    }
    // From the end: groups with a year or labels, label words, punctuation and
    // one bare year go; the first word always stays.
    while s.tokens.len() > 1 {
        let last = s.tokens[s.tokens.len() - 1];
        match last.group {
            Some(Group::Year(_) | Group::Labels(_)) => {
                s.tokens.pop();
                continue;
            }
            Some(Group::Other) => break,
            None => {}
        }
        s.last.clear();
        fold_into(&rest[last.start..last.end], &mut s.last);
        if s.last.is_empty() {
            s.tokens.pop();
            continue;
        }
        let before = s.tokens[s.tokens.len() - 2];
        if s.tokens.len() > 2 && before.group.is_none() {
            s.before.clear();
            fold_into(&rest[before.start..before.end], &mut s.before);
            if let Some(l) = pair_of(&s.before, &s.last) {
                labels = labels.with(l);
                s.tokens.truncate(s.tokens.len() - 2);
                continue;
            }
        }
        if let Some(l) = label_of(&s.last) {
            labels = labels.with(l);
            s.tokens.pop();
            continue;
        }
        match as_year(&s.last, max_year) {
            Some(y) if year == 0 => {
                year = y;
                s.tokens.pop();
            }
            _ => break,
        }
    }
    let end = s.tokens.last().map_or(rest.len(), |t| t.end);
    let title = rest[..end].trim_end_matches([' ', '-', ':', '|', '.', ',', '_']);
    NameParts {
        title: if title.is_empty() { rest } else { title },
        year,
        labels,
    }
}

/// The words and bracket groups of `text`, in order, into `s.tokens`.
fn tokens(text: &str, max_year: u16, s: &mut Scratch) {
    s.tokens.clear();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let Some(c) = text[i..].chars().next() else {
            break;
        };
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let close = match bytes[i] {
            b'(' => Some(b')'),
            b'[' => Some(b']'),
            _ => None,
        };
        if let Some(close) = close {
            if let Some(len) = text[i + 1..].bytes().position(|b| b == close) {
                let end = i + len + 2;
                s.last.clear();
                fold_into(&text[i + 1..i + 1 + len], &mut s.last);
                let group = group_of(&s.last, max_year);
                s.tokens.push(Token {
                    start: i,
                    end,
                    group: Some(group),
                });
                i = end;
                continue;
            }
        }
        // A word runs to the next space or bracket; an unclosed bracket is a
        // word of its own.
        let len = match text[i..].find(|c: char| c.is_whitespace() || c == '(' || c == '[') {
            Some(0) => c.len_utf8(),
            Some(n) => n,
            None => text.len() - i,
        };
        s.tokens.push(Token {
            start: i,
            end: i + len,
            group: None,
        });
        i += len;
    }
}

/// What a group in brackets holds (`folded`: what is inside, folded): a year
/// alone, labels only, or else.
fn group_of(folded: &str, max_year: u16) -> Group {
    if let Some(y) = as_year(folded, max_year) {
        return Group::Year(y);
    }
    match labels_of_words(folded, true) {
        Some(l) => Group::Labels(l),
        None => Group::Other,
    }
}

/// The labels of a category's name: those [`parse_name`] reads, and the label
/// words the name starts with ("Nederlands", "NL FILMS", "NL Gesproken Kids"):
/// a portal names a section after its language, with or without a `|`. A
/// title's name of one label keeps it as its title. A category's year means
/// nothing.
pub fn category_labels(name: &str) -> Labels {
    let mut labels = parse_name(name, u16::MAX).labels;
    let folded = fold(name);
    let mut words = folded.split(' ').filter(|w| !w.is_empty()).peekable();
    while let Some(w) = words.next() {
        if let Some(l) = words.peek().and_then(|next| pair_of(w, next)) {
            labels = labels.with(l);
            words.next();
        } else if let Some(l) = label_of(w) {
            labels = labels.with(l);
        } else {
            break;
        }
    }
    labels
}
```

In `rust/src/vod/mod.rs`, declare the versions: after the line `pub mod genres;`, add:

```rust
pub mod versions;
```

Create `rust/src/vod/versions.rs`:

```rust
//! Films merged into titles with versions (spec 2026-10-15 §4.2). A portal
//! lists one film many times: in 4K and in 1080p, spoken Dutch or with Dutch
//! subtitles. They are one title when their names, without prefix, labels and
//! year, fold to the same words and they give the same year. The highest
//! quality is the default version, as `catalog::quality_rank` picks a
//! channel's variant; a film page offers the others. Series are not merged.

use super::fold_into;
use super::genres::{parse_name, Labels};
use crate::portal::Movie;
use std::collections::HashMap;
use std::fmt::Write as _;

/// A version of a film title: the film's position in the list, and its labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilmVersion {
    pub index: u32,
    pub labels: Labels,
}

/// A film title: the films of one name and year.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilmTitle<'a> {
    /// What the tile shows: the default version's name without prefix, labels
    /// and year.
    pub title: &'a str,
    /// The folded title (`vod::fold`): what the versions share, and what the
    /// search compares.
    pub key: String,
    /// The year the names give; 0: none.
    pub year: u16,
    /// Its versions, the default one first: the highest quality tier, HDR
    /// before none, then the lowest stream id (the same default on every load).
    pub versions: Vec<FilmVersion>,
}

/// `movies` merged into titles, in the order of each title's first film.
/// `max_year` is the latest year a name may give (`genres::parse_name`).
pub fn merge(movies: &[Movie], max_year: u16) -> Vec<FilmTitle<'_>> {
    let mut titles: Vec<FilmTitle<'_>> = Vec::with_capacity(movies.len());
    // The folded title, a tab and the year: one buffer for every lookup, so
    // only a new title allocates its key.
    let mut by_key: HashMap<String, usize> = HashMap::with_capacity(movies.len());
    let mut lookup = String::new();
    for (i, movie) in movies.iter().enumerate() {
        let parts = parse_name(&movie.name, max_year);
        lookup.clear();
        fold_into(parts.title, &mut lookup);
        let _ = write!(lookup, "\t{}", parts.year);
        let version = FilmVersion {
            index: i as u32,
            labels: parts.labels,
        };
        match by_key.get(lookup.as_str()) {
            Some(&t) => titles[t].versions.push(version),
            None => {
                by_key.insert(lookup.clone(), titles.len());
                titles.push(FilmTitle {
                    title: parts.title,
                    key: String::new(),
                    year: parts.year,
                    versions: vec![version],
                });
            }
        }
    }
    // Each title takes its key from the map, without a copy: the folded title
    // before the tab.
    for (mut key, t) in by_key {
        key.truncate(key.rfind('\t').unwrap_or(key.len()));
        titles[t].key = key;
    }
    for title in &mut titles {
        if title.versions.len() > 1 {
            title.versions.sort_by_key(|v| {
                let id = movies[v.index as usize].stream_id;
                (
                    std::cmp::Reverse(v.labels.rank()),
                    !v.labels.has(Labels::HDR),
                    id,
                )
            });
            let first = &movies[title.versions[0].index as usize];
            title.title = parse_name(&first.name, max_year).title;
        }
    }
    titles
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `ct --test test_vod_genres --test test_vod_versions`
Expected: all pass (11 and 6).

- [ ] **Step 5: Format, lint, full suite**

- Run `fmt src/vod/mod.rs tests/test_vod_genres.rs tests/test_vod_versions.rs`. Expected: no output.
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 6: Commit**

Message file (in the scratchpad):
```
feat(rust): what the thuisbioscoop's names say, part 2: labels and language markers where labels stand, a category's leading labels, the year, and films merged into titles with versions

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/vod/mod.rs rust/src/vod/genres.rs rust/src/vod/versions.rs rust/tests/test_vod_genres.rs rust/tests/test_vod_versions.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/vod/mod.rs rust/src/vod/genres.rs rust/src/vod/versions.rs rust/tests/test_vod_genres.rs rust/tests/test_vod_versions.rs`

---

### Task 3: The title index: records, text tables and lookups

`TitleIndex` (spec §4.2): every series and film title of a catalog in a 20-byte record, in (kind, id) order, with its name, search name and cover in three shared text tables and a film's versions in 8 bytes each (Rulings 21–23). It is built from the lists, with the genres and languages of the categories, and offers the lookups the pages need: a title by its position or id, the title a stream is a version of, what details add (`learn`), and its stats and size. The queries follow in Task 4. Pure, no I/O.

**Files:**
- Create: `rust/src/vod/index.rs`
- Modify: `rust/src/vod/mod.rs` (`pub mod index;`)
- Test: `rust/tests/test_vod_index.rs` (new)

**Interfaces:**
- Consumes (Tasks 1, 2a and 2b): `Movie`, `Series`, `Categories`, `portal::vod_info::year_of`; `merge`, `FilmTitle`, `parse_name`, `genres_in`, `category_labels`, `Labels`, `Language`, `fold_into`, `Kind`, `TitleId`.
- Produces:
  ```rust
  pub struct TitleIndex;
  impl TitleIndex {
      pub fn build(movies: &[Movie], series: &[Series], categories: &Categories, max_year: u16) -> Self;
      pub fn len(&self) -> usize;
      pub fn is_empty(&self) -> bool;
      pub fn title(&self, pos: usize) -> Option<Title<'_>>;           // adult or not: `Titles` asks (Task 5)
      pub fn find(&self, id: TitleId) -> Option<usize>;
      pub fn title_of_version(&self, stream_id: i64) -> Option<usize>;
      pub fn categories(&self) -> impl Iterator<Item = CategoryInfo<'_>> + '_;
      pub fn learn(&self, pos: usize, learned: Learned);
      pub fn stats(&self) -> Stats;
      pub fn heap_bytes(&self) -> usize;                               // the 5 MB of §4.2
      pub fn cover_bytes(&self) -> usize;
      pub(crate) fn assemble(entries: Vec<Entry<'_>>, pool: &[VersionIn<'_>], categories: Vec<CategoryRec>) -> Self;
  }
  #[derive(Clone, Copy)] #[cfg_attr(test, derive(Debug))]   // its cover may hold the account
  pub struct Title<'a> { pub pos: usize, pub id: TitleId, pub name: &'a str, pub cover: Option<&'a str>,
      pub year: u16, pub rating: u8, pub genres: u32, pub category: Option<CategoryInfo<'a>>,
      pub added_day: u16, pub dutch: bool, pub english: bool, pub adult: bool, pub trailer: bool,
      pub versions: Versions<'a> }
  pub struct Versions<'a>;  // len, is_empty, get(i) -> Option<Version<'a>>, iter()
  pub struct Version<'a> { pub stream_id: i64, pub labels: Labels, pub ext: &'a str }
  pub struct CategoryInfo<'a> { pub kind: Kind, pub id: i64, pub name: &'a str }
  pub struct Learned { pub year: u16, pub genres: u32, pub trailer: bool }                 // Default
  pub struct Stats { pub series: usize, pub films: usize, pub versions: usize, pub with_cover: usize,
      pub with_rating: usize, pub with_year: usize, pub with_genre: usize, pub dutch: usize,
      pub english: usize, pub skipped: usize }
  // pub(crate), for the cache (Task 6a): Entry<'a>, VersionIn<'a>, CategoryIn<'a>, CategoryRec,
  // fn category_rec(c: &CategoryIn<'_>) -> CategoryRec, TextTable::get
  ```

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_vod_index.rs`:

```rust
//! The title index's build (vod/index.rs, spec 2026-10-15 §4.2): what a
//! record holds, where its year, genres and language come from, lookups by
//! id and by version, what details add, and its size.

use bongo_player::portal::{Categories, Category, Movie, Series};
use bongo_player::vod::genres::{Genre, Labels};
use bongo_player::vod::index::{Learned, TitleIndex};
use bongo_player::vod::{Kind, TitleId};

const MAX_YEAR: u16 = 2027;
/// 15 October 2026, 00:00 UTC, as unix seconds: unix day 20,741.
const OCT_15: i64 = 1_792_022_400;
const DAY: i64 = 86_400;

fn film(stream_id: i64, name: &str) -> Movie {
    Movie {
        stream_id,
        name: name.into(),
        container_extension: Some("mkv".into()),
        ..Movie::default()
    }
}

fn series(series_id: i64, name: &str) -> Series {
    Series {
        series_id,
        name: name.into(),
        ..Series::default()
    }
}

fn category(id: i64, name: &str) -> Category {
    Category {
        category_id: id,
        category_name: name.into(),
    }
}

fn categories() -> Categories {
    Categories {
        vod: vec![
            category(12, "NL | Komedie"),
            category(13, "EN | Action & Adventure"),
            category(14, "Nieuw"),
        ],
        series: vec![category(12, "Series | Drama"), category(7, "Kids")],
    }
}

fn film_id(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Film,
        id,
    }
}

fn series_id(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Series,
        id,
    }
}

#[test]
fn a_catalog_becomes_its_series_and_its_films_merged_into_titles() {
    let movies = [
        Movie {
            stream_icon: Some(" https://img.example/matrix.jpg ".into()),
            rating: 82,
            added: OCT_15 - 3 * DAY,
            category_id: 13,
            ..film(1, "The Matrix (1999) 1080p")
        },
        Movie {
            added: OCT_15 - 10 * DAY,
            category_id: 12,
            container_extension: Some("mp4".into()),
            ..film(2, "NL | The Matrix (1999) [4K]")
        },
        film(3, "Other (2001)"),
    ];
    let shows = [Series {
        cover: Some("https://img.example/dark.jpg".into()),
        rating: 87,
        release_date_camel: Some("2017-12-01".into()),
        genre: Some("Drama, Mystery".into()),
        category_id: 7,
        last_modified: OCT_15,
        ..series(4321, "Dark")
    }];
    let index = TitleIndex::build(&movies, &shows, &categories(), MAX_YEAR);
    assert_eq!(index.len(), 3, "one series, two film titles");
    // Series come first: the records are in (kind, id) order.
    let dark = index.title(0).unwrap();
    assert_eq!(dark.id, series_id(4321));
    assert_eq!((dark.name, dark.year, dark.rating), ("Dark", 2017, 87));
    assert_eq!(dark.cover, Some("https://img.example/dark.jpg"));
    assert_eq!(dark.genres, Genre::Drama.bit() | Genre::Mystery.bit());
    assert_eq!(
        dark.category.map(|c| (c.kind, c.id, c.name)),
        Some((Kind::Series, 7, "Kids"))
    );
    assert_eq!(dark.added_day, 20_741);
    assert!(dark.versions.is_empty());

    let matrix = index.title(index.find(film_id(2)).unwrap()).unwrap();
    assert_eq!(matrix.id, film_id(2), "the 4K version is the title's id");
    assert_eq!(
        (matrix.name, matrix.year, matrix.rating),
        ("The Matrix", 1999, 82)
    );
    assert_eq!(
        matrix.cover,
        Some("https://img.example/matrix.jpg"),
        "the first version that has one, trimmed"
    );
    assert_eq!(matrix.added_day, 20_741 - 10, "as new as its first version");
    assert_eq!(
        matrix.category.map(|c| c.name),
        Some("NL | Komedie"),
        "the default version's"
    );
    let versions: Vec<(i64, Labels, &str)> = matrix
        .versions
        .iter()
        .map(|v| (v.stream_id, v.labels, v.ext))
        .collect();
    assert_eq!(
        versions,
        [
            (2, Labels::UHD.with(Labels::NL), "mp4"),
            (1, Labels::FHD, "mkv")
        ]
    );
    assert_eq!(
        index.find(film_id(1)),
        None,
        "a version that is not the default is no title id"
    );
    assert_eq!(index.find(series_id(2)), None);
}

#[test]
fn a_film_title_takes_genres_and_languages_from_every_versions_category_and_labels() {
    let movies = [
        Movie {
            category_id: 12,
            ..film(1, "Hotel (2020)")
        },
        Movie {
            category_id: 13,
            ..film(2, "Hotel (2020) 1080p")
        },
        Movie {
            category_id: 14,
            ..film(3, "Plain (2020)")
        },
    ];
    let index = TitleIndex::build(&movies, &[], &categories(), MAX_YEAR);
    let hotel = index.title(index.find(film_id(2)).unwrap()).unwrap();
    assert_eq!(
        hotel.genres,
        Genre::Comedy.bit() | Genre::Action.bit() | Genre::Adventure.bit()
    );
    assert!(hotel.dutch && hotel.english, "NL | Komedie and EN | Action");
    let plain = index.title(index.find(film_id(3)).unwrap()).unwrap();
    assert_eq!(plain.genres, 0, "Nieuw names no genre: the details may");
    assert!(!plain.dutch && !plain.english);
}

#[test]
fn a_series_takes_its_genres_from_its_field_else_from_its_category() {
    let shows = [
        Series {
            genre: Some("Comedy".into()),
            category_id: 12,
            ..series(1, "A")
        },
        Series {
            genre: Some("".into()),
            category_id: 12,
            ..series(2, "B")
        },
        Series {
            category_id: 99,
            ..series(3, "C")
        },
    ];
    let index = TitleIndex::build(&[], &shows, &categories(), MAX_YEAR);
    let genres: Vec<u32> = (0..3).map(|p| index.title(p).unwrap().genres).collect();
    assert_eq!(genres, [Genre::Comedy.bit(), Genre::Drama.bit(), 0]);
    assert!(
        index.title(2).unwrap().category.is_none(),
        "an unknown category is none"
    );
}

#[test]
fn a_series_year_is_its_release_dates_else_its_names() {
    let shows = [
        Series {
            release_date: Some("2005-03-24".into()),
            ..series(1, "The Office (US) (2005)")
        },
        series(2, "Fargo (2014)"),
        Series {
            release_date_camel: Some("unknown".into()),
            ..series(3, "Lost")
        },
    ];
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    let years: Vec<(&str, u16)> = (0..3)
        .map(|p| {
            let t = index.title(p).unwrap();
            (t.name, t.year)
        })
        .collect();
    assert_eq!(
        years,
        [("The Office (US)", 2005), ("Fargo", 2014), ("Lost", 0)]
    );
}

#[test]
fn a_title_is_adult_when_one_of_its_versions_is() {
    let movies = [
        film(1, "Night (2020)"),
        Movie {
            is_adult: true,
            ..film(2, "Night (2020) 4K")
        },
        film(3, "Day (2020)"),
    ];
    let shows = [Series {
        is_adult: true,
        ..series(9, "Late")
    }];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    let adult = |id: TitleId| index.title(index.find(id).unwrap()).unwrap().adult;
    assert!(adult(film_id(2)));
    assert!(!adult(film_id(3)));
    assert!(adult(series_id(9)));
}

#[test]
fn what_plays_tells_which_title_it_is() {
    let movies = [
        film(10, "A (2001) 1080p"),
        film(11, "A (2001) 4K"),
        film(20, "B (2002)"),
    ];
    let shows = [series(5, "S")];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    let a = index.find(film_id(11)).unwrap();
    assert_eq!(index.title_of_version(10), Some(a), "the 1080p version");
    assert_eq!(index.title_of_version(11), Some(a));
    assert_eq!(index.title_of_version(20), index.find(film_id(20)));
    assert_eq!(index.title_of_version(5), None, "a series id is no version");
    assert_eq!(index.title_of_version(-1), None);
}

#[test]
fn an_id_that_does_not_fit_is_left_out_and_counted() {
    let movies = [
        film(1, "Fits (2001)"),
        film(-4, "Below (2001)"),
        film(5_000_000_000, "Above (2001)"),
    ];
    let shows = [series(-1, "Negative")];
    let index = TitleIndex::build(&movies, &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1);
    assert_eq!(index.stats().skipped, 3);
}

#[test]
fn two_series_of_one_id_keep_the_first() {
    let shows = [series(7, "First"), series(7, "Second")];
    let index = TitleIndex::build(&[], &shows, &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1);
    assert_eq!(index.title(0).unwrap().name, "First");
}

#[test]
fn details_fill_what_the_lists_left_open_and_change_nothing_else() {
    let movies = [
        film(1, "No Year"),
        Movie {
            category_id: 12,
            ..film(2, "Known (2001)")
        },
    ];
    let index = TitleIndex::build(&movies, &[], &categories(), MAX_YEAR);
    let learned = Learned {
        year: 1994,
        genres: Genre::Horror.bit(),
        trailer: true,
    };
    let (open, known) = (
        index.find(film_id(1)).unwrap(),
        index.find(film_id(2)).unwrap(),
    );
    index.learn(open, learned);
    index.learn(known, learned);
    let t = index.title(open).unwrap();
    assert_eq!(
        (t.year, t.genres, t.trailer),
        (1994, Genre::Horror.bit(), true)
    );
    let t = index.title(known).unwrap();
    assert_eq!(
        (t.year, t.genres, t.trailer),
        (2001, Genre::Comedy.bit(), true),
        "the name's year and the category's genre stay"
    );
    index.learn(99, learned); // no such title: nothing happens
}

#[test]
fn the_stats_count_what_the_lists_gave() {
    let movies = [
        Movie {
            stream_icon: Some("https://x/1.jpg".into()),
            rating: 70,
            category_id: 12,
            ..film(1, "A (2001)")
        },
        film(2, "B"),
        film(3, "B 4K"),
    ];
    let shows = [Series {
        genre: Some("Drama".into()),
        ..series(1, "S EN")
    }];
    let index = TitleIndex::build(&movies, &shows, &categories(), MAX_YEAR);
    let s = index.stats();
    assert_eq!((s.series, s.films, s.versions), (1, 2, 3));
    assert_eq!(
        (s.with_cover, s.with_rating, s.with_year, s.with_genre),
        (1, 1, 1, 2)
    );
    assert_eq!((s.dutch, s.english, s.skipped), (1, 1, 0));
}

#[test]
fn the_index_keeps_small_records_and_one_text_table_per_text() {
    let movies: Vec<Movie> = (0..1_000)
        .map(|i| Movie {
            stream_icon: Some(format!("https://image.tmdb.org/t/p/w600/{i:027}.jpg")),
            ..film(i, &format!("Film number {i} (2001)"))
        })
        .collect();
    let index = TitleIndex::build(&movies, &[], &Categories::default(), MAX_YEAR);
    assert_eq!(index.len(), 1_000);
    // A record is 20 bytes, a version 8, a place in the A-Z order 4, and a
    // name and its search name ~18 bytes each with 4 for where they end.
    let per_title = index.heap_bytes() / 1_000;
    assert!(per_title <= 90, "{per_title} bytes a title");
    assert!(
        index.cover_bytes() >= 1_000 * 67,
        "the covers are counted apart"
    );
}

#[test]
fn no_lists_no_titles() {
    let index = TitleIndex::build(&[], &[], &Categories::default(), MAX_YEAR);
    assert!(index.is_empty());
    assert!(index.title(0).is_none());
    assert_eq!(index.find(film_id(1)), None);
    assert_eq!(index.categories().count(), 0);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_vod_index`
Expected: compile errors: there is no module `vod::index`.

- [ ] **Step 3: The index**

In `rust/src/vod/mod.rs`, declare the index: after the line `pub mod genres;`, add:

```rust
pub mod index;
```

Create `rust/src/vod/index.rs`:

```rust
//! The title index (spec 2026-10-15 §4.2): every series and film title of a
//! portal in a small record, with its name, search name and cover in shared
//! text tables. Built once per catalog from its lists, off the screen; pure,
//! no I/O. The catalog hands it out behind its adult lock (`catalog::Titles`):
//! every query here takes `unlocked`, and nothing else filters.
//!
//! Records are kept in (kind, id) order, so a title is found by a binary
//! search. Ids are kept as `u32`: Xtream ids are positive and far below 4·10⁹;
//! a title whose id does not fit is left out and counted.

use super::genres::{category_labels, genres_in, parse_name, Labels, Language};
use super::versions::merge;
use super::{fold_into, Kind, TitleId};
use crate::portal::{Categories, Movie, Series};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU8, Ordering};

const FLAG_FILM: u8 = 1;
const FLAG_ADULT: u8 = 1 << 1;
const FLAG_TRAILER: u8 = 1 << 2;
const FLAG_DUTCH: u8 = 1 << 3;
const FLAG_ENGLISH: u8 = 1 << 4;

/// Texts one after the other, and where each ends.
#[derive(Default)]
pub(crate) struct TextTable {
    text: String,
    ends: Vec<u32>,
}

impl TextTable {
    fn with_capacity(texts: usize, bytes: usize) -> Self {
        Self {
            text: String::with_capacity(bytes),
            ends: Vec::with_capacity(texts),
        }
    }

    fn push(&mut self, s: &str) {
        self.text.push_str(s);
        self.ends.push(self.text.len() as u32);
    }

    pub(crate) fn get(&self, i: usize) -> &str {
        let start = match i {
            0 => 0,
            _ => self.ends[i - 1] as usize,
        };
        &self.text[start..self.ends[i] as usize]
    }

    fn heap_bytes(&self) -> usize {
        self.text.capacity() + self.ends.capacity() * 4
    }

    fn shrink(&mut self) {
        self.text.shrink_to_fit();
        self.ends.shrink_to_fit();
    }
}

/// One title: 20 bytes. What the details may add later (a year, genres, the
/// trailer) is atomic, so the shared index learns it in place (`learn`).
struct Record {
    id: u32,
    genres: AtomicU32,
    /// Where its versions end in `versions` (they start where the record
    /// before's end); a series has none.
    versions_end: u32,
    year: AtomicU16,
    /// The day the portal added it (unix days); 0: unknown.
    added: u16,
    /// Its category: a position in `categories` plus one; 0: none.
    category: u16,
    rating: u8,
    flags: AtomicU8,
}

/// A film's version as kept: 8 bytes.
#[derive(Debug, Clone, Copy)]
struct VersionRec {
    stream_id: u32,
    labels: u16,
    /// Its container: a position in `exts` plus one; 0: none.
    ext: u8,
}

/// A portal category as kept, with what its name says.
pub(crate) struct CategoryRec {
    pub(crate) kind: Kind,
    pub(crate) id: i64,
    pub(crate) name: Box<str>,
    genres: u32,
    language: Language,
}

/// A version of a film, as a page offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version<'a> {
    pub stream_id: i64,
    pub labels: Labels,
    /// Its container ("mkv"); empty when the portal named none.
    pub ext: &'a str,
}

/// A film's versions, the default first.
#[derive(Debug, Clone, Copy)]
pub struct Versions<'a> {
    recs: &'a [VersionRec],
    exts: &'a [Box<str>],
}

impl<'a> Versions<'a> {
    pub fn len(&self) -> usize {
        self.recs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recs.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<Version<'a>> {
        let r = self.recs.get(i)?;
        Some(Version {
            stream_id: i64::from(r.stream_id),
            labels: Labels(r.labels),
            ext: match r.ext {
                0 => "",
                n => &self.exts[n as usize - 1],
            },
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = Version<'a>> + 'a {
        let me = *self;
        (0..me.recs.len()).filter_map(move |i| me.get(i))
    }
}

/// A portal category, as a title names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryInfo<'a> {
    pub kind: Kind,
    pub id: i64,
    pub name: &'a str,
}

/// A title as the index knows it. `Debug` for the tests only: its cover's
/// address may hold the account.
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug))]
pub struct Title<'a> {
    /// Its position in the index.
    pub pos: usize,
    pub id: TitleId,
    pub name: &'a str,
    /// Its cover's address; none when the lists gave none.
    pub cover: Option<&'a str>,
    /// 0: unknown.
    pub year: u16,
    /// ★ in tenths; 0: none.
    pub rating: u8,
    /// A mask of `Genre::bit`s.
    pub genres: u32,
    pub category: Option<CategoryInfo<'a>>,
    /// The day the portal added it (unix days); 0: unknown.
    pub added_day: u16,
    /// A version, or the series, is spoken Dutch; English.
    pub dutch: bool,
    pub english: bool,
    pub adult: bool,
    /// Its details named a trailer.
    pub trailer: bool,
    /// A film's versions, the default first; a series has none.
    pub versions: Versions<'a>,
}

/// What a title's details tell the index (`TitleIndex::learn`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Learned {
    pub year: u16,
    pub genres: u32,
    pub trailer: bool,
}

/// How the lists' titles came out, for the catalog's log line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub series: usize,
    pub films: usize,
    pub versions: usize,
    pub with_cover: usize,
    pub with_rating: usize,
    pub with_year: usize,
    pub with_genre: usize,
    pub dutch: usize,
    pub english: usize,
    /// Left out: an id that does not fit a `u32`.
    pub skipped: usize,
}

/// Every title of one catalog.
pub struct TitleIndex {
    records: Vec<Record>,
    names: TextTable,
    keys: TextTable,
    covers: TextTable,
    versions: Vec<VersionRec>,
    exts: Vec<Box<str>>,
    categories: Vec<CategoryRec>,
    skipped: usize,
}

/// A title on its way into the index (the lists' build and the cache's).
pub(crate) struct Entry<'a> {
    pub(crate) kind: Kind,
    pub(crate) id: u32,
    pub(crate) name: &'a str,
    /// Its folded name; built here when none is given.
    pub(crate) key: Option<String>,
    pub(crate) cover: &'a str,
    pub(crate) year: u16,
    pub(crate) rating: u8,
    pub(crate) added: u16,
    /// A position in the categories plus one; 0: none.
    pub(crate) category: u16,
    pub(crate) genres: u32,
    pub(crate) dutch: bool,
    pub(crate) english: bool,
    pub(crate) adult: bool,
    pub(crate) trailer: bool,
    /// Its versions: a range of the versions `assemble` is handed.
    pub(crate) versions: std::ops::Range<u32>,
}

/// A film's version on its way into the index: stream id, labels, container.
pub(crate) type VersionIn<'a> = (u32, Labels, &'a str);

/// A category on its way into the index.
pub(crate) struct CategoryIn<'a> {
    pub(crate) kind: Kind,
    pub(crate) id: i64,
    pub(crate) name: &'a str,
}

/// Unix seconds as a unix day; 0 for none.
fn day_of(secs: i64) -> u16 {
    (secs.max(0) / 86_400).min(u16::MAX as i64) as u16
}

impl TitleIndex {
    /// The titles of these lists. `max_year` is the latest year a name may
    /// give: the year after this one.
    pub fn build(
        movies: &[Movie],
        series: &[Series],
        categories: &Categories,
        max_year: u16,
    ) -> Self {
        let cats: Vec<CategoryIn<'_>> = categories
            .vod
            .iter()
            .map(|c| (Kind::Film, c))
            .chain(categories.series.iter().map(|c| (Kind::Series, c)))
            .map(|(kind, c)| CategoryIn {
                kind,
                id: c.category_id,
                name: &c.category_name,
            })
            .collect();
        let cat_recs: Vec<CategoryRec> = cats.iter().map(category_rec).collect();
        // A category id, per kind, to its position plus one.
        let mut place: HashMap<(Kind, i64), u16> = HashMap::with_capacity(cats.len());
        for (i, c) in cats.iter().enumerate().take(u16::MAX as usize - 1) {
            place.entry((c.kind, c.id)).or_insert(i as u16 + 1);
        }
        let category_of = |kind: Kind, id: i64| match id {
            0 => 0,
            id => place.get(&(kind, id)).copied().unwrap_or(0),
        };
        let mut skipped = 0;
        let mut entries: Vec<Entry<'_>> = Vec::with_capacity(movies.len() + series.len());
        let mut pool: Vec<VersionIn<'_>> = Vec::with_capacity(movies.len());
        for title in merge(movies, max_year) {
            let film = |i: usize| &movies[title.versions[i].index as usize];
            let Ok(id) = u32::try_from(film(0).stream_id) else {
                skipped += 1;
                continue;
            };
            let mut entry = Entry {
                kind: Kind::Film,
                id,
                name: title.title,
                key: Some(title.key),
                cover: "",
                year: title.year,
                rating: 0,
                added: 0,
                category: category_of(Kind::Film, film(0).category_id),
                genres: 0,
                dutch: false,
                english: false,
                adult: false,
                trailer: false,
                versions: 0..0,
            };
            let first = pool.len() as u32;
            for (i, v) in title.versions.iter().enumerate() {
                let m = film(i);
                let category = category_of(Kind::Film, m.category_id);
                let in_category = cat_recs.get(usize::from(category).wrapping_sub(1));
                entry.genres |= in_category.map_or(0, |c| c.genres);
                let spoken = [
                    v.labels.language(),
                    in_category.map_or(Language::Unknown, |c| c.language),
                ];
                entry.dutch |= spoken.contains(&Language::Dutch);
                entry.english |= spoken.contains(&Language::English);
                entry.adult |= m.is_adult;
                if entry.cover.is_empty() {
                    entry.cover = m.stream_icon.as_deref().map_or("", str::trim);
                }
                if entry.rating == 0 {
                    entry.rating = m.rating;
                }
                // A title is as new as its first version: a new 4K copy of an
                // old film is no new title.
                let added = day_of(m.added);
                if added > 0 && (entry.added == 0 || added < entry.added) {
                    entry.added = added;
                }
                match u32::try_from(m.stream_id) {
                    Ok(sid) => pool.push((
                        sid,
                        v.labels,
                        m.container_extension.as_deref().unwrap_or(""),
                    )),
                    Err(_) => skipped += 1,
                }
            }
            entry.versions = first..pool.len() as u32;
            entries.push(entry);
        }
        // Series name the same few genre fields over and over.
        let mut genres_of_field: HashMap<&str, u32> = HashMap::new();
        for s in series {
            let Ok(id) = u32::try_from(s.series_id) else {
                skipped += 1;
                continue;
            };
            let parts = parse_name(&s.name, max_year);
            let category = category_of(Kind::Series, s.category_id);
            let in_category = cat_recs.get(usize::from(category).wrapping_sub(1));
            // Its genre field; when that names none, its category's, as a film's.
            let field = s.genre.as_deref().unwrap_or("");
            let mut genres = *genres_of_field
                .entry(field)
                .or_insert_with(|| genres_in(field));
            if genres == 0 {
                genres = in_category.map_or(0, |c| c.genres);
            }
            let released = s.released().map_or(0, crate::portal::vod_info::year_of);
            let spoken = [
                parts.labels.language(),
                in_category.map_or(Language::Unknown, |c| c.language),
            ];
            entries.push(Entry {
                kind: Kind::Series,
                id,
                name: parts.title,
                key: None,
                cover: s.cover.as_deref().map_or("", str::trim),
                year: if released > 0 { released } else { parts.year },
                rating: s.rating,
                added: day_of(s.last_modified),
                category,
                genres,
                dutch: spoken.contains(&Language::Dutch),
                english: spoken.contains(&Language::English),
                adult: s.is_adult,
                trailer: false,
                versions: 0..0,
            });
        }
        let mut index = Self::assemble(entries, &pool, cat_recs);
        index.skipped = skipped;
        index
    }

    /// The index of `entries`, whose versions are ranges of `pool` and whose
    /// `category` is a position plus one in `categories`. Two titles of one
    /// kind and id keep the first.
    pub(crate) fn assemble(
        mut entries: Vec<Entry<'_>>,
        pool: &[VersionIn<'_>],
        categories: Vec<CategoryRec>,
    ) -> Self {
        entries.sort_by_key(|e| (e.kind, e.id));
        entries.dedup_by_key(|e| (e.kind, e.id));
        let n = entries.len();
        let bytes: usize = entries.iter().map(|e| e.name.len()).sum();
        let cover_bytes: usize = entries.iter().map(|e| e.cover.len()).sum();
        let mut index = TitleIndex {
            records: Vec::with_capacity(n),
            names: TextTable::with_capacity(n, bytes),
            keys: TextTable::with_capacity(n, bytes),
            covers: TextTable::with_capacity(n, cover_bytes),
            versions: Vec::with_capacity(pool.len()),
            exts: Vec::new(),
            categories,
            skipped: 0,
        };
        let mut key = String::new();
        for e in entries {
            index.names.push(e.name);
            match &e.key {
                Some(k) => index.keys.push(k),
                None => {
                    key.clear();
                    fold_into(e.name, &mut key);
                    index.keys.push(&key);
                }
            }
            index.covers.push(e.cover);
            for &(sid, labels, ext) in &pool[e.versions.start as usize..e.versions.end as usize] {
                let ext = index.ext_code(ext);
                index.versions.push(VersionRec {
                    stream_id: sid,
                    labels: labels.0,
                    ext,
                });
            }
            let flags = [
                (e.kind == Kind::Film, FLAG_FILM),
                (e.adult, FLAG_ADULT),
                (e.trailer, FLAG_TRAILER),
                (e.dutch, FLAG_DUTCH),
                (e.english, FLAG_ENGLISH),
            ]
            .iter()
            .filter(|(on, _)| *on)
            .fold(0, |all, (_, bit)| all | bit);
            index.records.push(Record {
                id: e.id,
                genres: AtomicU32::new(e.genres),
                versions_end: index.versions.len() as u32,
                year: AtomicU16::new(e.year),
                added: e.added,
                category: e.category,
                rating: e.rating,
                flags: AtomicU8::new(flags),
            });
        }
        index.names.shrink();
        index.keys.shrink();
        index.covers.shrink();
        index.versions.shrink_to_fit();
        index
    }

    /// The code of container `ext` (its place plus one); 0 for none, and when
    /// 255 others are known already.
    fn ext_code(&mut self, ext: &str) -> u8 {
        let ext = ext.trim();
        if ext.is_empty() {
            return 0;
        }
        if let Some(i) = self.exts.iter().position(|e| &**e == ext) {
            return i as u8 + 1;
        }
        if self.exts.len() >= u8::MAX as usize {
            return 0;
        }
        self.exts.push(ext.into());
        self.exts.len() as u8
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The title at `pos`, adult or not: the catalog's `Titles` asks with the
    /// lock in mind.
    pub fn title(&self, pos: usize) -> Option<Title<'_>> {
        let r = self.records.get(pos)?;
        let flags = r.flags.load(Ordering::Relaxed);
        let kind = if flags & FLAG_FILM != 0 {
            Kind::Film
        } else {
            Kind::Series
        };
        let start = match pos {
            0 => 0,
            _ => self.records[pos - 1].versions_end as usize,
        };
        let cover = self.covers.get(pos);
        Some(Title {
            pos,
            id: TitleId {
                kind,
                id: i64::from(r.id),
            },
            name: self.names.get(pos),
            cover: (!cover.is_empty()).then_some(cover),
            year: r.year.load(Ordering::Relaxed),
            rating: r.rating,
            genres: r.genres.load(Ordering::Relaxed),
            category: self.category_at(r.category),
            added_day: r.added,
            dutch: flags & FLAG_DUTCH != 0,
            english: flags & FLAG_ENGLISH != 0,
            adult: flags & FLAG_ADULT != 0,
            trailer: flags & FLAG_TRAILER != 0,
            versions: Versions {
                recs: &self.versions[start..r.versions_end as usize],
                exts: &self.exts,
            },
        })
    }

    fn category_at(&self, code: u16) -> Option<CategoryInfo<'_>> {
        let c = self.categories.get(usize::from(code).checked_sub(1)?)?;
        Some(CategoryInfo {
            kind: c.kind,
            id: c.id,
            name: &c.name,
        })
    }

    /// Where title `id` is; none when the index does not hold it.
    pub fn find(&self, id: TitleId) -> Option<usize> {
        let id32 = u32::try_from(id.id).ok()?;
        let film = id.kind == Kind::Film;
        let pos = self.records.partition_point(|r| {
            let r_film = r.flags.load(Ordering::Relaxed) & FLAG_FILM != 0;
            (r_film, r.id) < (film, id32)
        });
        let r = self.records.get(pos)?;
        let r_film = r.flags.load(Ordering::Relaxed) & FLAG_FILM != 0;
        (r_film == film && r.id == id32).then_some(pos)
    }

    /// The film title that `stream_id` is a version of: what plays tells which
    /// title it is (a scan of the versions; no map is kept for it).
    pub fn title_of_version(&self, stream_id: i64) -> Option<usize> {
        let sid = u32::try_from(stream_id).ok()?;
        let at = self.versions.iter().position(|v| v.stream_id == sid)? as u32;
        Some(self.records.partition_point(|r| r.versions_end <= at))
    }

    /// The portal's categories, films' first.
    pub fn categories(&self) -> impl Iterator<Item = CategoryInfo<'_>> + '_ {
        self.categories.iter().map(|c| CategoryInfo {
            kind: c.kind,
            id: c.id,
            name: &c.name,
        })
    }

    /// What title `pos`'s details tell: a year where its name gave none (the
    /// name's year made the title), genres where its category and genre field
    /// named none, and whether it has a trailer.
    pub fn learn(&self, pos: usize, learned: Learned) {
        let Some(r) = self.records.get(pos) else {
            return;
        };
        if learned.year > 0 {
            let _ = r
                .year
                .compare_exchange(0, learned.year, Ordering::Relaxed, Ordering::Relaxed);
        }
        if learned.genres != 0 {
            let _ =
                r.genres
                    .compare_exchange(0, learned.genres, Ordering::Relaxed, Ordering::Relaxed);
        }
        if learned.trailer {
            r.flags.fetch_or(FLAG_TRAILER, Ordering::Relaxed);
        }
    }

    /// How the titles came out.
    pub fn stats(&self) -> Stats {
        let mut s = Stats {
            versions: self.versions.len(),
            skipped: self.skipped,
            ..Stats::default()
        };
        for pos in 0..self.len() {
            let Some(t) = self.title(pos) else { continue };
            match t.id.kind {
                Kind::Film => s.films += 1,
                Kind::Series => s.series += 1,
            }
            s.with_cover += usize::from(t.cover.is_some());
            s.with_rating += usize::from(t.rating > 0);
            s.with_year += usize::from(t.year > 0);
            s.with_genre += usize::from(t.genres != 0);
            s.dutch += usize::from(t.dutch);
            s.english += usize::from(t.english);
        }
        s
    }

    /// The bytes the index holds, the covers aside (spec §4.2's 5 MB): the
    /// records, the names and search names, the versions and the categories.
    pub fn heap_bytes(&self) -> usize {
        self.records.capacity() * std::mem::size_of::<Record>()
            + self.names.heap_bytes()
            + self.keys.heap_bytes()
            + self.versions.capacity() * std::mem::size_of::<VersionRec>()
            + self.exts.iter().map(|e| e.len() + 16).sum::<usize>()
            + self
                .categories
                .iter()
                .map(|c| c.name.len() + std::mem::size_of::<CategoryRec>())
                .sum::<usize>()
    }

    /// The bytes of the cover addresses.
    pub fn cover_bytes(&self) -> usize {
        self.covers.heap_bytes()
    }
}

/// A category as kept: its genres and language read from its name.
pub(crate) fn category_rec(c: &CategoryIn<'_>) -> CategoryRec {
    CategoryRec {
        kind: c.kind,
        id: c.id,
        name: c.name.into(),
        genres: genres_in(c.name),
        language: category_labels(c.name).language(),
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `ct --test test_vod_index`
Expected: all pass (12).

- [ ] **Step 5: Format, lint, full suite**

- Run `fmt src/vod/mod.rs tests/test_vod_index.rs`. Expected: no output.
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 6: Commit**

Message file (in the scratchpad):
```
feat(rust): the title index: 20-byte records in (kind, id) order, names, search names and covers in text tables, versions, lookups, learn and stats

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/vod/index.rs rust/src/vod/mod.rs rust/tests/test_vod_index.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/vod/index.rs rust/src/vod/mod.rs rust/tests/test_vod_index.rs`

---

### Task 4: The title index: search, filters, counts, sorts and "meer zoals dit"

The search page's queries (spec §3.3), in one pass over the records: every word in the folded title, the filters of the five groups (values of a group add up, the groups narrow each other), the count of every value with the text and the other groups, and the four sorts on one packed key with the A-Z rank breaking ties (Rulings 25 and 26). "Meer zoals dit" picks the best rated titles that share a genre or the category (Ruling 27). Adult titles take part only when the caller says the lock is off.

**Files:**
- Modify: `rust/src/vod/index.rs` (the A-Z rank; the queries at the end)
- Test: `rust/tests/test_vod_search.rs` (new)

**Interfaces:**
- Consumes (Task 3): `TitleIndex`, its records and text tables, `Title`, `FLAG_*`; `Genre`, `Kind`, `TitleId`, `fold`.
- Produces:
  ```rust
  pub enum Sort { Relevance /* default */, Rating, Newest, Title }
  pub enum Years { Since2020, The2010s, The2000s, Before2000 }  // ALL, bit(self) -> u8, of(year: u16) -> Option<Years>
  pub enum Stars { EightUp, SevenUp }                            // ALL, bit(self) -> u8, floor(self) -> u8
  pub enum Spoken { Dutch, English, Other }                      // ALL, bit(self) -> u8
  #[derive(Debug, Clone, Default, PartialEq, Eq)]
  pub struct Query { pub text: String, pub kind: Option<Kind>, pub genres: u32, pub years: u8,
      pub stars: u8, pub spoken: u8, pub sort: Sort }
  pub struct Counts { pub all: usize, pub series: usize, pub films: usize, pub genres: [usize; 24],
      pub years: [usize; 4], pub stars: [usize; 2], pub spoken: [usize; 3] }
  pub struct Results { pub titles: Vec<u32>, pub counts: Counts }   // positions, sorted
  impl TitleIndex {
      pub fn search(&self, query: &Query, unlocked: bool) -> Results;
      pub fn more_like(&self, pos: usize, limit: usize, unlocked: bool,
          watched: &HashSet<TitleId>, disliked: &HashSet<TitleId>) -> Vec<u32>;
  }
  ```

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_vod_search.rs`:

```rust
//! The title index's queries (vod/index.rs, spec 2026-10-15 §3.3): search by
//! title, the filters with their counts, the four sorts, and "meer zoals dit".

use bongo_player::portal::{Categories, Category, Movie, Series};
use bongo_player::vod::genres::Genre;
use bongo_player::vod::index::{Query, Sort, Spoken, Stars, TitleIndex, Years};
use bongo_player::vod::{Kind, TitleId};
use std::collections::HashSet;

const MAX_YEAR: u16 = 2027;
const DAY: i64 = 86_400;
/// Unix day 20,000 (10 October 2024), in seconds.
const DAY_20000: i64 = 20_000 * DAY;

/// A series with this name, genre field, release year and rating.
fn show(id: i64, name: &str, genre: &str, year: u16, rating: u8) -> Series {
    Series {
        series_id: id,
        name: name.into(),
        genre: Some(genre.into()),
        release_date_camel: (year > 0).then(|| format!("{year}-01-01")),
        rating,
        ..Series::default()
    }
}

/// A film in category `category` (see `categories`).
fn film(id: i64, name: &str, category: i64, rating: u8) -> Movie {
    Movie {
        stream_id: id,
        name: name.into(),
        category_id: category,
        rating,
        ..Movie::default()
    }
}

fn categories() -> Categories {
    Categories {
        vod: vec![
            Category {
                category_id: 1,
                category_name: "NL | Komedie".into(),
            },
            Category {
                category_id: 2,
                category_name: "EN | Drama".into(),
            },
        ],
        series: Vec::new(),
    }
}

fn index(movies: &[Movie], shows: &[Series]) -> TitleIndex {
    TitleIndex::build(movies, shows, &categories(), MAX_YEAR)
}

/// The names of a query's titles, in its order.
fn names(index: &TitleIndex, query: &Query) -> Vec<String> {
    index
        .search(query, false)
        .titles
        .iter()
        .map(|&p| index.title(p as usize).unwrap().name.to_owned())
        .collect()
}

fn text(t: &str) -> Query {
    Query {
        text: t.into(),
        ..Query::default()
    }
}

#[test]
fn every_word_must_be_in_the_title_whatever_its_case_or_accents() {
    let idx = index(
        &[],
        &[
            show(1, "Pokémon: De Film", "Kids", 2000, 60),
            show(2, "Pokemon Detective Pikachu", "Kids", 2019, 66),
            show(3, "The Matrix", "Action", 1999, 82),
        ],
    );
    assert_eq!(
        names(&idx, &text("pokemon")),
        ["Pokemon Detective Pikachu", "Pokémon: De Film"]
    );
    assert_eq!(names(&idx, &text("POKÉMON film")), ["Pokémon: De Film"]);
    assert_eq!(
        names(&idx, &text("matrix the")),
        ["The Matrix"],
        "in any order"
    );
    assert!(names(&idx, &text("matrix reloaded")).is_empty());
}

#[test]
fn a_title_that_starts_with_the_query_comes_first_then_the_best_rated() {
    let idx = index(
        &[],
        &[
            show(1, "Batman Begins", "Action", 2005, 70),
            show(2, "The Batman", "Action", 2022, 80),
            show(3, "Batman Returns", "Action", 1992, 75),
        ],
    );
    assert_eq!(
        names(&idx, &text("batman")),
        ["Batman Returns", "Batman Begins", "The Batman"]
    );
}

#[test]
fn the_four_sorts_order_as_the_page_names_them() {
    let shows = [
        Series {
            last_modified: DAY_20000 + 2 * DAY,
            ..show(1, "Beta", "Drama", 2010, 70)
        },
        Series {
            last_modified: DAY_20000 + 5 * DAY,
            ..show(2, "Alpha", "Drama", 2011, 0)
        },
        Series {
            last_modified: DAY_20000 + DAY,
            ..show(3, "Ähnlich", "Drama", 2012, 70)
        },
        show(4, "Gamma", "Drama", 2013, 90),
    ];
    let idx = index(&[], &shows);
    let sorted = |sort| {
        names(
            &idx,
            &Query {
                sort,
                ..Query::default()
            },
        )
    };
    // ★ high to low, none last; on a tie the newest.
    assert_eq!(sorted(Sort::Rating), ["Gamma", "Beta", "Ähnlich", "Alpha"]);
    // Newest first; an unknown day last.
    assert_eq!(sorted(Sort::Newest), ["Alpha", "Beta", "Ähnlich", "Gamma"]);
    // A to Z without accents: "ahnlich" before "alpha".
    assert_eq!(sorted(Sort::Title), ["Ähnlich", "Alpha", "Beta", "Gamma"]);
    // No text: everything starts with it, so ★.
    assert_eq!(
        sorted(Sort::Relevance),
        ["Gamma", "Ähnlich", "Beta", "Alpha"]
    );
}

#[test]
fn values_of_one_group_add_up_and_the_groups_narrow_each_other() {
    let movies = [
        film(10, "Komedie Film (2021)", 1, 72),
        film(11, "Drama Film (2015)", 2, 85),
    ];
    let shows = [
        show(1, "Komedie Serie", "Comedy", 2021, 81),
        show(2, "Drama Serie", "Drama", 2003, 65),
        show(3, "Horror Serie", "Horror", 1985, 90),
    ];
    let idx = index(&movies, &shows);
    let q = |f: &dyn Fn(&mut Query)| {
        let mut query = Query {
            sort: Sort::Title,
            ..Query::default()
        };
        f(&mut query);
        names(&idx, &query)
    };
    let comedy_or_drama = Genre::Comedy.bit() | Genre::Drama.bit();
    assert_eq!(
        q(&|x| x.genres = comedy_or_drama),
        ["Drama Film", "Drama Serie", "Komedie Film", "Komedie Serie"]
    );
    assert_eq!(
        q(&|x| {
            x.genres = comedy_or_drama;
            x.kind = Some(Kind::Film);
        }),
        ["Drama Film", "Komedie Film"]
    );
    assert_eq!(
        q(&|x| x.years = Years::Since2020.bit() | Years::Before2000.bit()),
        ["Horror Serie", "Komedie Film", "Komedie Serie"]
    );
    assert_eq!(
        q(&|x| x.stars = Stars::EightUp.bit()),
        ["Drama Film", "Horror Serie", "Komedie Serie"]
    );
    assert_eq!(q(&|x| x.spoken = Spoken::Dutch.bit()), ["Komedie Film"]);
    assert_eq!(
        q(&|x| x.spoken = Spoken::Other.bit()),
        ["Drama Serie", "Horror Serie", "Komedie Serie"],
        "neither Dutch nor English: unknown included"
    );
}

#[test]
fn the_counts_show_what_each_value_would_give_with_the_other_filters() {
    let movies = [
        film(10, "Komedie Film (2021)", 1, 72),
        film(11, "Drama Film (2015)", 2, 85),
    ];
    let shows = [
        show(1, "Komedie Serie", "Comedy", 2021, 81),
        show(2, "Drama Serie", "Drama", 2003, 65),
        show(3, "Horror Serie", "Horror", 1985, 90),
    ];
    let idx = index(&movies, &shows);
    let query = Query {
        text: "i".into(),
        kind: Some(Kind::Film),
        genres: Genre::Comedy.bit(),
        ..Query::default()
    };
    let results = idx.search(&query, false);
    assert_eq!(results.titles.len(), 1, "the comedy film");
    let c = &results.counts;
    // Soort: with the genre filter, not the kind's own.
    assert_eq!((c.all, c.series, c.films), (2, 1, 1));
    // Genre: films (the kind filter), any genre (not the genre's own).
    let genre = |g: Genre| c.genres[Genre::ALL.iter().position(|x| *x == g).unwrap()];
    assert_eq!(
        (
            genre(Genre::Comedy),
            genre(Genre::Drama),
            genre(Genre::Horror)
        ),
        (1, 1, 0)
    );
    // Jaar, Waardering, Taal: the comedy films.
    assert_eq!(c.years, [1, 0, 0, 0]);
    assert_eq!(c.stars, [0, 1]);
    assert_eq!(c.spoken, [1, 0, 0]);
}

#[test]
fn an_unknown_year_is_in_no_year_bucket() {
    let idx = index(
        &[],
        &[
            show(1, "Old", "Drama", 1950, 0),
            show(2, "Unknown", "Drama", 0, 0),
        ],
    );
    let results = idx.search(&Query::default(), false);
    assert_eq!(results.counts.years, [0, 0, 0, 1]);
    let before = Query {
        years: Years::Before2000.bit(),
        ..Query::default()
    };
    assert_eq!(names(&idx, &before), ["Old"]);
    assert_eq!(Years::of(2020), Some(Years::Since2020));
    assert_eq!(Years::of(2019), Some(Years::The2010s));
    assert_eq!(Years::of(2000), Some(Years::The2000s));
    assert_eq!(Years::of(1999), Some(Years::Before2000));
    assert_eq!(Years::of(0), None);
}

#[test]
fn adult_titles_are_found_and_counted_only_while_unlocked() {
    let shows = [
        show(1, "Night Show", "Drama", 2020, 70),
        Series {
            is_adult: true,
            ..show(2, "Night Club", "Drama", 2020, 70)
        },
    ];
    let idx = index(&[], &shows);
    let q = text("night");
    let locked = idx.search(&q, false);
    assert_eq!(locked.titles.len(), 1);
    assert_eq!(locked.counts.all, 1, "nor counted");
    let unlocked = idx.search(&q, true);
    assert_eq!(unlocked.titles.len(), 2);
    assert_eq!(unlocked.counts.all, 2);
}

#[test]
fn more_like_this_shares_a_genre_or_the_category_best_rated_first() {
    let movies = [
        film(10, "Seed (2020)", 1, 70),
        film(11, "Same Category (2019)", 1, 60),
        film(12, "Drama Film (2018)", 2, 99),
    ];
    let shows = [
        show(1, "Comedy Show", "Comedy", 2019, 80),
        show(2, "Seen Comedy", "Comedy", 2018, 95),
        show(3, "Disliked Comedy", "Comedy", 2018, 90),
        show(4, "Unrated Comedy", "Comedy", 2018, 0),
        Series {
            is_adult: true,
            ..show(5, "Adult Comedy", "Comedy", 2018, 99)
        },
    ];
    let idx = index(&movies, &shows);
    let seed = idx
        .find(TitleId {
            kind: Kind::Film,
            id: 10,
        })
        .unwrap();
    let watched: HashSet<TitleId> = [TitleId {
        kind: Kind::Series,
        id: 2,
    }]
    .into();
    let disliked: HashSet<TitleId> = [TitleId {
        kind: Kind::Series,
        id: 3,
    }]
    .into();
    let like: Vec<&str> = idx
        .more_like(seed, 6, false, &watched, &disliked)
        .iter()
        .map(|&p| idx.title(p as usize).unwrap().name)
        .collect();
    assert_eq!(like, ["Comedy Show", "Same Category", "Unrated Comedy"]);
    let top: Vec<u32> = idx.more_like(seed, 1, false, &watched, &disliked);
    assert_eq!(top.len(), 1);
    assert_eq!(idx.title(top[0] as usize).unwrap().name, "Comedy Show");
    assert!(idx
        .more_like(seed, 0, false, &watched, &disliked)
        .is_empty());
    let unlocked = idx.more_like(seed, 1, true, &watched, &disliked);
    assert_eq!(
        idx.title(unlocked[0] as usize).unwrap().name,
        "Adult Comedy"
    );
}

#[test]
fn a_title_without_genre_or_category_is_like_nothing() {
    let idx = index(&[film(1, "Lonely", 0, 50), film(2, "Other", 0, 50)], &[]);
    let none = HashSet::new();
    assert!(idx.more_like(0, 6, false, &none, &none).is_empty());
    assert!(
        idx.more_like(99, 6, false, &none, &none).is_empty(),
        "no such title"
    );
}

#[test]
fn a_query_without_letters_or_digits_is_no_text_filter() {
    let idx = index(
        &[],
        &[
            show(1, "Alpha", "Drama", 2001, 50),
            show(2, "Beta", "Drama", 2002, 60),
        ],
    );
    for q in ["", " ", "!!!", " - ", "★"] {
        assert_eq!(idx.search(&text(q), false).titles.len(), 2, "{q:?}");
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_vod_search`
Expected: compile errors: there is no `Query`, `Sort` or `search`.

- [ ] **Step 3: The A-Z rank**

In `rust/src/vod/index.rs`, the imports: the sets of `more_like`. Replace

```rust
use std::collections::HashMap;
```

with

```rust
use std::collections::{HashMap, HashSet};
```

In `rust/src/vod/index.rs`, in `pub struct TitleIndex`, before `skipped`, add the A-Z rank. Replace

```rust
    categories: Vec<CategoryRec>,
    skipped: usize,
}
```

with

```rust
    categories: Vec<CategoryRec>,
    /// Each title's place in the A-Z order of the search names: the sort
    /// "Titel (A–Z)", and the tie-break of the others.
    rank: Vec<u32>,
    skipped: usize,
}
```

In `rust/src/vod/index.rs`, in `TitleIndex::assemble`, in the struct literal. Replace

```rust
            categories,
            skipped: 0,
        };
```

with

```rust
            categories,
            rank: Vec::new(),
            skipped: 0,
        };
```

In `rust/src/vod/index.rs`, in `TitleIndex::assemble`, the ranks, after the loop over the entries. Before

```rust
        index.names.shrink();
        index.keys.shrink();
```

add:

```rust
        let mut order: Vec<u32> = (0..n as u32).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (a as usize, b as usize);
            index.keys.get(a).cmp(index.keys.get(b)).then(a.cmp(&b))
        });
        index.rank = vec![0; n];
        for (rank, pos) in order.into_iter().enumerate() {
            index.rank[pos as usize] = rank as u32;
        }
```

In `rust/src/vod/index.rs`, in `heap_bytes`, count the ranks. Replace

```rust
    /// The bytes the index holds, the covers aside (spec §4.2's 5 MB): the
    /// records, the names and search names, the versions and the categories.
    pub fn heap_bytes(&self) -> usize {
        self.records.capacity() * std::mem::size_of::<Record>()
            + self.names.heap_bytes()
            + self.keys.heap_bytes()
            + self.versions.capacity() * std::mem::size_of::<VersionRec>()
```

with

```rust
    /// The bytes the index holds, the covers aside (spec §4.2's 5 MB): the
    /// records, the names and search names, the versions, the A-Z order and
    /// the categories.
    pub fn heap_bytes(&self) -> usize {
        self.records.capacity() * std::mem::size_of::<Record>()
            + self.names.heap_bytes()
            + self.keys.heap_bytes()
            + self.versions.capacity() * std::mem::size_of::<VersionRec>()
            + self.rank.capacity() * 4
```

- [ ] **Step 4: The queries**

In `rust/src/vod/index.rs`, at the end of the file, after `fn category_rec`, add the queries:

```rust
// ---- the search page's queries (spec 2026-10-15 §3.3) ----

/// How results are ordered: "Sorteer".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// A title that starts with the query before one that holds it, then ★.
    #[default]
    Relevance,
    /// ★ high to low; a title without one last; the newest first on a tie.
    Rating,
    /// The portal's add day, newest first.
    Newest,
    /// A to Z by the folded title.
    Title,
}

/// The year filter's values, as bits of `Query::years`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Years {
    /// 2020 and later.
    Since2020,
    The2010s,
    The2000s,
    /// A known year before 2000.
    Before2000,
}

impl Years {
    pub const ALL: [Years; 4] = [
        Years::Since2020,
        Years::The2010s,
        Years::The2000s,
        Years::Before2000,
    ];

    pub fn bit(self) -> u8 {
        1 << self as u8
    }

    /// The bucket of `year`; none for an unknown year.
    pub fn of(year: u16) -> Option<Years> {
        match year {
            0 => None,
            2020.. => Some(Years::Since2020),
            2010..=2019 => Some(Years::The2010s),
            2000..=2009 => Some(Years::The2000s),
            _ => Some(Years::Before2000),
        }
    }
}

/// The rating filter's values, as bits of `Query::stars`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stars {
    /// ★ 8 and higher.
    EightUp,
    /// ★ 7 and higher.
    SevenUp,
}

impl Stars {
    pub const ALL: [Stars; 2] = [Stars::EightUp, Stars::SevenUp];

    pub fn bit(self) -> u8 {
        1 << self as u8
    }

    /// The lowest rating it lets through, in tenths.
    pub fn floor(self) -> u8 {
        match self {
            Stars::EightUp => 80,
            Stars::SevenUp => 70,
        }
    }
}

/// The language filter's values, as bits of `Query::spoken`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spoken {
    Dutch,
    English,
    /// Neither Dutch nor English as far as the names tell, unknown included.
    Other,
}

impl Spoken {
    pub const ALL: [Spoken; 3] = [Spoken::Dutch, Spoken::English, Spoken::Other];

    pub fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// What the search page asks: its text and its filters. Within a group the
/// chosen values add up (Komedie or Drama); the groups narrow each other.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Every word must be in the title; case and accents do not count.
    pub text: String,
    /// "Soort": a series or a film; none for "Alles".
    pub kind: Option<Kind>,
    /// "Genre": a mask of `Genre::bit`s; 0 when none is chosen.
    pub genres: u32,
    /// "Jaar": a mask of `Years::bit`s.
    pub years: u8,
    /// "Waardering": a mask of `Stars::bit`s.
    pub stars: u8,
    /// "Taal": a mask of `Spoken::bit`s.
    pub spoken: u8,
    pub sort: Sort,
}

/// How many titles each filter value gives, with the text and the other
/// groups' filters: what choosing it would show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    /// "Soort": everything, the series, the films.
    pub all: usize,
    pub series: usize,
    pub films: usize,
    /// Per genre, in `Genre::ALL`'s order.
    pub genres: [usize; 24],
    /// Per `Years::ALL`.
    pub years: [usize; 4],
    /// Per `Stars::ALL`.
    pub stars: [usize; 2],
    /// Per `Spoken::ALL`.
    pub spoken: [usize; 3],
}

/// A query's titles, in its order, and the counts beside them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Results {
    /// Positions in the index.
    pub titles: Vec<u32>,
    pub counts: Counts,
}

/// Which of the five filter groups a title passes, as bits.
const KIND: u8 = 1;
const GENRE: u8 = 1 << 1;
const YEAR: u8 = 1 << 2;
const STARS: u8 = 1 << 3;
const SPOKEN: u8 = 1 << 4;
const EVERY: u8 = KIND | GENRE | YEAR | STARS | SPOKEN;

impl TitleIndex {
    /// The titles that answer `query`, sorted as it asks, with the counts of
    /// every filter value. Adult titles are left out unless `unlocked`.
    pub fn search(&self, query: &Query, unlocked: bool) -> Results {
        let folded = super::fold(&query.text);
        let words: Vec<&str> = folded.split(' ').filter(|w| !w.is_empty()).collect();
        let mut counts = Counts::default();
        let mut hits: Vec<(u64, u32)> = Vec::new();
        for (pos, r) in self.records.iter().enumerate() {
            let flags = r.flags.load(Ordering::Relaxed);
            if flags & FLAG_ADULT != 0 && !unlocked {
                continue;
            }
            let key = self.keys.get(pos);
            if !words.iter().all(|w| key.contains(w)) {
                continue;
            }
            let film = flags & FLAG_FILM != 0;
            let genres = r.genres.load(Ordering::Relaxed);
            let year = Years::of(r.year.load(Ordering::Relaxed));
            let (dutch, english) = (flags & FLAG_DUTCH != 0, flags & FLAG_ENGLISH != 0);
            let spoken = if dutch || english {
                (u8::from(dutch) * Spoken::Dutch.bit())
                    | (u8::from(english) * Spoken::English.bit())
            } else {
                Spoken::Other.bit()
            };
            let stars = Stars::ALL
                .iter()
                .filter(|s| r.rating >= s.floor())
                .fold(0, |m, s| m | s.bit());
            let passes = [
                (KIND, query.kind.is_none_or(|k| (k == Kind::Film) == film)),
                (GENRE, query.genres == 0 || genres & query.genres != 0),
                (
                    YEAR,
                    query.years == 0 || year.is_some_and(|y| query.years & y.bit() != 0),
                ),
                (STARS, query.stars == 0 || stars & query.stars != 0),
                (SPOKEN, query.spoken == 0 || spoken & query.spoken != 0),
            ]
            .iter()
            .filter(|(_, ok)| *ok)
            .fold(0, |m, (bit, _)| m | bit);
            let but = |group: u8| passes | group == EVERY;
            if but(KIND) {
                counts.all += 1;
                if film {
                    counts.films += 1;
                } else {
                    counts.series += 1;
                }
            }
            if but(GENRE) {
                let mut g = genres;
                while g != 0 {
                    counts.genres[g.trailing_zeros() as usize] += 1;
                    g &= g - 1;
                }
            }
            if let (true, Some(y)) = (but(YEAR), year) {
                counts.years[y as usize] += 1;
            }
            if but(STARS) {
                for (i, s) in Stars::ALL.iter().enumerate() {
                    counts.stars[i] += usize::from(stars & s.bit() != 0);
                }
            }
            if but(SPOKEN) {
                for (i, s) in Spoken::ALL.iter().enumerate() {
                    counts.spoken[i] += usize::from(spoken & s.bit() != 0);
                }
            }
            if passes == EVERY {
                let starts = key.starts_with(folded.as_str());
                hits.push((self.sort_key(pos, query.sort, starts), pos as u32));
            }
        }
        hits.sort_unstable_by_key(|&(key, _)| key);
        Results {
            titles: hits.into_iter().map(|(_, pos)| pos).collect(),
            counts,
        }
    }

    /// The order of title `pos` under `sort`, as one number, smallest first;
    /// the A-Z rank breaks every tie. `starts`: its title starts with the query.
    fn sort_key(&self, pos: usize, sort: Sort, starts: bool) -> u64 {
        let r = &self.records[pos];
        let unrated = u64::from(u8::MAX - r.rating);
        let older = u64::from(u16::MAX - r.added);
        let rank = u64::from(self.rank[pos]);
        match sort {
            Sort::Relevance => (u64::from(!starts) << 63) | (unrated << 32) | rank,
            Sort::Rating => (unrated << 48) | (older << 32) | rank,
            Sort::Newest => (older << 48) | (unrated << 32) | rank,
            Sort::Title => rank,
        }
    }

    /// Up to `limit` titles like title `pos` ("Meer zoals dit", "Omdat je X
    /// leuk vond"): those that share a genre with it or sit in its category,
    /// best rated first (the newest on a tie), without it, without what was
    /// `watched` or `disliked`, and without adult titles unless `unlocked`.
    pub fn more_like(
        &self,
        pos: usize,
        limit: usize,
        unlocked: bool,
        watched: &HashSet<TitleId>,
        disliked: &HashSet<TitleId>,
    ) -> Vec<u32> {
        let Some(this) = self.title(pos) else {
            return Vec::new();
        };
        let category = self.records[pos].category;
        let mut like: Vec<(u64, u32)> = Vec::new();
        for (other, r) in self.records.iter().enumerate() {
            let flags = r.flags.load(Ordering::Relaxed);
            let shares = r.genres.load(Ordering::Relaxed) & this.genres != 0
                || (category != 0 && r.category == category);
            if other == pos || !shares || (flags & FLAG_ADULT != 0 && !unlocked) {
                continue;
            }
            let kind = if flags & FLAG_FILM != 0 {
                Kind::Film
            } else {
                Kind::Series
            };
            let id = TitleId {
                kind,
                id: i64::from(r.id),
            };
            if watched.contains(&id) || disliked.contains(&id) {
                continue;
            }
            like.push((self.sort_key(other, Sort::Rating, false), other as u32));
        }
        if like.len() > limit {
            if limit == 0 {
                return Vec::new();
            }
            like.select_nth_unstable_by_key(limit - 1, |&(key, _)| key);
            like.truncate(limit);
        }
        like.sort_unstable_by_key(|&(key, _)| key);
        like.into_iter().map(|(_, pos)| pos).collect()
    }
}
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `ct --test test_vod_search --test test_vod_index`
Expected: all pass (10 and 12).

- [ ] **Step 6: Format, lint, full suite**

- Run `fmt src/vod/mod.rs tests/test_vod_search.rs`. Expected: no output.
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 7: Commit**

Message file (in the scratchpad):
```
feat(rust): the title index's queries: every word in the folded title, five filter groups with the counts of every value, four sorts, and meer zoals dit

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/vod/index.rs rust/tests/test_vod_search.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/vod/index.rs rust/tests/test_vod_search.rs`

---

### Task 5: The catalog: the titles built after the catalog, off the screen, behind the lock

The catalog's fetch task asks the portal's two category lists beside its three lists, in a task of their own, and puts the catalog as soon as the three lists are in, as before phase 1: zapping, the zap ring, `f` and the guide never wait for the categories (Ruling 1'). The titles wait for them, 15 s at most (then they go without), are then built on the blocking pool (Ruling 28) and are put in the store beside the catalog (`put_titles`, counted in `titles_revision`); F5 does the same. What only the index reads moves out of the lists before they are stored (`vod::index::Extras`, Ruling 22); the build reads the stored lists with that beside them and copies none of them (`TitleIndex::build_with`, Ruling 53). One log line tells what the lists gave (Ruling 30, spec §11). The index leaves the store only as `Titles`, the index and the lock state when asked, whose every answer leaves adult titles out while locked (spec §4.2: "Er komt nergens anders een filter").

**Files:**
- Modify: `rust/src/catalog.rs`, `rust/src/vod/index.rs` (`Extras`, `build_with`)
- Test: `rust/tests/test_vod_catalog.rs` (new; 12 tests), `rust/src/catalog.rs` (`mod zap_tests`: one test reads the new `StoredCatalog::new`)

**Interfaces:**
- Consumes: Task 1's `Categories` and `Portal::fetch_categories` (whose errors name no address); Tasks 3–4's `TitleIndex::{build, title, find, title_of_version, more_like, learn, categories, stats, heap_bytes, cover_bytes, search}` (Task 3's body of `build` becomes `build_with`'s), `Query`, `Results`, `Title`, `CategoryInfo`, `Learned`; `TitleId`.
- Produces:
  ```rust
  // vod/index.rs
  #[derive(Default)]
  pub struct Extras;   // what the lists give up: a film's cover; a series' cover, genre field and release year
  impl Extras {
      pub fn take(movies: &mut [Movie], series: &mut [Series]) -> Extras;   // moves them out, copies nothing
  }
  impl TitleIndex {
      pub fn build_with(movies: &[Movie], series: &[Series], extras: &Extras, categories: &Categories,
          max_year: u16) -> Self;   // `build` is this with `Extras::default()`: every record keeps its own
  }
  // catalog.rs
  #[derive(Clone)]
  pub struct Titles;   // the index and the lock state when asked
  impl Titles {
      pub fn search(&self, query: &Query) -> Results;
      pub fn title(&self, pos: usize) -> Option<Title<'_>>;           // none for an adult one while locked
      pub fn find(&self, id: TitleId) -> Option<usize>;
      pub fn title_of_version(&self, stream_id: i64) -> Option<usize>;
      pub fn more_like(&self, pos: usize, limit: usize, watched: &HashSet<TitleId>, disliked: &HashSet<TitleId>) -> Vec<u32>;
      pub fn learn(&self, pos: usize, learned: Learned);
      pub fn categories(&self) -> impl Iterator<Item = CategoryInfo<'_>> + '_;
  }
  impl CatalogStore {
      pub fn titles(&self) -> Option<Titles>;      // none until the titles of a catalog are put
      pub fn titles_revision(&self) -> u64;        // one more with every put of titles
      pub fn with_categories_wait(self, wait: Duration) -> Self;   // 15 s; a test waits less
  }
  // private: StoredCatalog.catalog: Arc<PortalCatalog> (its films and series, shared with the build);
  //          StoredCatalog::new(PortalCatalog) -> (StoredCatalog, TitleInput);
  //          struct TitleInput { lists: Arc<PortalCatalog>, extras: Extras }, TitleInput::build(self, &Categories) -> TitleIndex;
  //          CatalogStore.{titles: Arc<RwLock<Option<Arc<TitleIndex>>>>, titles_revision, categories_wait};
  //          const CATEGORIES_WAIT: Duration (15 s);
  //          async fn categories_after(JoinHandle<Result<Categories, PortalError>>, Duration) -> Categories;
  //          fn put_catalog(..) -> u64 (the count it put the catalog at);
  //          fn put_titles(slot, titles_revision, revision, of: u64, Arc<TitleIndex>);
  //          fn latest_year() -> u16; fn log_titles(&TitleIndex, &Categories, Duration)
  ```
  Log lines: info Ruling 30's `vod: <n> titles (…) …`; warn `catalog: no categories (<e>) - the titles go without` (`<e>` names no address; `none within 15s` at the bound), `vod: titles not built (<e>)`.

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_vod_catalog.rs`:

```rust
//! The title index behind the catalog (catalog.rs, spec 2026-10-15 §4.2): the
//! portal's categories are asked beside the three lists, the catalog is put as
//! soon as those are in, and its titles are built after it, once the categories
//! are in (15 s at most), off the screen and from the stored lists. They leave
//! the store only as `Titles`, which apply the adult lock. The channels wait for
//! none of it, and `f` is untouched.

use bongo_player::args::XtreamCreds;
use bongo_player::catalog::{CatalogStatus, CatalogStore};
use bongo_player::portal::m3u::M3uPortal;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::{Catalog, Movie, Portal, Series};
use bongo_player::vod::genres::Genre;
use bongo_player::vod::index::Query;
use bongo_player::vod::{Kind, TitleId};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn idle_portal() -> Arc<dyn Portal> {
    Arc::new(XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h".into(),
        port: 80,
    }))
}

fn film(id: i64, name: &str, adult: bool) -> Movie {
    Movie {
        stream_id: id,
        name: name.into(),
        is_adult: adult,
        ..Movie::default()
    }
}

fn library() -> Catalog {
    Catalog {
        movies: vec![
            film(1, "NL | The Matrix (1999) [4K]", false),
            film(2, "The Matrix (1999) 1080p", false),
            film(3, "Night Club (2020)", true),
        ],
        series: vec![Series {
            series_id: 9,
            name: "Dark".into(),
            genre: Some("Drama".into()),
            ..Series::default()
        }],
        ..Catalog::default()
    }
}

/// The names a search for everything gives, A to Z.
fn everything(store: &CatalogStore) -> Vec<String> {
    let titles = store.titles().unwrap();
    let query = Query {
        sort: bongo_player::vod::index::Sort::Title,
        ..Query::default()
    };
    titles
        .search(&query)
        .titles
        .iter()
        .map(|&p| titles.title(p as usize).unwrap().name.to_owned())
        .collect()
}

#[test]
fn a_catalog_hands_out_its_titles_behind_the_lock() {
    let store = CatalogStore::from_catalog(idle_portal(), library());
    assert_eq!(
        everything(&store),
        ["Dark", "The Matrix"],
        "locked: no adult title"
    );
    let adult = TitleId {
        kind: Kind::Film,
        id: 3,
    };
    let titles = store.titles().unwrap();
    let pos = titles.find(adult);
    assert_eq!(pos, None, "nor found by its id");
    assert_eq!(titles.title_of_version(3), None);
    store.set_adult_unlocked(true);
    assert_eq!(everything(&store), ["Dark", "Night Club", "The Matrix"]);
    let unlocked = store.titles().unwrap();
    let pos = unlocked.find(adult).unwrap();
    assert!(unlocked.title(pos).unwrap().adult);
    assert!(
        titles.title(pos).is_none(),
        "titles asked while locked stay locked"
    );
    store.set_adult_unlocked(false);
    assert_eq!(everything(&store).len(), 2);
}

#[test]
fn there_are_no_titles_before_a_catalog_and_none_on_an_m3u_list() {
    assert!(CatalogStore::new(idle_portal()).titles().is_none());
    let (portal, catalog) =
        M3uPortal::from_text("#EXTM3U\n#EXTINF:-1,Een\nhttp://a.example/1\n").unwrap();
    let m3u = CatalogStore::from_catalog(Arc::new(portal), catalog);
    let titles = m3u.titles().unwrap();
    assert!(titles.search(&Query::default()).titles.is_empty());
}

#[test]
fn what_the_stored_lists_give_up_reaches_the_titles() {
    let catalog = Catalog {
        movies: vec![Movie {
            stream_icon: Some("https://c.example/een.jpg".into()),
            ..film(1, "Een (2001)", false)
        }],
        series: vec![Series {
            series_id: 9,
            name: "Dark".into(),
            cover: Some("https://c.example/dark.jpg".into()),
            genre: Some("Drama".into()),
            release_date: Some("2017-12-01".into()),
            ..Series::default()
        }],
        ..Catalog::default()
    };
    let store = CatalogStore::from_catalog(idle_portal(), catalog);
    let titles = store.titles().unwrap();
    let title = |kind, id| {
        titles
            .title(titles.find(TitleId { kind, id }).unwrap())
            .unwrap()
    };
    assert_eq!(
        title(Kind::Film, 1).cover,
        Some("https://c.example/een.jpg")
    );
    let dark = title(Kind::Series, 9);
    assert_eq!(
        (dark.cover, dark.genres, dark.year),
        (Some("https://c.example/dark.jpg"), Genre::Drama.bit(), 2017)
    );
}

#[test]
fn f_still_finds_a_film_by_its_full_name() {
    let store = CatalogStore::from_catalog(idle_portal(), library());
    let found: Vec<String> = store.search("matrix").into_iter().map(|i| i.name).collect();
    assert_eq!(
        found,
        ["The Matrix (1999) 1080p", "NL | The Matrix (1999) [4K]"]
    );
}

/// Two live channels, to zap through.
const LIVE: &str = r#"[{"stream_id":1,"name":"NL | Een"},{"stream_id":2,"name":"NL | Twee"}]"#;

/// A portal on a mock with these films, two live channels and no series; its
/// film categories are `vod_categories` (`None`: refused with 503), answered
/// after `delay`.
async fn mock_portal(
    films: &str,
    vod_categories: Option<&str>,
    delay: Duration,
) -> (MockServer, Arc<dyn Portal>) {
    let server = MockServer::start().await;
    let answer = |action: &str, body: &str| {
        Mock::given(method("GET"))
            .and(path("/player_api.php"))
            .and(query_param("action", action))
            .respond_with(ResponseTemplate::new(200).set_body_string(body.to_owned()))
    };
    answer("get_live_streams", LIVE).mount(&server).await;
    answer("get_vod_streams", films).mount(&server).await;
    answer("get_series", "[]").mount(&server).await;
    answer("get_series_categories", "[]").mount(&server).await;
    let categories = match vod_categories {
        Some(body) => ResponseTemplate::new(200).set_body_string(body.to_owned()),
        None => ResponseTemplate::new(503),
    };
    Mock::given(query_param("action", "get_vod_categories"))
        .respond_with(categories.set_delay(delay))
        .mount(&server)
        .await;
    let uri = url::Url::parse(&server.uri()).unwrap();
    let portal = XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: uri.host_str().unwrap().into(),
        port: uri.port().unwrap(),
    });
    (server, Arc::new(portal))
}

/// A store on [`mock_portal`], whose categories answer at once.
async fn portal_with(films: &str, vod_categories: Option<&str>) -> (MockServer, Arc<CatalogStore>) {
    let (server, portal) = mock_portal(films, vod_categories, Duration::ZERO).await;
    (server, Arc::new(CatalogStore::new(portal)))
}

/// Waits (2 s at most) until the store's catalog is in.
async fn loaded(store: &CatalogStore) {
    for _ in 0..400 {
        if store.status() == CatalogStatus::Loaded {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the catalog never loaded: {:?}", store.status());
}

/// Waits (2 s at most) until the store's titles are in: they come after the
/// catalog.
async fn titled(store: &CatalogStore) {
    for _ in 0..400 {
        if store.titles().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the titles never came: {:?}", store.status());
}

const FILMS: &str = r#"[{"stream_id":5,"name":"Lachen (2020)","category_id":"12"}]"#;

/// The film list's one category.
const KOMEDIE: &str = r#"[{"category_id":"12","category_name":"NL | Komedie"}]"#;

#[tokio::test]
async fn the_fetch_gives_the_titles_the_genres_of_their_categories() {
    let (_server, store) = portal_with(FILMS, Some(KOMEDIE)).await;
    store.spawn_fetch();
    titled(&store).await;
    let titles = store.titles().unwrap();
    let lachen = titles.title(0).unwrap();
    assert_eq!(lachen.name, "Lachen");
    assert_eq!(lachen.genres, Genre::Comedy.bit());
    assert!(lachen.dutch, "NL | in the category's name");
    assert_eq!(lachen.category.map(|c| c.name), Some("NL | Komedie"));
    assert_eq!(titles.categories().count(), 1);
}

#[tokio::test]
async fn a_refused_category_list_leaves_the_titles_without_genres() {
    let (_server, store) = portal_with(FILMS, None).await;
    store.spawn_fetch();
    titled(&store).await;
    let titles = store.titles().unwrap();
    assert_eq!(titles.title(0).unwrap().genres, 0);
    assert_eq!(titles.categories().count(), 0);
}

#[tokio::test]
async fn a_stalled_category_list_holds_up_neither_the_catalog_nor_zapping() {
    let (_server, portal) = mock_portal(FILMS, Some(KOMEDIE), Duration::from_secs(30)).await;
    let store = Arc::new(CatalogStore::new(portal));
    store.spawn_fetch();
    loaded(&store).await;
    assert!(store.is_loaded());
    assert_eq!(store.zap_channels().len(), 2, "zapping works");
    assert_eq!(store.search("lachen").len(), 1, "f works");
    assert!(
        store.titles().is_none(),
        "the titles wait for the categories"
    );
}

#[tokio::test]
async fn the_titles_come_after_the_categories() {
    let (_server, portal) = mock_portal(FILMS, Some(KOMEDIE), Duration::from_millis(600)).await;
    let store = Arc::new(CatalogStore::new(portal));
    store.spawn_fetch();
    loaded(&store).await;
    assert!(store.titles().is_none(), "the categories are not in yet");
    titled(&store).await;
    let lachen = store.titles().unwrap().title(0).unwrap().genres;
    assert_eq!(lachen, Genre::Comedy.bit(), "built with them");
}

#[tokio::test]
async fn at_the_bound_the_titles_are_built_without_the_categories() {
    let (_server, portal) = mock_portal(FILMS, Some(KOMEDIE), Duration::from_secs(30)).await;
    let wait = Duration::from_millis(200);
    let store = Arc::new(CatalogStore::new(portal).with_categories_wait(wait));
    store.spawn_fetch();
    titled(&store).await;
    let titles = store.titles().unwrap();
    assert_eq!(titles.title(0).unwrap().genres, 0, "no genres");
    assert_eq!(titles.categories().count(), 0, "no categories");
}

#[test]
fn the_channels_wait_for_nothing_while_the_titles_are_built() {
    // One blocking thread, which the test holds: the build of the titles has
    // to wait for it, the catalog must not.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_server, store) = portal_with(FILMS, Some("[]")).await;
        let (release, held) = std::sync::mpsc::channel::<()>();
        let holder = tokio::task::spawn_blocking(move || held.recv());
        store.spawn_fetch();
        loaded(&store).await;
        // The lists are in: the catalog is there, zapping and `f` work, and
        // the titles are still being built.
        assert!(store.is_loaded());
        assert_eq!(store.zap_channels().len(), 2, "zapping works");
        assert_eq!(store.search("lachen").len(), 1, "f works");
        assert!(store.titles().is_none(), "the titles are not built yet");
        assert_eq!(store.titles_revision(), 0);
        release.send(()).unwrap();
        holder.await.unwrap().unwrap();
        // ... and then they come.
        titled(&store).await;
        assert_eq!(store.titles().unwrap().title(0).unwrap().name, "Lachen");
        assert_eq!(store.titles_revision(), 1);
    });
}

/// What the log said on this thread while `work` ran.
fn logged(work: impl FnOnce()) -> String {
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // A second dispatcher, as app.rs's LogCapture has: with this test's one
    // alone, another test's thread (no subscriber) that hits the `vod:` line
    // first would silence it here for good.
    let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let sink = Sink::default();
    let writer = sink.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, work);
    let bytes = sink.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn the_log_says_what_the_lists_gave() {
    let catalog = Catalog {
        movies: vec![
            Movie {
                stream_icon: Some("https://x/1.jpg".into()),
                rating: 70,
                ..film(1, "A (2001)", false)
            },
            film(2, "B", false),
        ],
        ..Catalog::default()
    };
    let text = logged(|| {
        CatalogStore::from_catalog(idle_portal(), catalog);
    });
    let line = text
        .lines()
        .find(|l| l.contains("vod: "))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        line.contains("vod: 2 titles (0 series, 2 films in 2 versions, 0 left out), 0 categories - cover 50 %, rating 50 %, year 50 %, genre 0 %, Dutch 0 %, English 0 % - index "),
        "{line}"
    );
    assert!(
        line.contains(" KB, covers ") && line.contains(" ms"),
        "{line}"
    );
    assert!(!line.contains("https://"), "no address: {line}");
}

#[test]
fn a_category_list_that_fails_is_logged_without_its_address() {
    // The fetch runs on this thread (the runtime has one), so its lines are
    // this thread's.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let text = logged(|| {
        runtime.block_on(async {
            // No JSON: the answer's reader fails, with the request's address.
            let (_server, store) = portal_with(FILMS, Some("not a list")).await;
            store.spawn_fetch();
            titled(&store).await;
        })
    });
    let line = text
        .lines()
        .find(|l| l.contains("catalog: no categories ("))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        !line.contains("http://") && !line.contains("player_api"),
        "no address: {line}"
    );
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_vod_catalog`
Expected: compile errors: `CatalogStore` has no method `titles`, `titles_revision` or `with_categories_wait`.

- [ ] **Step 3: What the lists give up, and the stored catalog**

In `rust/src/vod/index.rs`, after `fn day_of`, add what a catalog's lists give up for the index (`Extras`); `TitleIndex::build` keeps its signature and calls the new `build_with`, which takes over its body. Replace

```rust
impl TitleIndex {
    /// The titles of these lists. `max_year` is the latest year a name may
    /// give: the year after this one.
    pub fn build(
        movies: &[Movie],
        series: &[Series],
        categories: &Categories,
        max_year: u16,
    ) -> Self {
        let cats: Vec<CategoryIn<'_>> = categories
```

with

```rust
/// What a catalog's lists give up for its title index (Ruling 22): a film's
/// cover, and a series' cover, genre field and release year, moved out of
/// their records ([`Extras::take`]). The catalog keeps the lists without them
/// and builds its titles from both ([`TitleIndex::build_with`]): no list is
/// copied for the build, and no address is held twice once the index has it.
#[derive(Default)]
pub struct Extras {
    /// The films' covers, in the films' order.
    film_covers: Vec<Option<String>>,
    /// The series' own, in the series' order.
    series: Vec<SeriesExtras>,
}

/// A series' share of [`Extras`].
struct SeriesExtras {
    cover: Option<String>,
    genre: Option<String>,
    /// The year of its release date; 0 for none.
    released: u16,
}

impl Extras {
    /// Moves them out of the records of these lists: the strings move, and
    /// none is copied. A series' release dates go; their year stays.
    pub fn take(movies: &mut [Movie], series: &mut [Series]) -> Self {
        let film_covers = movies.iter_mut().map(|m| m.stream_icon.take()).collect();
        let series = series
            .iter_mut()
            .map(|s| {
                let released = s.released().map_or(0, crate::portal::vod_info::year_of);
                s.release_date = None;
                s.release_date_camel = None;
                SeriesExtras {
                    cover: s.cover.take(),
                    genre: s.genre.take(),
                    released,
                }
            })
            .collect();
        Self {
            film_covers,
            series,
        }
    }
}

impl TitleIndex {
    /// The titles of these lists. `max_year` is the latest year a name may
    /// give: the year after this one.
    pub fn build(
        movies: &[Movie],
        series: &[Series],
        categories: &Categories,
        max_year: u16,
    ) -> Self {
        Self::build_with(movies, series, &Extras::default(), categories, max_year)
    }

    /// [`TitleIndex::build`] for lists whose records gave up what `extras`
    /// holds: the catalog's build, which reads its stored lists and copies
    /// none of them. A record that `extras` does not cover keeps its own.
    pub fn build_with(
        movies: &[Movie],
        series: &[Series],
        extras: &Extras,
        categories: &Categories,
        max_year: u16,
    ) -> Self {
        let cats: Vec<CategoryIn<'_>> = categories
```

In `rust/src/vod/index.rs`, in `TitleIndex::build_with`, in the loop over a film title's versions: its cover, from the extras when the list gave it up. Replace

```rust
                if entry.cover.is_empty() {
                    entry.cover = m.stream_icon.as_deref().map_or("", str::trim);
                }
```

with

```rust
                if entry.cover.is_empty() {
                    let cover = match extras.film_covers.get(v.index as usize) {
                        Some(given_up) => given_up.as_deref(),
                        None => m.stream_icon.as_deref(),
                    };
                    entry.cover = cover.map_or("", str::trim);
                }
```

In `rust/src/vod/index.rs`, in `TitleIndex::build_with`, the loop over the series: each with its share of the extras. Replace

```rust
        for s in series {
            let Ok(id) = u32::try_from(s.series_id) else {
```

with

```rust
        for (n, s) in series.iter().enumerate() {
            let given_up = extras.series.get(n);
            let Ok(id) = u32::try_from(s.series_id) else {
```

In `rust/src/vod/index.rs`, in the same loop, its genre field. Replace

```rust
            let field = s.genre.as_deref().unwrap_or("");
```

with

```rust
            let field = given_up
                .map_or(s.genre.as_deref(), |x| x.genre.as_deref())
                .unwrap_or("");
```

In `rust/src/vod/index.rs`, in the same loop, its release year. Replace

```rust
            let released = s.released().map_or(0, crate::portal::vod_info::year_of);
            let spoken = [
```

with

```rust
            let released = match given_up {
                Some(x) => x.released,
                None => s.released().map_or(0, crate::portal::vod_info::year_of),
            };
            let spoken = [
```

In `rust/src/vod/index.rs`, in the same loop, its cover. Replace

```rust
                cover: s.cover.as_deref().map_or("", str::trim),
```

with

```rust
                cover: given_up
                    .map_or(s.cover.as_deref(), |x| x.cover.as_deref())
                    .map_or("", str::trim),
```

In `rust/src/catalog.rs`, the imports. Replace

```rust
use crate::portal::{Catalog as PortalCatalog, LiveChannel, Movie, Portal, PortalError, Series};
use crate::search::{rank, ItemKind, SearchItem};
use crate::sources::SourceKind;
use parking_lot::{Mutex, RwLock};
```

with

```rust
use crate::portal::{
    Catalog as PortalCatalog, Categories, LiveChannel, Movie, Portal, PortalError, Series,
};
use crate::search::{rank, ItemKind, SearchItem};
use crate::sources::SourceKind;
use crate::vod::index::{CategoryInfo, Extras, Learned, Query, Results, Title, TitleIndex};
use crate::vod::TitleId;
use parking_lot::{Mutex, RwLock};
```

In `rust/src/catalog.rs`, the imports, `Duration` beside `Instant`. Replace

```rust
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};
```

with

```rust
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};
```

In `rust/src/catalog.rs`, in `struct StoredCatalog`, the lists are shared with the build of their titles. Replace

```rust
    /// The films and series; its `live` is empty (see `live`).
    catalog: PortalCatalog,
```

with

```rust
    /// The films and series; its `live` is empty (see `live`). Shared with the
    /// build of their titles, which reads them once the catalog is in
    /// (`TitleInput`).
    catalog: Arc<PortalCatalog>,
```

In `rust/src/catalog.rs`, `StoredCatalog::new` hands back what the titles are built from. Replace

```rust
    /// The one way a catalog gets stored: the fetch and `from_catalog` both come
    /// through here, so what is worked out cannot differ between them.
    fn new(mut catalog: PortalCatalog) -> Self {
```

with

```rust
    /// The one way a catalog gets stored: the fetch and `from_catalog` both come
    /// through here, so what is worked out cannot differ between them. What only
    /// the title index reads leaves the lists here, and comes back as the
    /// [`TitleInput`] of their titles, which are built once the catalog is in.
    fn new(mut catalog: PortalCatalog) -> (Self, TitleInput) {
```

In `rust/src/catalog.rs`, in `StoredCatalog::new`, after the live channels are taken out. Replace

```rust
        let live = Arc::new(std::mem::take(&mut catalog.live));
        Self {
            catalog,
```

with

```rust
        let live = Arc::new(std::mem::take(&mut catalog.live));
        // The covers, a series' genre and release date are the index's alone
        // (Ruling 22): they move out of the stored lists, to its build.
        let extras = Extras::take(&mut catalog.movies, &mut catalog.series);
        let catalog = Arc::new(catalog);
        let titles = TitleInput {
            lists: catalog.clone(),
            extras,
        };
        let stored = Self {
            catalog,
```

In `rust/src/catalog.rs`, at the end of `StoredCatalog::new`, in the struct it returns. Replace

```rust
            zap: Mutex::new(None),
        }
    }
```

with

```rust
            zap: Mutex::new(None),
        };
        (stored, titles)
    }
```

In `rust/src/catalog.rs`, after `impl std::ops::Deref for StoredCatalog`, add what the titles are built from, `latest_year`, the log line and the `Titles` handle. After

```rust
impl std::ops::Deref for StoredCatalog {
    type Target = PortalCatalog;

    fn deref(&self) -> &PortalCatalog {
        &self.catalog
    }
}

```

add:

```rust
/// What the titles of a stored catalog are built from ([`TitleInput::build`]):
/// its lists, shared with it through their `Arc`, and what only the index
/// reads, which the lists gave up before they were stored, so that no address
/// is held twice once the index has it (Ruling 22).
struct TitleInput {
    lists: Arc<PortalCatalog>,
    extras: Extras,
}

impl TitleInput {
    /// The title index of these lists, with the portal's `categories`, and its
    /// `vod:` line: on the blocking pool in the fetch, once the catalog is put
    /// and its categories are in (up to ~150 ms for a big portal); in place in
    /// `from_catalog`. It reads the stored lists and copies none of them.
    fn build(self, categories: &Categories) -> TitleIndex {
        let t0 = Instant::now();
        let titles = TitleIndex::build_with(
            &self.lists.movies,
            &self.lists.series,
            &self.extras,
            categories,
            latest_year(),
        );
        if !titles.is_empty() {
            log_titles(&titles, categories, t0.elapsed());
        }
        titles
    }
}

/// The latest year a title's name may give: the year after this one ("Blade
/// Runner 2049" is no film of 2049).
fn latest_year() -> u16 {
    use chrono::Datelike;
    (chrono::Utc::now().year() + 1).clamp(1900, u16::MAX as i32) as u16
}

/// What the lists gave the title index, in one line (spec 2026-10-15 §11: the
/// quality of the portal's data shows only with real titles).
fn log_titles(titles: &TitleIndex, categories: &Categories, took: std::time::Duration) {
    let s = titles.stats();
    let n = titles.len().max(1);
    let pct = |part: usize| part * 100 / n;
    info!(
        "vod: {} titles ({} series, {} films in {} versions, {} left out), {} categories - cover {} %, rating {} %, year {} %, genre {} %, Dutch {} %, English {} % - index {} KB, covers {} KB, built in {} ms",
        titles.len(),
        s.series,
        s.films,
        s.versions,
        s.skipped,
        categories.vod.len() + categories.series.len(),
        pct(s.with_cover),
        pct(s.with_rating),
        pct(s.with_year),
        pct(s.with_genre),
        pct(s.dutch),
        pct(s.english),
        titles.heap_bytes() / 1024,
        titles.cover_bytes() / 1024,
        took.as_millis()
    );
}

/// The thuisbioscoop's titles as the adult lock lets them show (spec
/// 2026-10-15 §4.2). Every answer leaves the adult titles out while locked:
/// this is the only way out of the catalog for the index, so nothing else
/// filters. Asked again after the lock changes.
#[derive(Clone)]
pub struct Titles {
    index: Arc<TitleIndex>,
    unlocked: bool,
}

impl Titles {
    /// The titles that answer `query`, with the counts of its filters.
    pub fn search(&self, query: &Query) -> Results {
        self.index.search(query, self.unlocked)
    }

    /// The title at `pos`; none for an adult one while locked.
    pub fn title(&self, pos: usize) -> Option<Title<'_>> {
        self.index.title(pos).filter(|t| self.unlocked || !t.adult)
    }

    /// Where title `id` is, when it shows.
    pub fn find(&self, id: TitleId) -> Option<usize> {
        self.index.find(id).filter(|&pos| self.title(pos).is_some())
    }

    /// The film title a version plays, when it shows.
    pub fn title_of_version(&self, stream_id: i64) -> Option<usize> {
        self.index
            .title_of_version(stream_id)
            .filter(|&pos| self.title(pos).is_some())
    }

    /// Titles like the one at `pos` ([`TitleIndex::more_like`]).
    pub fn more_like(
        &self,
        pos: usize,
        limit: usize,
        watched: &HashSet<TitleId>,
        disliked: &HashSet<TitleId>,
    ) -> Vec<u32> {
        if self.title(pos).is_none() {
            return Vec::new();
        }
        self.index
            .more_like(pos, limit, self.unlocked, watched, disliked)
    }

    /// What title `pos`'s details tell the index ([`TitleIndex::learn`]).
    pub fn learn(&self, pos: usize, learned: Learned) {
        self.index.learn(pos, learned);
    }

    /// The portal's categories.
    pub fn categories(&self) -> impl Iterator<Item = CategoryInfo<'_>> + '_ {
        self.index.categories()
    }
}

```

In `rust/src/catalog.rs`, in the test `a_stored_catalog_knows_its_adult_channels_by_the_portals_flag_alone` (`mod zap_tests`), `StoredCatalog::new` hands back the titles' input too. Replace

```rust
        let stored = StoredCatalog::new(PortalCatalog {
```

with

```rust
        let (stored, _) = StoredCatalog::new(PortalCatalog {
```

- [ ] **Step 4: The store: the catalog at once, then its titles once the categories are in**

In `rust/src/catalog.rs`, in `pub struct CatalogStore`, after `defect_revision`, the titles and their count. Replace

```rust
    defect_revision: AtomicU64,
}
```

with

```rust
    defect_revision: AtomicU64,
    /// The thuisbioscoop's titles (`vod::index`), adult ones included: they
    /// leave the store only as `Titles`, which apply the lock. Put after the
    /// catalog they come from (`put_titles`).
    titles: Arc<RwLock<Option<Arc<TitleIndex>>>>,
    /// How many times `titles` was put (see [`CatalogStore::titles_revision`]).
    titles_revision: Arc<AtomicU64>,
    /// How long the titles wait for the category lists once the catalog is in
    /// ([`CatalogStore::with_categories_wait`]).
    categories_wait: Duration,
}
```

In `rust/src/catalog.rs`, `put_catalog` says which count it put the catalog at, and after it `put_titles`. Replace

```rust
/// Puts a catalog in its slot and counts it. The count moves after the catalog
/// is in, so whoever sees the new count finds the new catalog.
fn put_catalog(slot: &RwLock<Option<StoredCatalog>>, revision: &AtomicU64, stored: StoredCatalog) {
    *slot.write() = Some(stored);
    revision.fetch_add(1, Ordering::Release);
}
```

with

```rust
/// Puts a catalog in its slot and counts it. The count moves after the catalog
/// is in, so whoever sees the new count finds the new catalog. The count it put
/// the catalog at: its titles are put for that one ([`put_titles`]).
fn put_catalog(
    slot: &RwLock<Option<StoredCatalog>>,
    revision: &AtomicU64,
    stored: StoredCatalog,
) -> u64 {
    *slot.write() = Some(stored);
    revision.fetch_add(1, Ordering::Release) + 1
}

/// Puts the titles built from the catalog put at count `of` in their slot and
/// counts them, as `put_catalog` does, when that catalog is still the store's
/// (`revision`): a newer one's own titles come after it.
fn put_titles(
    slot: &RwLock<Option<Arc<TitleIndex>>>,
    titles_revision: &AtomicU64,
    revision: &AtomicU64,
    of: u64,
    titles: Arc<TitleIndex>,
) {
    let mut held = slot.write();
    if revision.load(Ordering::Acquire) != of {
        return;
    }
    *held = Some(titles);
    titles_revision.fetch_add(1, Ordering::Release);
}

/// How long the titles wait for the portal's category lists once the catalog
/// is in: then they are built without them (spec 2026-10-15 §5: the channels
/// wait for none of it, and the titles not for long).
const CATEGORIES_WAIT: Duration = Duration::from_secs(15);

/// The category lists the fetch asked beside the three lists, once its catalog
/// is in: what they brought; none when they fail or have not come within
/// `wait`, and then their request stops. The warn line names no address
/// (`api_json`).
async fn categories_after(
    mut asked: tokio::task::JoinHandle<Result<Categories, PortalError>>,
    wait: Duration,
) -> Categories {
    let why = match tokio::time::timeout(wait, &mut asked).await {
        Ok(Ok(Ok(categories))) => return categories,
        Ok(Ok(Err(e))) => e.to_string(),
        Ok(Err(e)) => e.to_string(),
        Err(_) => {
            asked.abort();
            format!("none within {wait:?}")
        }
    };
    warn!("catalog: no categories ({why}) - the titles go without");
    Categories::default()
}
```

In `rust/src/catalog.rs`, in `CatalogStore::new`, start the new fields, and after `new` add `with_categories_wait`. Replace

```rust
            defect_revision: AtomicU64::new(0),
        }
    }
```

with

```rust
            defect_revision: AtomicU64::new(0),
            titles: Arc::new(RwLock::new(None)),
            titles_revision: Arc::new(AtomicU64::new(0)),
            categories_wait: CATEGORIES_WAIT,
        }
    }

    /// How long this store's titles wait for the portal's category lists once
    /// its catalog is in: 15 s, a test less. Then they are built without them.
    pub fn with_categories_wait(mut self, wait: Duration) -> Self {
        self.categories_wait = wait;
        self
    }
```

In `rust/src/catalog.rs`, `CatalogStore::from_catalog`: the titles too, built in place. Replace

```rust
    /// A store with a catalog already loaded (tests, tools).
    pub fn from_catalog(portal: Arc<dyn Portal>, catalog: PortalCatalog) -> Self {
        let store = Self::new(portal);
        put_catalog(&store.inner, &store.revision, StoredCatalog::new(catalog));
```

with

```rust
    /// A store with a catalog already loaded (tests, tools), and its titles,
    /// built in place. It has no categories: its films take no genre from them.
    pub fn from_catalog(portal: Arc<dyn Portal>, catalog: PortalCatalog) -> Self {
        let store = Self::new(portal);
        let (stored, input) = StoredCatalog::new(catalog);
        let of = put_catalog(&store.inner, &store.revision, stored);
        let titles = Arc::new(input.build(&Categories::default()));
        put_titles(
            &store.titles,
            &store.titles_revision,
            &store.revision,
            of,
            titles,
        );
```

In `rust/src/catalog.rs`, after `CatalogStore::revision`, add `titles_revision`. After

```rust
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
```

add:

```rust

    /// How many times this store's titles were put: 0 until the first
    /// catalog's are built, then one more with every build. Whatever is worked
    /// out from the titles compares this per frame, an atomic load, as the
    /// guide does [`CatalogStore::revision`].
    pub fn titles_revision(&self) -> u64 {
        self.titles_revision.load(Ordering::Acquire)
    }
```

In `rust/src/catalog.rs`, in `CatalogStore::spawn_fetch`, with the other clones: after the line `let notice = self.notice.clone();`, add:

```rust
        let titles = self.titles.clone();
        let titles_revision = self.titles_revision.clone();
        let categories_wait = self.categories_wait;
```

In `rust/src/catalog.rs`, in `CatalogStore::spawn_fetch`, the saved M3U list. Replace

```rust
                if let Some(saved) = portal.saved_catalog().await {
                    put_catalog(&inner, &revision, StoredCatalog::new(saved));
```

with

```rust
                if let Some(saved) = portal.saved_catalog().await {
                    // An M3U list has no films or series: its titles come with
                    // the fresh list's.
                    let (stored, _) = StoredCatalog::new(saved);
                    put_catalog(&inner, &revision, stored);
```

In `rust/src/catalog.rs`, in `CatalogStore::spawn_fetch`, the two category lists asked beside the three lists. Before

```rust
            match portal.fetch_catalog().await {
                Ok(c) => {
```

add:

```rust
            // The two category lists are asked beside the three lists (spec
            // 2026-10-15 §4.1, §5), in a task of their own: only the titles
            // wait for them.
            let asker = portal.clone();
            let categories = tokio::spawn(async move { asker.fetch_categories().await });
```

In `rust/src/catalog.rs`, in `CatalogStore::spawn_fetch`, in the `Ok(c)` arm, after the archive dump: the catalog at once, then its titles once the categories are in. Replace

```rust
                    // Worked out before the lock is taken: readers never wait for it.
                    let stored = StoredCatalog::new(c);
                    put_catalog(&inner, &revision, stored);
                    *status.write() = CatalogStatus::Loaded;
                }
```

with

```rust
                    // Worked out before the lock is taken: readers never wait for it.
                    let (stored, input) = StoredCatalog::new(c);
                    let of = put_catalog(&inner, &revision, stored);
                    *status.write() = CatalogStatus::Loaded;
                    // The channels, zapping, `f` and the guide work from here on,
                    // whatever the categories do. The titles wait for them (15 s
                    // at most), then take up to ~150 ms for a big portal on the
                    // blocking pool: off the screen and off the async workers,
                    // where the relay runs.
                    let categories = categories_after(categories, categories_wait).await;
                    let build = move || input.build(&categories);
                    match tokio::task::spawn_blocking(build).await {
                        Ok(built) => {
                            put_titles(&titles, &titles_revision, &revision, of, Arc::new(built));
                        }
                        Err(e) => warn!("vod: titles not built ({e})"),
                    }
                }
```

In `rust/src/catalog.rs`, in `CatalogStore::spawn_fetch`, the two arms without a new catalog: nothing waits for the categories. Replace

```rust
                // The same list as the one that shows: it stays as it is.
                Err(PortalError::Unchanged) => *status.write() = CatalogStatus::Loaded,
                Err(e) => {
```

with

```rust
                // The same list as the one that shows: it stays as it is, and
                // no titles wait for the categories.
                Err(PortalError::Unchanged) => {
                    categories.abort();
                    *status.write() = CatalogStatus::Loaded;
                }
                Err(e) => {
                    categories.abort();
```

In `rust/src/catalog.rs`, after `CatalogStore::search`, add `titles`. After

```rust
    pub fn search(&self, query: &str) -> Vec<SearchItem> {
        let items = self.search_items();
        rank(query, &items).into_iter().cloned().collect()
    }
```

add:

```rust

    /// The thuisbioscoop's titles as the lock stands now ([`Titles`]): those of
    /// the store's catalog once they are built (`put_titles`); none before.
    pub fn titles(&self) -> Option<Titles> {
        let unlocked = self.adult_unlocked();
        let held = self.titles.read();
        held.as_ref().map(|index| Titles {
            index: index.clone(),
            unlocked,
        })
    }
```

At the point where the catalog is put, the zenderlijst's later tasks may have added fields to `StoredCatalog` or work to `StoredCatalog::new` (the channel list's groups): keep them, and keep the order of this step: the catalog first, then the categories, then its titles.

- [ ] **Step 5: Run the tests to see them pass**

Run: `ct --test test_vod_catalog --test test_catalog_adult --test test_catalog_m3u --test test_zap_ring`, then `ct --lib catalog::`
Expected: all pass (12 in `test_vod_catalog`). `the_channels_wait_for_nothing_while_the_titles_are_built` holds the runtime's one blocking thread: the catalog loads, zaps and searches while the titles wait for it, and they come once it is free. Three tests answer the film categories late: after 30 s, where the catalog loads, zaps and searches at once while the titles wait; after 600 ms, where the titles come after them, with their genres; and after 30 s with a bound of 200 ms, where the titles come without them. `what_the_stored_lists_give_up_reaches_the_titles` checks that a cover, a series' genre and its year reach the titles from the extras.

- [ ] **Step 6: Format, lint, full suite**

- Run `fmt src/vod/mod.rs tests/test_vod_catalog.rs`. Expected: no output.
- Run `hunks src/catalog.rs`. Expected: the count Task 1 Step 1 recorded (`3` at c81d7b0).
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 7: Commit**

Message file (in the scratchpad):
```
feat(rust): the catalog asks the category lists beside the lists and puts the catalog at once; its titles wait for the categories (15 s at most), are built on the blocking pool from the stored lists, copying none, log what the lists gave, and are handed out only behind the lock

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/catalog.rs rust/src/vod/index.rs rust/tests/test_vod_catalog.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/catalog.rs rust/src/vod/index.rs rust/tests/test_vod_catalog.rs`

---

### Task 6a: The index cache: the file

The title index in a file per portal (spec §4.6, Rulings 31 and 51), pure file work: what goes in, what never does (an adult title, a cover address that holds an account or a query), and what a file says that nothing may trust (genre bits past the 24, a category it does not list, labels with stray bits, more containers than codes). It is written as it is serialized, from the index's own text, through `write_atomic_with`, the one atomic writer of `vod/` (Ruling 52). Task 6b gives the file to the catalog.

**Files:**
- Create: `rust/src/vod/cache.rs`
- Modify: `rust/src/vod/mod.rs` (`pub mod cache;`, `write_atomic_with`), `rust/src/vod/index.rs` (`category_code`)
- Test: `rust/tests/test_vod_cache.rs` (new; 9 tests)

**Interfaces:**
- Consumes: Task 3's `TitleIndex::{assemble, title, categories, len}`, `Title`, `Entry`, `VersionIn`, `CategoryIn`, `category_rec`; Task 2a's `Genre`; `crate::args::XtreamCreds`.
- Produces:
  ```rust
  // vod/cache.rs
  pub fn portal_key(host: &str, port: u16) -> String;                               // "<host>_<port>"
  pub fn path_for(cache_dir: &Path, creds: Option<&XtreamCreds>) -> Option<PathBuf>;  // cache\vod-<key>.json
  pub fn save(path: &Path, index: &TitleIndex, saved_at: i64) -> std::io::Result<(usize, u64)>;
  pub fn load(path: &Path) -> Result<Option<(TitleIndex, i64)>, String>;
  // private: CacheFile / CachedTitle (read), CacheFileRef / CachedTitleRef<'a> / TitlesRef<'a> (written)
  // vod/index.rs
  pub(crate) fn category_code(&self, pos: usize) -> u16;
  // vod/mod.rs
  pub(crate) fn write_atomic_with(path: &Path,
      write: impl FnOnce(&mut BufWriter<File>) -> io::Result<()>) -> io::Result<()>;   // <name>.json.tmp, renamed
  ```

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_vod_cache.rs`:

```rust
//! The title index in a file (vod/cache.rs, spec 2026-10-15 §4.6): what goes
//! in and what never does, a file that is missing or broken, and what a file
//! says that nothing may trust.

use bongo_player::args::XtreamCreds;
use bongo_player::portal::{Categories, Category, Movie, Series};
use bongo_player::vod::cache::{load, path_for, portal_key, save};
use bongo_player::vod::genres::{Genre, Labels};
use bongo_player::vod::index::{Query, TitleIndex};
use bongo_player::vod::{Kind, TitleId};
use std::collections::HashSet;
use std::path::Path;

const SAVED_AT: i64 = 1_792_022_400;

fn library() -> TitleIndex {
    let movies = [
        Movie {
            stream_id: 1,
            name: "NL | The Matrix (1999) [4K]".into(),
            container_extension: Some("mkv".into()),
            stream_icon: Some("https://img.example/m.jpg".into()),
            rating: 82,
            added: SAVED_AT,
            category_id: 12,
            ..Movie::default()
        },
        Movie {
            stream_id: 2,
            name: "The Matrix (1999) 1080p".into(),
            container_extension: Some("mp4".into()),
            ..Movie::default()
        },
        Movie {
            stream_id: 3,
            name: "Night Club (2020)".into(),
            is_adult: true,
            ..Movie::default()
        },
        Movie {
            stream_id: 4,
            name: "Secret (2021)".into(),
            stream_icon: Some("http://h:80/movie/u/p/4.jpg".into()),
            ..Movie::default()
        },
    ];
    let shows = [Series {
        series_id: 9,
        name: "Dark".into(),
        genre: Some("Drama".into()),
        release_date_camel: Some("2017-12-01".into()),
        ..Series::default()
    }];
    let categories = Categories {
        vod: vec![Category {
            category_id: 12,
            category_name: "NL | Komedie".into(),
        }],
        series: Vec::new(),
    };
    TitleIndex::build(&movies, &shows, &categories, 2027)
}

fn film(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Film,
        id,
    }
}

#[test]
fn what_is_written_is_read_back_without_adult_titles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache").join("vod-h_80.json");
    let index = library();
    let (n, bytes) = save(&path, &index, SAVED_AT).unwrap();
    assert_eq!(n, 3, "the adult title stays out");
    assert!(bytes > 0);
    assert!(!path.with_extension("json.tmp").exists());
    let (back, saved_at) = load(&path).unwrap().unwrap();
    assert_eq!(saved_at, SAVED_AT);
    assert_eq!(back.len(), 3);
    assert_eq!(back.find(film(3)), None);
    let matrix = back.title(back.find(film(1)).unwrap()).unwrap();
    let was = index.title(index.find(film(1)).unwrap()).unwrap();
    assert_eq!(
        (
            matrix.name,
            matrix.cover,
            matrix.year,
            matrix.rating,
            matrix.genres
        ),
        (was.name, was.cover, was.year, was.rating, was.genres)
    );
    assert_eq!(matrix.genres, Genre::Comedy.bit());
    assert_eq!(
        (matrix.added_day, matrix.dutch, matrix.english),
        (was.added_day, true, false)
    );
    assert_eq!(
        matrix.category.map(|c| (c.kind, c.id, c.name)),
        Some((Kind::Film, 12, "NL | Komedie"))
    );
    let versions: Vec<(i64, Labels, &str)> = matrix
        .versions
        .iter()
        .map(|v| (v.stream_id, v.labels, v.ext))
        .collect();
    assert_eq!(
        versions,
        [
            (1, Labels::UHD.with(Labels::NL), "mkv"),
            (2, Labels::FHD, "mp4")
        ]
    );
    let dark = back
        .title(
            back.find(TitleId {
                kind: Kind::Series,
                id: 9,
            })
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        (dark.name, dark.year, dark.genres),
        ("Dark", 2017, Genre::Drama.bit())
    );
    // The search works on what was read: its search names are built again.
    let found = back.search(
        &Query {
            text: "matrix".into(),
            ..Query::default()
        },
        false,
    );
    assert_eq!(found.titles.len(), 1);
}

#[test]
fn a_cover_with_an_account_in_it_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    save(&path, &library(), SAVED_AT).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("/movie/u/p/"), "{text}");
    let (back, _) = load(&path).unwrap().unwrap();
    let secret = back.title(back.find(film(4)).unwrap()).unwrap();
    assert_eq!(secret.cover, None, "its tile shows the placeholder");
}

#[test]
fn a_cover_with_a_token_in_its_query_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    let movies = [Movie {
        stream_id: 1,
        name: "Tokened (2020)".into(),
        stream_icon: Some("https://cdn.example/p.jpg?token=secret".into()),
        ..Movie::default()
    }];
    let index = TitleIndex::build(&movies, &[], &Categories::default(), 2027);
    save(&path, &index, SAVED_AT).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("token="),
        "redact sees no token there: {text}"
    );
    let (back, _) = load(&path).unwrap().unwrap();
    assert_eq!(
        back.title(0).unwrap().cover,
        None,
        "its tile shows the placeholder"
    );
}

#[test]
fn a_missing_file_is_no_cache_and_a_broken_one_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load(&dir.path().join("none.json")).unwrap().is_none());
    let broken = dir.path().join("broken.json");
    std::fs::write(&broken, b"{\"version\":1,\"saved_at\":").unwrap();
    assert!(load(&broken).is_err());
    let other = dir.path().join("other.json");
    std::fs::write(
        &other,
        br#"{"version":99,"saved_at":0,"categories":[],"titles":[]}"#,
    )
    .unwrap();
    let Err(e) = load(&other) else {
        panic!("another version was read");
    };
    assert!(e.contains("99"), "{e}");
}

#[test]
fn a_write_that_fails_leaves_no_temporary_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::create_dir(&path).unwrap();
    assert!(save(&path, &library(), SAVED_AT).is_err());
    assert!(!path.with_extension("json.tmp").exists());
}

#[test]
fn the_file_is_named_after_the_portal_and_nothing_else() {
    assert_eq!(portal_key("192.168.1.5", 8080), "192.168.1.5_8080");
    let odd = portal_key("evil/../host:x\\y", 80);
    assert!(
        !odd.contains('/') && !odd.contains('\\') && !odd.contains(':'),
        "{odd}"
    );
    let creds = XtreamCreds {
        username: "user".into(),
        password: "secret".into(),
        host: "pandora.example".into(),
        port: 8080,
    };
    let path = path_for(Path::new("C:/cfg/cache"), Some(&creds)).unwrap();
    assert_eq!(
        path,
        Path::new("C:/cfg/cache").join("vod-pandora.example_8080.json")
    );
    assert!(
        path_for(Path::new("C:/cfg/cache"), None).is_none(),
        "an M3U list keeps none"
    );
}

#[test]
fn a_file_with_stray_genre_bits_counts_the_24_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    std::fs::write(&path, br#"{"version":1,"saved_at":0,"categories":[],"titles":[[true,1,"X","",0,0,4294967295,0,0,false,false,false,[[1,0,""]]]]}"#).unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let counts = index.search(&Query::default(), false).counts;
    assert_eq!(counts.genres.iter().sum::<usize>(), 24);
}

#[test]
fn a_category_the_file_does_not_list_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    // One category: title 1 is in it, titles 2 and 3 name a place past it.
    std::fs::write(
        &path,
        br#"{"version":1,"saved_at":0,"categories":[[true,12,"NL | Komedie"]],"titles":[
            [true,1,"A","",0,0,0,1,0,false,false,false,[[1,0,""]]],
            [true,2,"B","",0,0,0,7,0,false,false,false,[[2,0,""]]],
            [true,3,"C","",0,0,0,7,0,false,false,false,[[3,0,""]]]]}"#,
    )
    .unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let category = |id: i64| {
        let t = index.title(index.find(film(id)).unwrap()).unwrap();
        t.category.map(|c| c.id)
    };
    assert_eq!(
        (category(1), category(2), category(3)),
        (Some(12), None, None)
    );
    let none = HashSet::new();
    let b = index.find(film(2)).unwrap();
    assert!(
        index.more_like(b, 6, false, &none, &none).is_empty(),
        "no category to share"
    );
}

#[test]
fn odd_labels_and_more_containers_than_codes_are_read_safely() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod.json");
    // Labels with every bit set, and more containers than the 255 codes.
    let versions: Vec<String> = (0..300)
        .map(|i| format!(r#"[{},65535,"c{i}"]"#, 100 + i))
        .collect();
    let body = format!(
        r#"{{"version":1,"saved_at":0,"categories":[],"titles":[[true,100,"X","",0,0,0,0,0,false,false,false,[{}]]]}}"#,
        versions.join(",")
    );
    std::fs::write(&path, body).unwrap();
    let (index, _) = load(&path).unwrap().unwrap();
    let x = index.title(0).unwrap();
    assert_eq!(x.versions.len(), 300);
    let first = x.versions.get(0).unwrap();
    assert_eq!((first.ext, first.labels.rank()), ("c0", 4));
    assert!(!first.labels.text().is_empty());
    assert_eq!(x.versions.get(254).unwrap().ext, "c254", "the last code");
    assert!(
        x.versions.iter().skip(255).all(|v| v.ext.is_empty()),
        "past the codes: no container"
    );
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_vod_cache`
Expected: compile errors: there is no module `vod::cache`.

- [ ] **Step 3: The file**

In `rust/src/vod/mod.rs`, declare the cache: before the line `pub mod genres;`, add:

```rust
pub mod cache;
```

In `rust/src/vod/mod.rs`, at the end of the file, add the atomic writer of the thuisbioscoop's files:

```rust
/// Writes a file of the thuisbioscoop through a temporary one beside it
/// (`<name>.json.tmp`) that is renamed over it, so a write that fails leaves
/// the old file whole and no temporary file behind (spec 2026-10-15 §6).
/// `write` fills the temporary file, buffered.
pub(crate) fn write_atomic_with(
    path: &std::path::Path,
    write: impl FnOnce(&mut std::io::BufWriter<std::fs::File>) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::File::create(&tmp)
        .and_then(|file| {
            let mut out = std::io::BufWriter::new(file);
            write(&mut out)?;
            out.flush()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}
```

Create `rust/src/vod/cache.rs`:

```rust
//! The title index in a file (spec 2026-10-15 §4.6), so the next start has
//! the titles before the fresh lists come: `config\cache\vod-<host>_<port>.json`.
//! No adult title goes in (as the EPG cache keeps no adult channel), and no
//! credential: a cover address that `redact` would change holds an account,
//! and one with a query may hold a token that `redact` does not know; both are
//! left out. The catalog writes the file after every build of its titles and
//! reads it once, after the first frame, both on the blocking pool. A file that
//! cannot be read is ignored: the next load writes a new one. What a file says
//! is checked as it is read: a genre bit that no genre has, or a category it
//! does not list, names nothing.

use super::genres::{Genre, Labels};
use super::index::{category_rec, CategoryIn, Entry, Title, TitleIndex, VersionIn};
use super::{write_atomic_with, Kind};
use crate::args::XtreamCreds;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;

/// The file as it is read.
#[derive(Deserialize)]
struct CacheFile {
    version: u32,
    /// When it was written: unix seconds.
    saved_at: i64,
    /// `[film, id, name]`: films' categories and series' ones are two id spaces.
    categories: Vec<(bool, i64, String)>,
    titles: Vec<CachedTitle>,
}

/// A title as written: `[film, id, name, cover, year, rating, genres,
/// category, added, dutch, english, trailer, [[stream id, labels, container]]]`.
/// `category` is a place in `categories` plus one; 0 for none.
#[derive(Deserialize)]
struct CachedTitle(
    bool,
    u32,
    String,
    String,
    u16,
    u8,
    u32,
    u16,
    u16,
    bool,
    bool,
    bool,
    Vec<(u32, u16, String)>,
);

/// The file as it is written: borrowed from the index, so no copy of its text
/// is made on the way (`save`).
#[derive(Serialize)]
struct CacheFileRef<'a> {
    version: u32,
    saved_at: i64,
    categories: Vec<(bool, i64, &'a str)>,
    titles: TitlesRef<'a>,
}

/// A title as written ([`CachedTitle`]'s array), borrowed from the index.
#[derive(Serialize)]
struct CachedTitleRef<'a>(
    bool,
    u32,
    &'a str,
    &'a str,
    u16,
    u8,
    u32,
    u16,
    u16,
    bool,
    bool,
    bool,
    Vec<(u32, u16, &'a str)>,
);

/// The titles of an index that go into the file, written one by one: no list
/// of them is made.
struct TitlesRef<'a>(&'a TitleIndex);

impl Serialize for TitlesRef<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(kept(self.0).map(|t| cached(self.0, t)))
    }
}

/// The titles of `index` that go into the file: no adult one.
fn kept(index: &TitleIndex) -> impl Iterator<Item = Title<'_>> {
    (0..index.len())
        .filter_map(|pos| index.title(pos))
        .filter(|t| !t.adult)
}

/// Title `t` of `index` as written. A cover address that holds an account, or
/// a query (`redact` sees no token there), is left out: its tile shows the
/// placeholder until the fresh lists come.
fn cached<'a>(index: &TitleIndex, t: Title<'a>) -> CachedTitleRef<'a> {
    let cover = t
        .cover
        .filter(|c| !c.contains('?') && crate::redact::redact(c) == *c)
        .unwrap_or_default();
    CachedTitleRef(
        t.id.kind == Kind::Film,
        t.id.id as u32,
        t.name,
        cover,
        t.year,
        t.rating,
        t.genres,
        index.category_code(t.pos),
        t.added_day,
        t.dutch,
        t.english,
        t.trailer,
        t.versions
            .iter()
            .map(|v| (v.stream_id as u32, v.labels.0, v.ext))
            .collect(),
    )
}

/// The portal at `host`:`port` as a file or folder name: `<host>_<port>`,
/// every character but letters, digits, '.' and '-' turned into '_' (as the
/// EPG cache names its file). The user name and password never go into it.
pub fn portal_key(host: &str, port: u16) -> String {
    let host: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{host}_{port}")
}

/// The index cache of the portal with these credentials, in `cache_dir`; none
/// without a portal (an M3U list keeps no titles).
pub fn path_for(cache_dir: &Path, creds: Option<&XtreamCreds>) -> Option<PathBuf> {
    let c = creds?;
    Some(cache_dir.join(format!("vod-{}.json", portal_key(&c.host, c.port))))
}

/// Writes the titles of `index` to `path` at `saved_at` (unix seconds), as
/// they are serialized, through a temporary file renamed over it
/// (`write_atomic_with`), so a write that fails leaves the old file whole.
/// Adult titles stay out, and so does a cover address that holds an account
/// or a query. How many titles went in and the file's size.
pub fn save(path: &Path, index: &TitleIndex, saved_at: i64) -> std::io::Result<(usize, u64)> {
    let file = CacheFileRef {
        version: VERSION,
        saved_at,
        categories: index
            .categories()
            .map(|c| (c.kind == Kind::Film, c.id, c.name))
            .collect(),
        titles: TitlesRef(index),
    };
    write_atomic_with(path, |out| {
        serde_json::to_writer(out, &file).map_err(std::io::Error::other)
    })?;
    let bytes = std::fs::metadata(path)?.len();
    Ok((kept(index).count(), bytes))
}

/// The titles in the file at `path` and when it was written; none when there
/// is no file. A file that is no index cache of this version is an error (the
/// caller logs it and goes on without).
pub fn load(path: &Path) -> Result<Option<(TitleIndex, i64)>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let file: CacheFile = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if file.version != VERSION {
        return Err(format!("version {}", file.version));
    }
    let categories = file
        .categories
        .iter()
        .map(|(film, id, name)| {
            category_rec(&CategoryIn {
                kind: if *film { Kind::Film } else { Kind::Series },
                id: *id,
                name,
            })
        })
        .collect::<Vec<_>>();
    let known = categories.len();
    let mut pool: Vec<VersionIn<'_>> = Vec::new();
    let mut entries = Vec::with_capacity(file.titles.len());
    for t in &file.titles {
        let first = pool.len() as u32;
        pool.extend(
            t.12.iter()
                .map(|(sid, labels, ext)| (*sid, Labels(*labels), ext.as_str())),
        );
        entries.push(Entry {
            kind: if t.0 { Kind::Film } else { Kind::Series },
            id: t.1,
            name: &t.2,
            key: None,
            cover: &t.3,
            year: t.4,
            rating: t.5.min(100),
            genres: t.6 & ((1 << Genre::ALL.len()) - 1), // stray bits of a file name no genre
            // A place past the file's categories names none.
            category: if usize::from(t.7) <= known { t.7 } else { 0 },
            added: t.8,
            dutch: t.9,
            english: t.10,
            adult: false,
            trailer: t.11,
            versions: first..pool.len() as u32,
        });
    }
    Ok(Some((
        TitleIndex::assemble(entries, &pool, categories),
        file.saved_at,
    )))
}
```

In `rust/src/vod/index.rs`, in `impl TitleIndex`, after `cover_bytes`, add the accessor the cache needs. After

```rust
    /// The bytes of the cover addresses.
    pub fn cover_bytes(&self) -> usize {
        self.covers.heap_bytes()
    }
```

add:

```rust

    /// Title `pos`'s category as its record keeps it: a place in the
    /// categories plus one, 0 for none (the cache writes it so).
    pub(crate) fn category_code(&self, pos: usize) -> u16 {
        self.records[pos].category
    }
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `ct --test test_vod_cache`
Expected: all pass (9).

- [ ] **Step 5: Format, lint, full suite**

- Run `fmt src/vod/mod.rs tests/test_vod_cache.rs`. Expected: no output.
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 6: Commit**

Message file (in the scratchpad):
```
feat(rust): the index cache file: the titles of a portal as they are serialized, never an adult title, an account or a query, and nothing it says trusted

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/vod/cache.rs rust/src/vod/mod.rs rust/src/vod/index.rs rust/tests/test_vod_cache.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/vod/cache.rs rust/src/vod/mod.rs rust/src/vod/index.rs rust/tests/test_vod_cache.rs`

---

### Task 6b: The index cache: kept by the catalog, read after the first frame

The catalog keeps its titles in the file (spec §4.6, Rulings 32 and 33): written after every build of its titles, on the blocking pool, one write at a time; read once, after the app's first frame, on the blocking pool. The file's titles are put only while the store has none, and the catalog's own take their place when they are built; titles that came first keep theirs. A file that cannot be read is ignored, and the next load writes a new one. The store learns its file's path from `main` and `install_portal`.

**Files:**
- Modify: `rust/src/catalog.rs`, `rust/src/main.rs`
- Modify: `rust/src/app.rs` (`TvApp.first_frame_drawn`, `TvApp::start`, `TvApp::update_ui`, `TvApp::install_portal`; a test)
- Test: `rust/tests/test_vod_cache.rs` (5 more), `rust/src/app.rs` (`mod tests`), `rust/src/catalog.rs` (`mod zap_tests`)

**Interfaces:**
- Consumes: Task 5's `Titles`, `CatalogStore.{titles, titles_revision, categories_wait}`, `with_categories_wait`, `put_titles`, `spawn_fetch`'s `Ok(built)` arm; Task 6a's `cache::{path_for, save, load}`; `crate::args::XtreamCreds`; the tests' `Harness`, `loaded_catalog`, `idle_portal`, `wait_until`.
- Produces:
  ```rust
  // catalog.rs
  impl CatalogStore { pub fn with_vod_cache(self, path: Option<PathBuf>) -> Self; pub fn read_vod_cache(&self); }
  impl Titles { pub fn saved_at(&self) -> Option<i64>; }   // "Gegevens van <dag> <tijd>" in phase 3
  // private: type StoredTitles = (Arc<TitleIndex>, Option<i64>), the slot `titles` holds;
  //          fn put_titles(..) -> bool (whether it put them); fn write_vod_cache(PathBuf, Arc<TitleIndex>, Arc<Mutex<()>>)
  // app.rs
  TvApp.first_frame_drawn: bool
  ```
  Log lines: info `vod: cache read (<n> titles, written at <unix>) in <ms> ms`, `vod: cache written (<n> titles, <k> KB)`; warn `vod: cache unreadable (<e>) - ignored, the next load writes a new one`, `vod: cache not written (<e>)`.

- [ ] **Step 1: Write the failing tests**

In `rust/tests/test_vod_cache.rs`, its doc and imports: the store's side too. Replace

```rust
//! The title index in a file (vod/cache.rs, spec 2026-10-15 §4.6): what goes
//! in and what never does, a file that is missing or broken, and what a file
//! says that nothing may trust.

use bongo_player::args::XtreamCreds;
use bongo_player::portal::{Categories, Category, Movie, Series};
use bongo_player::vod::cache::{load, path_for, portal_key, save};
use bongo_player::vod::genres::{Genre, Labels};
use bongo_player::vod::index::{Query, TitleIndex};
use bongo_player::vod::{Kind, TitleId};
use std::collections::HashSet;
use std::path::Path;

```

with

```rust
//! The title index in a file (vod/cache.rs and the catalog's side of it, spec
//! 2026-10-15 §4.6): what goes in and what never does, a file that is missing
//! or broken, what a file says that nothing may trust, and the titles a store
//! shows from it until its catalog's own are built.

use bongo_player::args::XtreamCreds;
use bongo_player::catalog::{CatalogStatus, CatalogStore};
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::{Categories, Category, Movie, Portal, Series};
use bongo_player::vod::cache::{load, path_for, portal_key, save};
use bongo_player::vod::genres::{Genre, Labels};
use bongo_player::vod::index::{Query, Sort, TitleIndex};
use bongo_player::vod::{Kind, TitleId};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::query_param;
use wiremock::{Mock, MockServer, ResponseTemplate};

```

In `rust/tests/test_vod_cache.rs`, at the end of the file, add the tests of the store's side:

```rust
/// Waits (2 s at most) until `done`.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..400 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

/// A portal on a mock whose film list holds one film, `name`, beside two live
/// channels.
async fn portal_with_film(name: &str) -> (MockServer, Arc<dyn Portal>) {
    let server = MockServer::start().await;
    let films = format!(r#"[{{"stream_id":7,"name":"{name}"}}]"#);
    Mock::given(query_param("action", "get_vod_streams"))
        .respond_with(ResponseTemplate::new(200).set_body_string(films))
        .mount(&server)
        .await;
    let live = r#"[{"stream_id":1,"name":"NL | Een"},{"stream_id":2,"name":"NL | Twee"}]"#;
    Mock::given(query_param("action", "get_live_streams"))
        .respond_with(ResponseTemplate::new(200).set_body_string(live))
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
        .mount(&server)
        .await;
    let uri = url::Url::parse(&server.uri()).unwrap();
    let portal = XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: uri.host_str().unwrap().into(),
        port: uri.port().unwrap(),
    });
    (server, Arc::new(portal))
}

/// The names a store's titles show, A to Z, and when their file was written.
fn shown(store: &CatalogStore) -> Option<(Vec<String>, Option<i64>)> {
    let titles = store.titles()?;
    let q = Query {
        sort: Sort::Title,
        ..Query::default()
    };
    let names = titles
        .search(&q)
        .titles
        .iter()
        .map(|&p| titles.title(p as usize).unwrap().name.to_owned())
        .collect();
    Some((names, titles.saved_at()))
}

fn cache_with_library(dir: &Path) -> PathBuf {
    let path = dir.join("vod-h_80.json");
    save(&path, &library(), SAVED_AT).unwrap();
    path
}

#[tokio::test]
async fn a_store_shows_the_files_titles_until_its_catalog_has_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let path = cache_with_library(dir.path());
    let (_server, portal) = portal_with_film("Fresh (2026)").await;
    let store = Arc::new(CatalogStore::new(portal).with_vod_cache(Some(path.clone())));
    assert!(
        store.titles().is_none(),
        "nothing is read before it is asked"
    );
    store.read_vod_cache();
    until("the file's titles", || store.titles().is_some()).await;
    assert_eq!(
        shown(&store),
        Some((
            vec!["Dark".into(), "Secret".into(), "The Matrix".into()],
            Some(SAVED_AT)
        ))
    );
    store.spawn_fetch();
    until("the catalog's own titles", || {
        shown(&store).is_some_and(|(_, saved_at)| saved_at.is_none())
    })
    .await;
    assert_eq!(
        shown(&store),
        Some((vec!["Fresh".into()], None)),
        "the catalog's own"
    );
    // The fresh titles write the file anew.
    until("the new file", || {
        load(&path)
            .ok()
            .flatten()
            .is_some_and(|(index, _)| index.find(film(7)).is_some())
    })
    .await;
}

#[test]
fn the_cached_titles_show_while_the_fresh_ones_are_built() {
    // One blocking thread, which the test holds once the file is read: the
    // build of the fresh titles has to wait for it, the catalog must not.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_with_library(dir.path());
        let (_server, portal) = portal_with_film("Fresh (2026)").await;
        let store = Arc::new(CatalogStore::new(portal).with_vod_cache(Some(path)));
        store.read_vod_cache();
        until("the file's titles", || store.titles().is_some()).await;
        let (release, held) = std::sync::mpsc::channel::<()>();
        let holder = tokio::task::spawn_blocking(move || held.recv());
        store.spawn_fetch();
        until("the catalog", || store.status() == CatalogStatus::Loaded).await;
        // The lists are in: the catalog works, beside the file's titles.
        assert!(store.is_loaded());
        assert_eq!(store.zap_channels().len(), 2, "zapping works");
        assert_eq!(
            shown(&store).map(|(names, saved_at)| (names.len(), saved_at)),
            Some((3, Some(SAVED_AT))),
            "still the file's titles"
        );
        release.send(()).unwrap();
        holder.await.unwrap().unwrap();
        // ... until the fresh ones take their place.
        until("the catalog's own titles", || {
            shown(&store).is_some_and(|(_, saved_at)| saved_at.is_none())
        })
        .await;
        assert_eq!(shown(&store), Some((vec!["Fresh".into()], None)));
    });
}

#[tokio::test]
async fn the_file_is_read_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod-h_80.json");
    std::fs::write(&path, b"<html>not a cache</html>").unwrap();
    let store = CatalogStore::new(idle_portal()).with_vod_cache(Some(path.clone()));
    store.read_vod_cache();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(store.titles().is_none(), "nothing to show");
    // Another file in its place: a second read would show its one title.
    let other = TitleIndex::build(
        &[],
        &[Series {
            series_id: 1,
            name: "Other".into(),
            ..Series::default()
        }],
        &Categories::default(),
        2027,
    );
    save(&path, &other, SAVED_AT).unwrap();
    store.read_vod_cache();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(store.titles().is_none(), "not asked again");
}

#[tokio::test]
async fn an_unreadable_file_is_ignored_and_the_next_load_replaces_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vod-h_80.json");
    std::fs::write(&path, b"<html>not a cache</html>").unwrap();
    let (_server, portal) = portal_with_film("Fresh (2026)").await;
    let store = Arc::new(CatalogStore::new(portal).with_vod_cache(Some(path.clone())));
    store.read_vod_cache();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(store.titles().is_none());
    store.spawn_fetch();
    until("the new file", || {
        load(&path).is_ok_and(|read| read.is_some())
    })
    .await;
}

#[tokio::test]
async fn a_catalog_that_came_first_shows_its_own_titles() {
    let dir = tempfile::tempdir().unwrap();
    let path = cache_with_library(dir.path());
    let (_server, portal) = portal_with_film("Fresh (2026)").await;
    let store = Arc::new(CatalogStore::new(portal).with_vod_cache(Some(path)));
    store.spawn_fetch();
    until("the catalog's own titles", || store.titles().is_some()).await;
    store.read_vod_cache();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(shown(&store), Some((vec!["Fresh".into()], None)));
}

fn idle_portal() -> Arc<dyn Portal> {
    Arc::new(XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h".into(),
        port: 80,
    }))
}
```

In `rust/src/app.rs`, in `mod tests`, after `fn library_catalog` and before `fn f_pressed` (with its doc), add the test. Before

```rust
    /// The press of `f` as a keyboard makes it: the key and its letter, together.
    fn f_pressed() -> Vec<egui::Event> {
```

add:

```rust
    #[tokio::test]
    async fn the_titles_of_the_last_start_are_read_once_a_frame_is_drawn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vod-h_80.json");
        let kept = crate::portal::Movie {
            stream_id: 7,
            name: "Kept (2020)".into(),
            ..Default::default()
        };
        let categories = crate::portal::Categories::default();
        let index = crate::vod::index::TitleIndex::build(&[kept], &[], &categories, 2027);
        crate::vod::cache::save(&path, &index, 1_792_022_400).unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        // A store with the file, as `main` makes it, from its first frame on.
        let store = Arc::new(CatalogStore::new(idle_portal()).with_vod_cache(Some(path)));
        h.app.catalog = store.clone();
        h.app.first_frame_drawn = false;
        h.frame(&[]);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(store.titles().is_none(), "not read in the first frame");
        h.frame(&[]);
        wait_until("the titles of the last start", || store.titles().is_some()).await;
        assert_eq!(store.titles().unwrap().saved_at(), Some(1_792_022_400));
    }

```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_vod_cache`
Expected: compile errors: `CatalogStore` has no `with_vod_cache`, `read_vod_cache` or `Titles::saved_at`.

- [ ] **Step 3: The catalog's side**

In `rust/src/catalog.rs`, the imports: after the line `use std::collections::{HashMap, HashSet};`, add:

```rust
use std::path::PathBuf;
```

In `rust/src/catalog.rs`, the doc of `pub struct Titles`: add a paragraph after its first. Replace

```rust
/// filters. Asked again after the lock changes.
#[derive(Clone)]
pub struct Titles {
```

with

```rust
/// filters. Asked again after the lock changes.
///
/// Until the titles of the store's catalog are built, they are those of the
/// cache file (`saved_at`).
#[derive(Clone)]
pub struct Titles {
```

In `rust/src/catalog.rs`, in `pub struct Titles`, after `unlocked`. Replace

```rust
    index: Arc<TitleIndex>,
    unlocked: bool,
}
```

with

```rust
    index: Arc<TitleIndex>,
    unlocked: bool,
    /// When the cache file the titles come from was written, unix seconds;
    /// none for a catalog's own.
    saved_at: Option<i64>,
}
```

In `rust/src/catalog.rs`, at the end of `impl Titles`, after `categories`. Replace

```rust
    /// The portal's categories.
    pub fn categories(&self) -> impl Iterator<Item = CategoryInfo<'_>> + '_ {
        self.index.categories()
    }
}
```

with

```rust
    /// The portal's categories.
    pub fn categories(&self) -> impl Iterator<Item = CategoryInfo<'_>> + '_ {
        self.index.categories()
    }

    /// When the cache file these titles come from was written (unix seconds):
    /// Thuis says "Gegevens van <dag> <tijd>". None for a catalog's own.
    pub fn saved_at(&self) -> Option<i64> {
        self.saved_at
    }
}
```

In `rust/src/catalog.rs`, the type of the titles in the store and the file's writer: before the line `/// Puts a catalog in its slot and counts it.`, add:

```rust
/// Titles in the store, and when the cache file they come from was written
/// (unix seconds; none for a catalog's own).
type StoredTitles = (Arc<TitleIndex>, Option<i64>);

/// Writes `titles` to the cache file at `path`, on the blocking pool, one
/// write at a time (`writing`). A write that fails is logged; the file that
/// was there stays whole.
fn write_vod_cache(path: PathBuf, titles: Arc<TitleIndex>, writing: Arc<Mutex<()>>) {
    tokio::task::spawn_blocking(move || {
        let _turn = writing.lock();
        let now = chrono::Utc::now().timestamp();
        match crate::vod::cache::save(&path, &titles, now) {
            Ok((n, bytes)) => info!("vod: cache written ({} titles, {} KB)", n, bytes / 1024),
            Err(e) => warn!("vod: cache not written ({e})"),
        }
    });
}

```

In `rust/src/catalog.rs`, in `pub struct CatalogStore`, the titles may be the file's, and the file's fields. Replace

```rust
    /// The thuisbioscoop's titles (`vod::index`), adult ones included: they
    /// leave the store only as `Titles`, which apply the lock. Put after the
    /// catalog they come from (`put_titles`).
    titles: Arc<RwLock<Option<Arc<TitleIndex>>>>,
    /// How many times `titles` was put (see [`CatalogStore::titles_revision`]).
    titles_revision: Arc<AtomicU64>,
    /// How long the titles wait for the category lists once the catalog is in
    /// ([`CatalogStore::with_categories_wait`]).
    categories_wait: Duration,
}
```

with

```rust
    /// The thuisbioscoop's titles (`vod::index`), adult ones included: they
    /// leave the store only as `Titles`, which apply the lock. Put after the
    /// catalog they come from (`put_titles`); until then those of the cache
    /// file, once read (`read_vod_cache`).
    titles: Arc<RwLock<Option<StoredTitles>>>,
    /// How many times `titles` was put (see [`CatalogStore::titles_revision`]).
    titles_revision: Arc<AtomicU64>,
    /// How long the titles wait for the category lists once the catalog is in
    /// ([`CatalogStore::with_categories_wait`]).
    categories_wait: Duration,
    /// Where this portal's titles are kept between starts (`vod::cache`);
    /// none for an M3U list, and in tests that do not ask for one.
    vod_cache: Option<PathBuf>,
    /// The file was asked for: it is read once per store.
    vod_cache_asked: AtomicBool,
    /// One write of the file at a time: two through its one temporary file
    /// would mix.
    vod_cache_writing: Arc<Mutex<()>>,
}
```

In `rust/src/catalog.rs`, `put_titles` takes the place of the file's titles, and says whether it put them. Replace

```rust
/// counts them, as `put_catalog` does, when that catalog is still the store's
/// (`revision`): a newer one's own titles come after it.
fn put_titles(
    slot: &RwLock<Option<Arc<TitleIndex>>>,
    titles_revision: &AtomicU64,
    revision: &AtomicU64,
    of: u64,
    titles: Arc<TitleIndex>,
) {
    let mut held = slot.write();
    if revision.load(Ordering::Acquire) != of {
        return;
    }
    *held = Some(titles);
    titles_revision.fetch_add(1, Ordering::Release);
}
```

with

```rust
/// counts them, as `put_catalog` does, when that catalog is still the store's
/// (`revision`): a newer one's own titles come after it. They take the place
/// of the cache file's. Whether they were put.
fn put_titles(
    slot: &RwLock<Option<StoredTitles>>,
    titles_revision: &AtomicU64,
    revision: &AtomicU64,
    of: u64,
    titles: Arc<TitleIndex>,
) -> bool {
    let mut held = slot.write();
    if revision.load(Ordering::Acquire) != of {
        return false;
    }
    *held = Some((titles, None));
    titles_revision.fetch_add(1, Ordering::Release);
    true
}
```

In `rust/src/catalog.rs`, in `CatalogStore::new`, start the new fields. Replace

```rust
            categories_wait: CATEGORIES_WAIT,
        }
    }
```

with

```rust
            categories_wait: CATEGORIES_WAIT,
            vod_cache: None,
            vod_cache_asked: AtomicBool::new(false),
            vod_cache_writing: Arc::new(Mutex::new(())),
        }
    }
```

In `rust/src/catalog.rs`, after `CatalogStore::with_categories_wait`, add `with_vod_cache` and `read_vod_cache`. After

```rust
    /// How long this store's titles wait for the portal's category lists once
    /// its catalog is in: 15 s, a test less. Then they are built without them.
    pub fn with_categories_wait(mut self, wait: Duration) -> Self {
        self.categories_wait = wait;
        self
    }
```

add:

```rust

    /// This store keeps its titles in the file at `path` (`vod::cache`): it
    /// writes it after every build of its titles and reads it once, when asked
    /// ([`CatalogStore::read_vod_cache`]). None keeps no file.
    pub fn with_vod_cache(mut self, path: Option<PathBuf>) -> Self {
        self.vod_cache = path;
        self
    }

    /// Reads the titles of the last start from the file, once, on the blocking
    /// pool: the app asks after its first frame (spec 2026-10-15 §4.6). They are
    /// put only while the store has no titles, and the titles of its catalog
    /// take their place. A file that cannot be read is ignored; the next load
    /// writes a new one.
    pub fn read_vod_cache(&self) {
        // Asked every frame: once asked, an atomic load and nothing else.
        if self.vod_cache_asked.load(Ordering::Acquire) {
            return;
        }
        let Some(path) = self.vod_cache.clone() else {
            self.vod_cache_asked.store(true, Ordering::Release);
            return;
        };
        if self.vod_cache_asked.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (titles, titles_revision) = (self.titles.clone(), self.titles_revision.clone());
        runtime.spawn_blocking(move || {
            let t0 = Instant::now();
            match crate::vod::cache::load(&path) {
                Ok(Some((index, saved_at))) => {
                    info!(
                        "vod: cache read ({} titles, written at {}) in {} ms",
                        index.len(),
                        saved_at,
                        t0.elapsed().as_millis()
                    );
                    // Titles that came first (the catalog's own) are fresher.
                    let mut held = titles.write();
                    if held.is_none() {
                        *held = Some((Arc::new(index), Some(saved_at)));
                        titles_revision.fetch_add(1, Ordering::Release);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("vod: cache unreadable ({e}) - ignored, the next load writes a new one")
                }
            }
        });
    }
```

In `rust/src/catalog.rs`, in `CatalogStore::spawn_fetch`, with the other clones. After

```rust
        let titles = self.titles.clone();
        let titles_revision = self.titles_revision.clone();
```

add:

```rust
        let vod_cache = self.vod_cache.clone();
        let vod_cache_writing = self.vod_cache_writing.clone();
```

In `rust/src/catalog.rs`, in `CatalogStore::spawn_fetch`, where the titles are put (Task 5's `Ok(built)` arm): the file keeps them. Replace

```rust
                        Ok(built) => {
                            put_titles(&titles, &titles_revision, &revision, of, Arc::new(built));
                        }
```

with

```rust
                        Ok(built) => {
                            let built = Arc::new(built);
                            let put =
                                put_titles(&titles, &titles_revision, &revision, of, built.clone());
                            // The file keeps the titles that show.
                            if let (true, Some(path)) = (put, vod_cache) {
                                write_vod_cache(path, built, vod_cache_writing);
                            }
                        }
```

In `rust/src/catalog.rs`, `CatalogStore::titles`: the file's titles until the catalog's own. Replace

```rust
    /// The thuisbioscoop's titles as the lock stands now ([`Titles`]): those of
    /// the store's catalog once they are built (`put_titles`); none before.
    pub fn titles(&self) -> Option<Titles> {
        let unlocked = self.adult_unlocked();
        let held = self.titles.read();
        held.as_ref().map(|index| Titles {
            index: index.clone(),
            unlocked,
        })
    }
```

with

```rust
    /// The thuisbioscoop's titles as the lock stands now ([`Titles`]): those of
    /// the store's catalog once they are built (`put_titles`), else those of the
    /// cache file once read ([`CatalogStore::read_vod_cache`]); none before.
    pub fn titles(&self) -> Option<Titles> {
        let unlocked = self.adult_unlocked();
        let held = self.titles.read();
        held.as_ref().map(|(index, saved_at)| Titles {
            index: index.clone(),
            unlocked,
            saved_at: *saved_at,
        })
    }
```

In `rust/src/catalog.rs`, at the end of `mod zap_tests`, a store whose own titles came first holds no second index. Replace

```rust
        // Its live channels are kept apart, in `live`.
        assert_eq!(stored.live.len(), 4);
    }
}
```

with

```rust
        // Its live channels are kept apart, in `live`.
        assert_eq!(stored.live.len(), 4);
    }

    #[tokio::test]
    async fn a_store_whose_titles_came_first_takes_none_from_the_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vod-h_80.json");
        let cached = TitleIndex::build(
            &[],
            &[Series {
                series_id: 1,
                name: "Cached".into(),
                ..Series::default()
            }],
            &Categories::default(),
            2027,
        );
        crate::vod::cache::save(&path, &cached, 1).unwrap();
        let portal = Arc::new(crate::portal::xtream::XtreamPortal::new(
            crate::args::XtreamCreds {
                username: "u".into(),
                password: "p".into(),
                host: "h".into(),
                port: 80,
            },
        ));
        let store =
            CatalogStore::from_catalog(portal, PortalCatalog::default()).with_vod_cache(Some(path));
        store.read_vod_cache();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let held = store.titles.read();
        let (index, saved_at) = held.as_ref().expect("the catalog's own titles");
        assert!(
            index.is_empty() && saved_at.is_none(),
            "no second index held"
        );
        assert_eq!(store.titles_revision(), 1, "the file put nothing");
    }
}
```

- [ ] **Step 4: The app's side**

In `rust/src/main.rs`, in `main`, where the catalog store is made. Replace

```rust
    let catalog = Arc::new(CatalogStore::new(portal));
```

with

```rust
    // The thuisbioscoop's titles of the last start: read after the first frame.
    let vod_cache = bongo_player::vod::cache::path_for(&storage.cache_dir(), creds.as_ref());
    let catalog = Arc::new(CatalogStore::new(portal).with_vod_cache(vod_cache));
```

In `rust/src/app.rs`, in `pub struct TvApp`: after the line `epg_cache_retry_after: Option<Instant>,`, add:

```rust
    /// A frame has been drawn: from the next one on the catalog may read the
    /// thuisbioscoop's titles of the last start (`CatalogStore::read_vod_cache`).
    first_frame_drawn: bool,
```

In `rust/src/app.rs`, in `TvApp::start`'s struct literal: after the line `epg_cache_retry_after: None,`, add:

```rust
            first_frame_drawn: false,
```

In `rust/src/app.rs`, in `TvApp::update_ui`: after the line `self.logos.drain();`, add:

```rust
        // The titles of the last start, once a frame is drawn (spec 2026-10-15
        // §4.6); the store reads them once, so this is an atomic load a frame.
        if self.first_frame_drawn {
            self.catalog.read_vod_cache();
        }
        self.first_frame_drawn = true;
```

In `rust/src/app.rs`, in `TvApp::install_portal`, where the new catalog store is made. Replace

```rust
        self.catalog = Arc::new(CatalogStore::new(portal));
```

with

```rust
        let vod_cache = crate::vod::cache::path_for(&self.storage.cache_dir(), self.creds.as_ref());
        self.catalog = Arc::new(CatalogStore::new(portal).with_vod_cache(vod_cache));
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `ct --test test_vod_cache --test test_vod_catalog`, then `ct --lib app::tests::the_titles_of_the_last_start`, then `ct --lib catalog::zap_tests`
Expected: all pass (14 in `test_vod_cache`, the app's test, and `a_store_whose_titles_came_first_takes_none_from_the_cache_file`).

- [ ] **Step 6: Format, lint, full suite**

- Run `fmt src/main.rs tests/test_vod_cache.rs`. Expected: no output.
- Run `hunks src/catalog.rs`. Expected: the count Task 1 Step 1 recorded (`3` at c81d7b0).
- Run `hunks src/app.rs`. Expected: the count Task 1 Step 1 recorded (`7` at c81d7b0).
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 7: Commit**

Message file (in the scratchpad):
```
feat(rust): the catalog keeps its titles in the cache file: written after every build, read once after the first frame and shown only until its own are built

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/catalog.rs rust/src/main.rs rust/src/app.rs rust/tests/test_vod_cache.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/catalog.rs rust/src/main.rs rust/src/app.rs rust/tests/test_vod_cache.rs`

---

### Task 7: What was watched

`Watching` (spec §4.5, Rulings 34–36): where each film version and episode stopped and whether it was seen, Mijn lijst and the thumbs, in exactly the spec's file format, one file per source beside its favorites. The 60 s and 90 % rules, writes through `write_atomic_with` (Task 6a) at most once every 5 s and always at a stop, and an unreadable file kept as `.bad`. Deleting a source removes its file. Nothing creates a `Watching` before phase 2.

**Files:**
- Create: `rust/src/vod/watching.rs`
- Modify: `rust/src/vod/mod.rs` (`pub mod watching;`), `rust/src/storage.rs` (`watching_path`, `remove_source_files`)
- Test: `rust/tests/test_vod_watching.rs` (new), `rust/tests/test_storage.rs`

**Interfaces:**
- Consumes: `Kind`, `TitleId` (Task 2a); `write_atomic_with` (Task 6a); `Storage::source_file` (private, in storage.rs).
- Produces:
  ```rust
  // vod/watching.rs
  pub const STARTED_SECS: f64;    // 60
  pub const SEEN_SHARE: f64;      // 0.9
  pub const WRITE_EVERY: Duration; // 5 s
  pub enum Item { Film(i64), Episode(i64) }                         // "m:<id>", "e:<id>"
  pub struct EpisodeOf { pub series: i64, pub season: u32, pub number: u32 }
  pub struct Progress { pub pos: u32, pub dur: u32, pub at: i64, pub done: bool, pub episode: Option<EpisodeOf> }
  pub enum Thumb { Up, Down }
  pub struct Watching;
  impl Watching {
      pub fn load(path: PathBuf) -> Self;
      pub fn progress(&self, item: Item) -> Option<&Progress>;
      pub fn latest(&self, items: impl IntoIterator<Item = Item>) -> Option<(Item, &Progress)>;
      pub fn played(&mut self, item: Item, pos: f64, dur: f64, episode: Option<EpisodeOf>, now_unix: i64);
      pub fn finished(&mut self, item: Item, dur: f64, episode: Option<EpisodeOf>, now_unix: i64);
      pub fn in_list(&self, t: TitleId) -> bool;
      pub fn toggle_list(&mut self, t: TitleId, now_unix: i64) -> bool;
      pub fn list(&self) -> Vec<TitleId>;                              // the latest added first
      pub fn thumb(&self, t: TitleId) -> Option<Thumb>;
      pub fn press_thumb(&mut self, t: TitleId, thumb: Thumb) -> Option<Thumb>;
      pub fn is_dirty(&self) -> bool;
      pub fn save_if_due(&mut self, now: Instant);
      pub fn flush(&mut self);
  }
  // storage.rs
  impl Storage { pub fn watching_path(&self) -> PathBuf; }   // remove_source_files removes it and its .bad
  ```
  Log lines (warn): `vod: <file> unreadable (<why>) - kept as .bad - starting empty` (or `- left as it is -`), `vod: <file> unreadable (<e>) - starting empty`, `vod: <file> not written (<e>)`; the file's name only, never its folder.

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_vod_watching.rs`:

```rust
//! What was watched (vod/watching.rs, spec 2026-10-15 §4.5): the file's
//! format, the 60 s and 90 % rules, Mijn lijst and the thumbs, the writes 5 s
//! apart and at a stop, and a file that cannot be read.

use bongo_player::vod::watching::{EpisodeOf, Item, Progress, Thumb, Watching, WRITE_EVERY};
use bongo_player::vod::{Kind, TitleId};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const NOW: i64 = 1_792_089_306;

fn fresh() -> (tempfile::TempDir, PathBuf, Watching) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    let watching = Watching::load(path.clone());
    (dir, path, watching)
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn series(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Series,
        id,
    }
}

fn film(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Film,
        id,
    }
}

#[test]
fn the_file_is_in_the_specs_format() {
    let (_dir, path, mut w) = fresh();
    w.played(Item::Film(12345), 4380.4, 9960.0, None, NOW);
    let episode = EpisodeOf {
        series: 4321,
        season: 2,
        number: 5,
    };
    w.played(
        Item::Episode(987654),
        1415.0,
        2820.0,
        Some(episode),
        NOW + 94,
    );
    w.toggle_list(series(4321), NOW - 306);
    w.press_thumb(series(4321), Thumb::Up);
    w.press_thumb(film(555), Thumb::Down);
    w.flush();
    let expected = serde_json::json!({
        "version": 1,
        "items": {
            "m:12345": { "pos": 4380, "dur": 9960, "at": NOW, "done": false },
            "e:987654": { "pos": 1415, "dur": 2820, "at": NOW + 94, "done": false, "series": 4321, "s": 2, "n": 5 }
        },
        "list": [ { "k": "s", "id": 4321, "at": NOW - 306 } ],
        "thumbs": { "s:4321": 1, "m:555": -1 }
    });
    assert_eq!(json(&path), expected);
    let again = Watching::load(path);
    assert_eq!(
        again.progress(Item::Episode(987654)),
        Some(&Progress {
            pos: 1415,
            dur: 2820,
            at: NOW + 94,
            done: false,
            episode: Some(episode)
        })
    );
    assert!(again.in_list(series(4321)));
    assert_eq!(again.thumb(film(555)), Some(Thumb::Down));
}

#[test]
fn under_a_minute_is_not_started() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 59.9, 6000.0, None, NOW);
    assert_eq!(w.progress(Item::Film(1)), None);
    assert!(!w.is_dirty());
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    assert!(w.progress(Item::Film(1)).is_some());
    // Back to the start: not started again.
    w.played(Item::Film(1), 12.0, 6000.0, None, NOW + 1);
    assert_eq!(w.progress(Item::Film(1)), None);
}

#[test]
fn from_90_percent_or_at_the_end_it_is_seen() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 5399.0, 6000.0, None, NOW);
    assert!(!w.progress(Item::Film(1)).unwrap().done);
    w.played(Item::Film(1), 5400.0, 6000.0, None, NOW);
    assert!(w.progress(Item::Film(1)).unwrap().done, "90 %");
    w.played(Item::Film(2), 300.0, 0.0, None, NOW);
    assert!(
        !w.progress(Item::Film(2)).unwrap().done,
        "no length: not by share"
    );
    w.finished(Item::Film(2), 0.0, None, NOW);
    let end = w.progress(Item::Film(2)).unwrap();
    assert!(end.done, "the end of the file");
    assert_eq!(end.pos, 300);
}

#[test]
fn a_seen_item_stays_seen_under_a_minute_and_a_rewatch_is_in_progress_again() {
    let (_dir, _path, mut w) = fresh();
    w.finished(Item::Episode(7), 2820.0, None, NOW);
    w.played(Item::Episode(7), 30.0, 2820.0, None, NOW + 10);
    assert!(
        w.progress(Item::Episode(7)).unwrap().done,
        "a peek at the start"
    );
    w.played(Item::Episode(7), 600.0, 2820.0, None, NOW + 600);
    let p = w.progress(Item::Episode(7)).unwrap();
    assert!(!p.done && p.pos == 600, "watched again: {p:?}");
}

#[test]
fn the_same_place_again_is_no_change() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 600.2, 6000.0, None, NOW);
    w.flush();
    assert!(!w.is_dirty());
    w.played(Item::Film(1), 600.9, 6000.0, None, NOW + 5);
    assert!(!w.is_dirty(), "paused: the same second");
    w.played(Item::Film(1), 601.0, 6000.0, None, NOW + 6);
    assert!(w.is_dirty());
}

#[test]
fn writes_are_five_seconds_apart_and_a_stop_writes_at_once() {
    let (_dir, path, mut w) = fresh();
    let t0 = Instant::now();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.save_if_due(t0);
    assert_eq!(
        json(&path)["items"]["m:1"]["pos"],
        600,
        "the first write goes at once"
    );
    w.played(Item::Film(1), 601.0, 6000.0, None, NOW + 1);
    w.save_if_due(t0 + Duration::from_secs(1));
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 600, "not yet");
    w.save_if_due(t0 + WRITE_EVERY);
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 601);
    w.played(Item::Film(1), 602.0, 6000.0, None, NOW + 2);
    w.flush();
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 602, "a stop");
    assert!(!path.with_extension("json.tmp").exists());
}

#[test]
fn mijn_lijst_has_the_latest_first_and_a_second_press_takes_a_title_off() {
    let (_dir, _path, mut w) = fresh();
    assert!(w.toggle_list(series(1), NOW));
    assert!(w.toggle_list(film(2), NOW + 10));
    assert!(w.toggle_list(series(3), NOW + 5));
    assert_eq!(w.list(), [film(2), series(3), series(1)]);
    assert!(!w.toggle_list(film(2), NOW + 20));
    assert!(!w.in_list(film(2)));
    assert_eq!(w.list(), [series(3), series(1)]);
}

#[test]
fn thumbs_up_and_down_exclude_each_other_and_a_second_press_withdraws() {
    let (_dir, _path, mut w) = fresh();
    assert_eq!(w.press_thumb(film(1), Thumb::Up), Some(Thumb::Up));
    assert_eq!(
        w.press_thumb(film(1), Thumb::Down),
        Some(Thumb::Down),
        "👎 takes 👍's place"
    );
    assert_eq!(w.thumb(film(1)), Some(Thumb::Down));
    assert_eq!(
        w.press_thumb(film(1), Thumb::Down),
        None,
        "pressed again: withdrawn"
    );
    assert_eq!(w.thumb(film(1)), None);
}

#[test]
fn a_film_title_shows_the_latest_progress_of_its_versions() {
    let (_dir, _path, mut w) = fresh();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.played(Item::Film(2), 1200.0, 6000.0, None, NOW + 60);
    let (item, p) = w
        .latest([Item::Film(1), Item::Film(2), Item::Film(3)])
        .unwrap();
    assert_eq!((item, p.pos), (Item::Film(2), 1200));
    assert!(w.latest([Item::Film(3)]).is_none());
}

#[test]
fn a_missing_file_is_an_empty_store() {
    let (_dir, path, w) = fresh();
    assert!(!path.exists());
    assert!(w.list().is_empty() && !w.is_dirty());
}

#[test]
fn an_unreadable_file_is_kept_as_bad_and_the_store_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    for body in [
        &b"{\"version\":1,\"items\":"[..],
        br#"{"version":2,"items":{}}"#,
    ] {
        std::fs::write(&path, body).unwrap();
        let mut w = Watching::load(path.clone());
        assert!(w.progress(Item::Film(1)).is_none());
        assert_eq!(
            std::fs::read(path.with_extension("json.bad")).unwrap(),
            body,
            "kept"
        );
        assert!(!path.exists(), "moved aside");
        w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
        w.flush();
        assert_eq!(json(&path)["version"], 1, "the next write starts afresh");
    }
}

#[test]
fn entries_the_store_cannot_read_are_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    let body = r#"{"version":1,
        "items":{"m:1":{"pos":600,"dur":6000,"at":1,"done":false},"x:2":{"pos":1,"dur":1,"at":1,"done":false}},
        "list":[{"k":"q","id":3,"at":1},{"k":"m","id":4,"at":1}],
        "thumbs":{"s:5":1,"s:6":7,"bad":1}}"#;
    std::fs::write(&path, body).unwrap();
    let w = Watching::load(path);
    assert!(w.progress(Item::Film(1)).is_some());
    assert_eq!(w.list(), [film(4)]);
    assert_eq!(w.thumb(series(5)), Some(Thumb::Up));
    assert_eq!(w.thumb(series(6)), None);
}

#[test]
fn a_write_that_fails_keeps_the_old_file_and_tries_again_later() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watching.json");
    std::fs::create_dir(&path).unwrap(); // a folder where the file goes
    let mut w = Watching::load(path.clone());
    let t0 = Instant::now();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.save_if_due(t0);
    assert!(w.is_dirty(), "not written");
    assert!(!path.with_extension("json.tmp").exists());
    std::fs::remove_dir(&path).unwrap();
    w.save_if_due(t0 + WRITE_EVERY);
    assert!(!w.is_dirty());
    assert_eq!(json(&path)["items"]["m:1"]["pos"], 600);
}

#[test]
fn the_file_holds_numbers_and_times_only() {
    let (_dir, path, mut w) = fresh();
    w.played(Item::Film(1), 600.0, 6000.0, None, NOW);
    w.toggle_list(film(1), NOW);
    w.flush();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("http") && !text.contains("name"), "{text}");
}
```

In `rust/tests/test_storage.rs`, in `source_1_keeps_the_0_6_1_file_names_and_the_others_carry_their_id`, after the first `bad_streams_path` assertion. Replace

```rust
    assert_eq!(storage.bad_streams_path(), root.join("bad_streams.json"));
    storage.set_source(2);
```

with

```rust
    assert_eq!(storage.bad_streams_path(), root.join("bad_streams.json"));
    assert_eq!(storage.watching_path(), root.join("watching.json"));
    storage.set_source(2);
```

In `rust/tests/test_storage.rs`, in the same test: after the line `assert_eq!(storage.bad_streams_path(), root.join("bad_streams-2.json"));`, add:

```rust
    assert_eq!(storage.watching_path(), root.join("watching-2.json"));
```

In `rust/tests/test_storage.rs`, in `removing_a_sources_files_leaves_the_other_sources_alone`. Replace

```rust
            storage.bad_streams_path(),
            storage.m3u_cache_path(id),
        ] {
            std::fs::write(path, "x").unwrap();
        }
    }
    assert_eq!(storage.remove_source_files(2), 5, "its marks go too");
    assert!(!storage.favorites_path().exists(), "source 2's own");
    assert!(!storage.bad_streams_path().exists(), "source 2's own marks");
    storage.set_source(1);
    assert!(storage.favorites_path().exists() && storage.m3u_cache_path(1).exists());
    assert!(storage.bad_streams_path().exists(), "source 1's marks stay");
```

with

```rust
            storage.bad_streams_path(),
            storage.watching_path(),
            storage.watching_path().with_extension("json.bad"),
            storage.m3u_cache_path(id),
        ] {
            std::fs::write(path, "x").unwrap();
        }
    }
    assert_eq!(
        storage.remove_source_files(2),
        7,
        "its marks and what it watched go too"
    );
    assert!(!storage.favorites_path().exists(), "source 2's own");
    assert!(!storage.bad_streams_path().exists(), "source 2's own marks");
    assert!(!storage.watching_path().exists(), "what source 2 watched");
    storage.set_source(1);
    assert!(storage.favorites_path().exists() && storage.m3u_cache_path(1).exists());
    assert!(storage.bad_streams_path().exists(), "source 1's marks stay");
    assert!(
        storage.watching_path().exists(),
        "what source 1 watched stays"
    );
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_vod_watching --test test_storage`
Expected: compile errors: there is no module `vod::watching`, and `Storage` has no `watching_path`.

- [ ] **Step 3: The store and its path**

In `rust/src/vod/mod.rs`, declare the watching store: after the line `pub mod versions;`, add:

```rust
pub mod watching;
```

Create `rust/src/vod/watching.rs`:

```rust
//! What was watched (spec 2026-10-15 §4.5): where each film version and
//! episode stopped and whether it was seen, the user's list ("Mijn lijst")
//! and thumbs. One file per source, beside its favorites (`watching.json`,
//! `watching-<id>.json`): numbers and times only, no names, no addresses.
//!
//! A position under 60 s is "not started"; from 90 % of the length on, or at
//! the end of the file, it is seen. Writes go through a temporary file, at
//! most one every 5 s, and always at a stop (`flush`). A file that cannot be
//! read is kept as `watching.json.bad`; the store starts empty and says so
//! once in the log.

use super::{write_atomic_with, Kind, TitleId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Under this a position is "not started".
pub const STARTED_SECS: f64 = 60.0;
/// From this share of its length on, a film or an episode is seen.
pub const SEEN_SHARE: f64 = 0.9;
/// Writes are this far apart at least; a stop writes at once.
pub const WRITE_EVERY: Duration = Duration::from_secs(5);

const VERSION: u32 = 1;

/// What plays: a film's version by its stream id (`m:<id>`), or an episode by
/// its id (`e:<id>`). A film title's progress is that of its most recent
/// version ([`Watching::latest`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Item {
    Film(i64),
    Episode(i64),
}

impl Item {
    fn key(self) -> String {
        match self {
            Item::Film(id) => format!("m:{id}"),
            Item::Episode(id) => format!("e:{id}"),
        }
    }

    fn of_key(key: &str) -> Option<Item> {
        let (kind, id) = key.split_once(':')?;
        let id = id.parse().ok()?;
        match kind {
            "m" => Some(Item::Film(id)),
            "e" => Some(Item::Episode(id)),
            _ => None,
        }
    }
}

/// The episode an item is: for "Verder kijken" and the next episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodeOf {
    pub series: i64,
    pub season: u32,
    pub number: u32,
}

/// Where an item stopped, and whether it was seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Seconds.
    pub pos: u32,
    /// Its length in seconds; 0: unknown.
    pub dur: u32,
    /// When, unix seconds.
    pub at: i64,
    pub done: bool,
    pub episode: Option<EpisodeOf>,
}

/// A thumb: 👍 or 👎.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thumb {
    Up,
    Down,
}

#[derive(Serialize, Deserialize)]
struct WatchFile {
    version: u32,
    #[serde(default)]
    items: BTreeMap<String, ItemEntry>,
    #[serde(default)]
    list: Vec<ListEntry>,
    #[serde(default)]
    thumbs: BTreeMap<String, i8>,
}

#[derive(Serialize, Deserialize)]
struct ItemEntry {
    pos: u32,
    dur: u32,
    at: i64,
    done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    series: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    s: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
}

#[derive(Serialize, Deserialize)]
struct ListEntry {
    k: String,
    id: i64,
    at: i64,
}

/// A title as the list and the thumbs name it: `s:<id>` or `m:<id>`.
fn title_key(t: TitleId) -> String {
    format!("{}:{}", kind_letter(t.kind), t.id)
}

fn kind_letter(kind: Kind) -> &'static str {
    match kind {
        Kind::Series => "s",
        Kind::Film => "m",
    }
}

fn kind_of(letter: &str) -> Option<Kind> {
    match letter {
        "s" => Some(Kind::Series),
        "m" => Some(Kind::Film),
        _ => None,
    }
}

fn title_of_key(key: &str) -> Option<TitleId> {
    let (k, id) = key.split_once(':')?;
    Some(TitleId {
        kind: kind_of(k)?,
        id: id.parse().ok()?,
    })
}

/// What one source watched, its list and its thumbs.
pub struct Watching {
    path: PathBuf,
    items: HashMap<Item, Progress>,
    /// The list, in the order of adding, with when.
    list: Vec<(TitleId, i64)>,
    thumbs: HashMap<TitleId, Thumb>,
    dirty: bool,
    last_write: Option<Instant>,
    /// The last write failed: the next failure is not logged again.
    failing: bool,
}

impl Watching {
    /// What the file at `path` holds. A missing file holds nothing; one that
    /// cannot be read is kept as `<name>.bad`, and the store starts empty.
    pub fn load(path: PathBuf) -> Self {
        let mut me = Self {
            path,
            items: HashMap::new(),
            list: Vec::new(),
            thumbs: HashMap::new(),
            dirty: false,
            last_write: None,
            failing: false,
        };
        let bytes = match std::fs::read(&me.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return me,
            Err(e) => {
                tracing::warn!(
                    "vod: {} unreadable ({e}) - starting empty",
                    file_name(&me.path)
                );
                return me;
            }
        };
        let file = match serde_json::from_slice::<WatchFile>(&bytes) {
            Ok(file) if file.version == VERSION => file,
            Ok(file) => {
                me.keep_as_bad(&format!("version {}", file.version));
                return me;
            }
            Err(e) => {
                me.keep_as_bad(&e.to_string());
                return me;
            }
        };
        for (key, e) in file.items {
            let Some(item) = Item::of_key(&key) else {
                continue;
            };
            let episode = e.series.map(|series| EpisodeOf {
                series,
                season: e.s.unwrap_or(0),
                number: e.n.unwrap_or(0),
            });
            me.items.insert(
                item,
                Progress {
                    pos: e.pos,
                    dur: e.dur,
                    at: e.at,
                    done: e.done,
                    episode,
                },
            );
        }
        for e in file.list {
            if let Some(kind) = kind_of(&e.k) {
                let t = TitleId { kind, id: e.id };
                if !me.list.iter().any(|(had, _)| *had == t) {
                    me.list.push((t, e.at));
                }
            }
        }
        for (key, value) in file.thumbs {
            let thumb = match value {
                1 => Thumb::Up,
                -1 => Thumb::Down,
                _ => continue,
            };
            if let Some(t) = title_of_key(&key) {
                me.thumbs.insert(t, thumb);
            }
        }
        me
    }

    /// Moves the unreadable file aside as `<name>.bad` (over an older one)
    /// and says so once.
    fn keep_as_bad(&self, why: &str) {
        let bad = self.path.with_extension("json.bad");
        let kept = std::fs::rename(&self.path, &bad).is_ok();
        tracing::warn!(
            "vod: {} unreadable ({why}) - {} - starting empty",
            file_name(&self.path),
            if kept {
                "kept as .bad"
            } else {
                "left as it is"
            }
        );
    }

    /// Where `item` stopped.
    pub fn progress(&self, item: Item) -> Option<&Progress> {
        self.items.get(&item)
    }

    /// The most recent progress of `items`: a film title shows its versions'
    /// latest, so a new default version keeps where an older one stopped.
    pub fn latest(&self, items: impl IntoIterator<Item = Item>) -> Option<(Item, &Progress)> {
        items
            .into_iter()
            .filter_map(|i| Some((i, self.items.get(&i)?)))
            .max_by_key(|(_, p)| p.at)
    }

    /// The player says `item` is at `pos` of `dur` seconds (`now_unix`).
    /// Under 60 s an item is not started: one in progress goes, one that was
    /// seen stays seen. From 90 % on it is seen; under that, one that was seen
    /// is watched again and in progress. The same place again changes nothing.
    pub fn played(
        &mut self,
        item: Item,
        pos: f64,
        dur: f64,
        episode: Option<EpisodeOf>,
        now_unix: i64,
    ) {
        if !pos.is_finite() || pos < 0.0 {
            return;
        }
        let old = self.items.get(&item).copied();
        if pos < STARTED_SECS {
            if old.is_some_and(|p| !p.done) {
                self.items.remove(&item);
                self.dirty = true;
            }
            return;
        }
        let dur = if dur.is_finite() && dur > 0.0 {
            dur as u32
        } else {
            old.map_or(0, |p| p.dur)
        };
        let seen = dur > 0 && pos >= SEEN_SHARE * f64::from(dur);
        let new = Progress {
            pos: pos as u32,
            dur,
            at: now_unix,
            done: seen,
            episode: episode.or(old.and_then(|p| p.episode)),
        };
        let same = |p: &Progress| {
            p.pos == new.pos && p.dur == new.dur && p.done == new.done && p.episode == new.episode
        };
        if !old.is_some_and(|p| same(&p)) {
            self.items.insert(item, new);
            self.dirty = true;
        }
    }

    /// The end of `item`'s file was reached: it is seen.
    pub fn finished(&mut self, item: Item, dur: f64, episode: Option<EpisodeOf>, now_unix: i64) {
        let old = self.items.get(&item).copied();
        let dur = if dur.is_finite() && dur > 0.0 {
            dur as u32
        } else {
            old.map_or(0, |p| p.dur)
        };
        self.items.insert(
            item,
            Progress {
                pos: dur.max(old.map_or(0, |p| p.pos)),
                dur,
                at: now_unix,
                done: true,
                episode: episode.or(old.and_then(|p| p.episode)),
            },
        );
        self.dirty = true;
    }

    /// Whether title `t` is in the list.
    pub fn in_list(&self, t: TitleId) -> bool {
        self.list.iter().any(|(had, _)| *had == t)
    }

    /// ＋: puts `t` in the list, or takes it off when it is there. Whether it is
    /// in the list now.
    pub fn toggle_list(&mut self, t: TitleId, now_unix: i64) -> bool {
        self.dirty = true;
        if let Some(i) = self.list.iter().position(|(had, _)| *had == t) {
            self.list.remove(i);
            false
        } else {
            self.list.push((t, now_unix));
            true
        }
    }

    /// The list, the latest added first.
    pub fn list(&self) -> Vec<TitleId> {
        let mut list = self.list.clone();
        list.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        list.into_iter().map(|(t, _)| t).collect()
    }

    /// The thumb `t` has.
    pub fn thumb(&self, t: TitleId) -> Option<Thumb> {
        self.thumbs.get(&t).copied()
    }

    /// 👍 or 👎 pressed for `t`: it gets that thumb, or loses it when it had
    /// it already; 👍 and 👎 exclude each other. The thumb it has now.
    pub fn press_thumb(&mut self, t: TitleId, thumb: Thumb) -> Option<Thumb> {
        self.dirty = true;
        if self.thumbs.get(&t) == Some(&thumb) {
            self.thumbs.remove(&t);
            None
        } else {
            self.thumbs.insert(t, thumb);
            Some(thumb)
        }
    }

    /// Something changed that the file does not hold yet.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Writes the file when something changed and the last write is 5 s ago
    /// (`now`): what the player reports every second is written every 5 s.
    pub fn save_if_due(&mut self, now: Instant) {
        let due = self
            .last_write
            .is_none_or(|t| now.saturating_duration_since(t) >= WRITE_EVERY);
        if self.dirty && due {
            self.write(now);
        }
    }

    /// Writes the file now when something changed: a stop.
    pub fn flush(&mut self) {
        if self.dirty {
            self.write(Instant::now());
        }
    }

    fn write(&mut self, now: Instant) {
        self.last_write = Some(now);
        let file = WatchFile {
            version: VERSION,
            items: self
                .items
                .iter()
                .map(|(item, p)| {
                    let entry = ItemEntry {
                        pos: p.pos,
                        dur: p.dur,
                        at: p.at,
                        done: p.done,
                        series: p.episode.map(|e| e.series),
                        s: p.episode.map(|e| e.season),
                        n: p.episode.map(|e| e.number),
                    };
                    (item.key(), entry)
                })
                .collect(),
            list: self
                .list
                .iter()
                .map(|(t, at)| ListEntry {
                    k: kind_letter(t.kind).to_owned(),
                    id: t.id,
                    at: *at,
                })
                .collect(),
            thumbs: self
                .thumbs
                .iter()
                .map(|(t, thumb)| {
                    let value = match thumb {
                        Thumb::Up => 1,
                        Thumb::Down => -1,
                    };
                    (title_key(*t), value)
                })
                .collect(),
        };
        let written = write_atomic_with(&self.path, |out| {
            serde_json::to_writer_pretty(out, &file).map_err(std::io::Error::other)
        });
        match written {
            Ok(()) => {
                self.dirty = false;
                self.failing = false;
            }
            Err(e) => {
                if !self.failing {
                    tracing::warn!("vod: {} not written ({e})", file_name(&self.path));
                }
                self.failing = true;
            }
        }
    }
}

/// The file's name, for the log: its folder is the user's profile.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}
```

In `rust/src/storage.rs`, after `bad_streams_path`. After

```rust
    pub fn bad_streams_path(&self) -> PathBuf {
        self.source_file("bad_streams", self.source)
    }
```

add:

```rust
    /// What the source in use watched, its list and its thumbs
    /// (`vod::watching`): numbers and times only.
    pub fn watching_path(&self) -> PathBuf {
        self.source_file("watching", self.source)
    }
```

In `rust/src/storage.rs`, `remove_source_files`: its doc and the files it removes. Replace

```rust
    /// Removes source `id`'s favorites, presets, last channel, defect marks and
    /// saved list, with the temp file of that list; how many of them there were.
    pub fn remove_source_files(&self, id: SourceId) -> usize {
        [
            self.source_file("favorites", id),
            self.source_file("presets", id),
            self.source_file("last_watched", id),
            self.source_file("bad_streams", id),
```

with

```rust
    /// Removes source `id`'s favorites, presets, last channel, defect marks,
    /// what it watched (and the `.bad` copy of that file) and saved list, with
    /// the temp file of that list; how many of them there were.
    pub fn remove_source_files(&self, id: SourceId) -> usize {
        [
            self.source_file("favorites", id),
            self.source_file("presets", id),
            self.source_file("last_watched", id),
            self.source_file("bad_streams", id),
            self.source_file("watching", id),
            self.source_file("watching", id).with_extension("json.bad"),
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `ct --test test_vod_watching --test test_storage`
Expected: all pass (14 in `test_vod_watching`).

- [ ] **Step 5: Format, lint, full suite**

- Run `fmt src/vod/mod.rs src/storage.rs tests/test_vod_watching.rs tests/test_storage.rs`. Expected: no output.
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 6: Commit**

Message file (in the scratchpad):
```
feat(rust): what was watched: progress per film version and episode, Mijn lijst and thumbs, the 60 s and 90 % rules, a file per source written atomically

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/vod/watching.rs rust/src/vod/mod.rs rust/src/storage.rs rust/tests/test_vod_watching.rs rust/tests/test_storage.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/vod/watching.rs rust/src/vod/mod.rs rust/src/storage.rs rust/tests/test_vod_watching.rs rust/tests/test_storage.rs`

---

### Task 8: The details store

`DetailStore` (spec §4.3, Rulings 37 and 38): a page that opens asks for its title's details; a kept file shows in the same call, and the portal is asked in the background when there is none or it is too old (24 hours for a series, 30 days for a film), 2 at a time with the newest request first. Each answer is kept as a file with only what pages show, written through `write_atomic_with`, and without an image address that has a query (memory keeps it). Nothing creates a `DetailStore` before phase 2.

**Files:**
- Create: `rust/src/vod/details.rs`
- Modify: `rust/src/vod/mod.rs` (`pub mod details;`)
- Test: `rust/tests/test_vod_details.rs` (new)

**Interfaces:**
- Consumes: Task 1's `Portal::{fetch_vod_info, fetch_series_info}`, `VodDetails`; Task 6a's `cache::portal_key` and `write_atomic_with`; `Kind`, `TitleId`; `crate::args::XtreamCreds`.
- Produces:
  ```rust
  pub const SERIES_FRESH_SECS: i64;  // 86,400
  pub const FILM_FRESH_SECS: i64;    // 30 days
  pub const AT_ONCE: usize;          // 2
  pub const WAITING: usize;          // 8
  pub const IN_MEMORY: usize;        // 64
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum DetailState { Loading, Ready { details: Arc<VodDetails>, refreshing: bool }, Failed }
  pub struct DetailStore;
  impl DetailStore {
      pub fn new(portal: Arc<dyn Portal>, dir: Option<PathBuf>) -> Self;
      pub fn request(&mut self, id: TitleId, now_unix: i64);
      pub fn retry(&mut self, id: TitleId, now_unix: i64);
      pub fn get(&self, id: TitleId) -> DetailState;
      pub fn in_flight(&self) -> usize;
      pub fn drain(&mut self, now_unix: i64);                       // once per frame
  }
  pub fn dir_for(cache_dir: &Path, creds: Option<&XtreamCreds>) -> Option<PathBuf>;  // cache\vod\<key>
  pub fn file_name(id: TitleId) -> String;                                           // s4321.json, m5001.json
  ```
  Log lines: info `vod: details of <s|m><id> fetched in <ms> ms`; warn `vod: details of <s|m><id> not fetched (<e>)`, `vod: details of <s|m><id> not kept (<e>)`.

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_vod_details.rs`:

```rust
//! A film's and a series' details (vod/details.rs, spec 2026-10-15 §4.3):
//! asked when a page opens, kept as a file, fresh for a day (series) or 30
//! days (films), two at a time with the newest first.

use bongo_player::args::XtreamCreds;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::Portal;
use bongo_player::vod::details::{dir_for, file_name, DetailState, DetailStore, SERIES_FRESH_SECS};
use bongo_player::vod::{Kind, TitleId};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::query_param;
use wiremock::{Mock, MockServer, ResponseTemplate};

const NOW: i64 = 1_792_022_400;
const DAY: i64 = 86_400;

fn film(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Film,
        id,
    }
}

fn series(id: i64) -> TitleId {
    TitleId {
        kind: Kind::Series,
        id,
    }
}

fn portal_of(server: &MockServer) -> Arc<dyn Portal> {
    let uri = url::Url::parse(&server.uri()).unwrap();
    Arc::new(XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "secret".into(),
        host: uri.host_str().unwrap().into(),
        port: uri.port().unwrap(),
    }))
}

/// The portal answers a film's details with this plot, after `delay_ms`.
async fn film_answers(server: &MockServer, id: i64, plot: &str, delay_ms: u64) {
    let body = format!(
        r#"{{"info":{{"plot":"{plot}","direct_source":"http://h/movie/u/secret/{id}.mkv"}}}}"#
    );
    Mock::given(query_param("action", "get_vod_info"))
        .and(query_param("vod_id", id.to_string().as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .set_delay(Duration::from_millis(delay_ms)),
        )
        .mount(server)
        .await;
}

/// Lets the store take in answers until `done` says so (2 s at most).
async fn drain_until(store: &mut DetailStore, mut done: impl FnMut(&DetailStore) -> bool) {
    for _ in 0..400 {
        store.drain(NOW);
        if done(store) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the answers never came");
}

fn plot(state: DetailState) -> Option<(String, bool)> {
    match state {
        DetailState::Ready {
            details,
            refreshing,
        } => Some((details.plot.clone(), refreshing)),
        _ => None,
    }
}

/// How many times the portal was asked for these details.
async fn asked(server: &MockServer, action: &str) -> usize {
    let requests = server.received_requests().await.unwrap_or_default();
    requests
        .iter()
        .filter(|r| {
            r.url
                .query()
                .unwrap_or("")
                .contains(&format!("action={action}"))
        })
        .count()
}

#[tokio::test]
async fn a_page_asks_once_and_keeps_the_answer_as_a_file_without_the_account() {
    let server = MockServer::start().await;
    film_answers(&server, 5001, "A hacker.", 0).await;
    let dir = tempfile::tempdir().unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    assert_eq!(store.get(film(5001)), DetailState::Loading, "never asked");
    store.request(film(5001), NOW);
    assert_eq!(store.get(film(5001)), DetailState::Loading);
    drain_until(&mut store, |s| plot(s.get(film(5001))).is_some()).await;
    assert_eq!(
        plot(store.get(film(5001))),
        Some(("A hacker.".into(), false))
    );
    let text = std::fs::read_to_string(dir.path().join(file_name(film(5001)))).unwrap();
    assert!(text.contains("A hacker."));
    assert!(
        !text.contains("secret") && !text.contains("direct_source"),
        "{text}"
    );
    // A second page open within the month: the file, no question.
    let mut again = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    again.request(film(5001), NOW + 29 * DAY);
    assert_eq!(
        plot(again.get(film(5001))),
        Some(("A hacker.".into(), false)),
        "in the same call"
    );
    assert_eq!(again.in_flight(), 0);
    assert_eq!(asked(&server, "get_vod_info").await, 1);
}

#[tokio::test]
async fn an_old_file_shows_while_the_portal_is_asked_again() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_series_info"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"info":{"plot":"New."}}"#))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(file_name(series(4321)));
    let old = serde_json::json!({"version": 1, "fetched_at": NOW - SERIES_FRESH_SECS - 1, "details": {"plot": "Old."}});
    std::fs::write(&path, old.to_string()).unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(series(4321), NOW);
    assert_eq!(
        plot(store.get(series(4321))),
        Some(("Old.".into(), true)),
        "older than a day"
    );
    drain_until(&mut store, |s| {
        plot(s.get(series(4321))) == Some(("New.".into(), false))
    })
    .await;
}

#[tokio::test]
async fn a_series_file_of_less_than_a_day_and_a_film_file_of_less_than_a_month_are_fresh() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    for (id, age) in [
        (series(1), SERIES_FRESH_SECS),
        (film(2), 30 * DAY),
        (film(3), -DAY),
    ] {
        let file = serde_json::json!({"version": 1, "fetched_at": NOW - age, "details": {"plot": "Kept."}});
        std::fs::write(dir.path().join(file_name(id)), file.to_string()).unwrap();
    }
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    for id in [series(1), film(2), film(3)] {
        store.request(id, NOW);
        assert_eq!(plot(store.get(id)), Some(("Kept.".into(), false)), "{id:?}");
    }
    assert_eq!(store.in_flight(), 0, "a time from the future counts as now");
}

#[tokio::test]
async fn two_at_a_time_and_the_newest_request_first() {
    let server = MockServer::start().await;
    // 1 and 2 answer apart, so 4 and 3 start in two drains, in that order.
    for id in 1..=4 {
        let delay = if id == 2 { 600 } else { 300 };
        film_answers(&server, id, &format!("Film {id}."), delay).await;
    }
    let mut store = DetailStore::new(portal_of(&server), None);
    for id in 1..=4 {
        store.request(film(id), NOW);
    }
    assert_eq!(store.in_flight(), 2);
    drain_until(&mut store, |s| {
        (1..=4).all(|id| plot(s.get(film(id))).is_some())
    })
    .await;
    let order: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| {
            r.url
                .query_pairs()
                .find(|(k, _)| k == "vod_id")
                .map(|(_, v)| v.into_owned())
        })
        .collect();
    assert_eq!(
        &order[2..],
        ["4", "3"],
        "the newest waiting request first: {order:?}"
    );
}

#[tokio::test]
async fn a_failed_fetch_without_a_file_says_so_and_enter_asks_again() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_info"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    film_answers(&server, 7, "Second time.", 0).await;
    let mut store = DetailStore::new(portal_of(&server), None);
    store.request(film(7), NOW);
    drain_until(&mut store, |s| s.get(film(7)) == DetailState::Failed).await;
    store.request(film(7), NOW);
    assert_eq!(
        store.get(film(7)),
        DetailState::Failed,
        "opening again asks nothing"
    );
    store.retry(film(7), NOW);
    drain_until(&mut store, |s| plot(s.get(film(7))).is_some()).await;
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_old_details() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_info"))
        .respond_with(ResponseTemplate::new(509))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let old = serde_json::json!({"version": 1, "fetched_at": NOW - 31 * DAY, "details": {"plot": "Old."}});
    std::fs::write(dir.path().join(file_name(film(8))), old.to_string()).unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(8), NOW);
    drain_until(&mut store, |s| {
        s.in_flight() == 0 && plot(s.get(film(8))) == Some(("Old.".into(), false))
    })
    .await;
}

#[tokio::test]
async fn a_broken_file_is_asked_again_and_written_anew() {
    let server = MockServer::start().await;
    film_answers(&server, 9, "Fixed.", 0).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(file_name(film(9)));
    std::fs::write(&path, b"{not json").unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(9), NOW);
    assert_eq!(store.get(film(9)), DetailState::Loading);
    drain_until(&mut store, |s| plot(s.get(film(9))).is_some()).await;
    for _ in 0..100 {
        if std::fs::read_to_string(&path).is_ok_and(|t| t.contains("Fixed.")) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the file was not written anew");
}

#[tokio::test]
async fn only_the_eight_newest_requests_wait() {
    let server = MockServer::start().await;
    Mock::given(query_param("action", "get_vod_info"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"info":{"plot":"x"}}"#)
                .set_delay(Duration::from_millis(200)),
        )
        .mount(&server)
        .await;
    let mut store = DetailStore::new(portal_of(&server), None);
    for id in 1..=12 {
        store.request(film(id), NOW);
    }
    // 1 and 2 are on their way; 3 and 4 were dropped from the 10 that waited.
    drain_until(&mut store, |s| {
        s.in_flight() == 0 && (5..=12).all(|id| plot(s.get(film(id))).is_some())
    })
    .await;
    assert_eq!(store.get(film(3)), DetailState::Loading);
    assert_eq!(asked(&server, "get_vod_info").await, 10);
}

#[tokio::test]
async fn an_image_address_with_a_query_shows_but_is_not_kept() {
    let server = MockServer::start().await;
    let body = r#"{"info":{"plot":"Kids vanish.","backdrop_path":"https://cdn.example/b.jpg?token=secret"},
        "seasons":[{"season_number":1,"cover":"https://cdn.example/s1.jpg?token=secret"}],
        "episodes":{"1":[{"id":"7","episode_num":1,"info":{"movie_image":"https://cdn.example/e.jpg?token=secret"}}]}}"#;
    Mock::given(query_param("action", "get_series_info"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(series(4321), NOW);
    drain_until(&mut store, |s| plot(s.get(series(4321))).is_some()).await;
    let DetailState::Ready { details, .. } = store.get(series(4321)) else {
        panic!("no details");
    };
    assert_eq!(details.backdrop, "https://cdn.example/b.jpg?token=secret");
    assert!(details.seasons[0].cover.contains('?') && details.episodes[0].still.contains('?'));
    // The file is written before the answer is handed over.
    let text = std::fs::read_to_string(dir.path().join(file_name(series(4321)))).unwrap();
    assert!(
        text.contains("Kids vanish.") && !text.contains("token="),
        "redact sees no token there: {text}"
    );
}

#[tokio::test]
async fn a_file_from_the_far_past_or_the_far_future_is_read_without_a_panic() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    for (id, fetched_at) in [(film(10), i64::MIN), (film(11), i64::MAX)] {
        let file = serde_json::json!({"version": 1, "fetched_at": fetched_at, "details": {"plot": "Kept."}});
        std::fs::write(dir.path().join(file_name(id)), file.to_string()).unwrap();
    }
    let mut store = DetailStore::new(portal_of(&server), Some(dir.path().to_path_buf()));
    store.request(film(10), NOW);
    store.request(film(11), NOW);
    assert_eq!(
        plot(store.get(film(10))),
        Some(("Kept.".into(), true)),
        "the far past: asked again"
    );
    assert_eq!(
        plot(store.get(film(11))),
        Some(("Kept.".into(), false)),
        "a time from the future counts as now"
    );
    assert_eq!(store.in_flight(), 1);
}

#[test]
fn the_folder_and_files_are_named_after_the_portal_and_the_title() {
    let creds = XtreamCreds {
        username: "user".into(),
        password: "secret".into(),
        host: "pandora.example".into(),
        port: 8080,
    };
    let dir = dir_for(Path::new("C:/cfg/cache"), Some(&creds)).unwrap();
    assert_eq!(
        dir,
        Path::new("C:/cfg/cache")
            .join("vod")
            .join("pandora.example_8080")
    );
    assert!(dir_for(Path::new("C:/cfg/cache"), None).is_none());
    assert_eq!(file_name(series(4321)), "s4321.json");
    assert_eq!(file_name(film(5001)), "m5001.json");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_vod_details`
Expected: compile errors: there is no module `vod::details`.

- [ ] **Step 3: The store**

In `rust/src/vod/mod.rs`, declare the details store: before the line `pub mod genres;`, add:

```rust
pub mod details;
```

Create `rust/src/vod/details.rs`:

```rust
//! A film's and a series' details (spec 2026-10-15 §4.3), asked of the portal
//! when their page opens: on the tokio runtime, at most 2 at a time, the
//! newest request first. Each answer is kept as a file with only what the
//! pages show (`config\cache\vod\<host>_<port>\<s|m><id>.json`, a
//! `VodDetails`), and without an image address that has a query: `redact`
//! sees no token there (memory keeps it, so the page shows it). A series' file
//! is asked again once it is older than 24 hours, a film's after 30 days; the
//! old file shows meanwhile. Opening a page reads its file in the same call, so
//! it shows in the frame that opens it.

use super::cache::portal_key;
use super::{write_atomic_with, Kind, TitleId};
use crate::args::XtreamCreds;
use crate::portal::{Portal, VodDetails};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A series' details are asked again after this, in seconds: a day.
pub const SERIES_FRESH_SECS: i64 = 24 * 60 * 60;
/// A film's after this: 30 days.
pub const FILM_FRESH_SECS: i64 = 30 * 24 * 60 * 60;
/// Requests to the portal at the same time.
pub const AT_ONCE: usize = 2;
/// Requests that wait for a place; an older one is dropped (its page is long
/// closed, and opening it asks again).
pub const WAITING: usize = 8;
/// Titles whose details are kept in memory; an older one is read from its
/// file again when its page opens.
pub const IN_MEMORY: usize = 64;

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct DetailFile {
    version: u32,
    /// When the portal gave them, unix seconds.
    fetched_at: i64,
    details: VodDetails,
}

/// What a page has of its title's details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailState {
    /// Asked for, nothing to show yet; or never asked.
    Loading,
    /// There they are. `refreshing`: an old file shows while the portal is
    /// asked again.
    Ready {
        details: Arc<VodDetails>,
        refreshing: bool,
    },
    /// The portal gave none and no file had them: "Details konden niet geladen
    /// worden. Enter probeert opnieuw" (`DetailStore::retry`).
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fetch {
    Idle,
    Waiting,
    InFlight,
    Failed,
}

struct Entry {
    details: Option<Arc<VodDetails>>,
    fetch: Fetch,
    /// When its page last asked, as a count: the least recent leave memory.
    asked: u64,
}

/// What came back from the portal.
struct Arrived {
    id: TitleId,
    answer: Result<VodDetails, String>,
}

/// The details of the titles whose pages were opened.
pub struct DetailStore {
    portal: Arc<dyn Portal>,
    /// Where the files are; none keeps none.
    dir: Option<PathBuf>,
    entries: HashMap<TitleId, Entry>,
    /// Waiting for a place, the newest last.
    waiting: Vec<TitleId>,
    in_flight: usize,
    asks: u64,
    done: Arc<Mutex<Vec<Arrived>>>,
}

/// The folder of the details of the portal with these credentials, in
/// `cache_dir`; none without a portal.
pub fn dir_for(cache_dir: &Path, creds: Option<&XtreamCreds>) -> Option<PathBuf> {
    let c = creds?;
    Some(cache_dir.join("vod").join(portal_key(&c.host, c.port)))
}

/// A title's file name: `s4321.json`, `m5001.json`.
pub fn file_name(id: TitleId) -> String {
    let kind = match id.kind {
        Kind::Series => 's',
        Kind::Film => 'm',
    };
    format!("{kind}{}.json", id.id)
}

/// The id as the log names it: `s4321`, `m5001`.
fn log_name(id: TitleId) -> String {
    file_name(id).trim_end_matches(".json").to_owned()
}

/// How long `id`'s details stay fresh.
fn fresh_secs(id: TitleId) -> i64 {
    match id.kind {
        Kind::Series => SERIES_FRESH_SECS,
        Kind::Film => FILM_FRESH_SECS,
    }
}

impl DetailStore {
    /// Details asked of `portal`, kept as files in `dir`.
    pub fn new(portal: Arc<dyn Portal>, dir: Option<PathBuf>) -> Self {
        Self {
            portal,
            dir,
            entries: HashMap::new(),
            waiting: Vec::new(),
            in_flight: 0,
            asks: 0,
            done: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The page of `id` opens at `now_unix`: its file shows at once, and the
    /// portal is asked when there is none or it is too old.
    pub fn request(&mut self, id: TitleId, now_unix: i64) {
        self.asks += 1;
        let asked = self.asks;
        let file = match self.entries.get(&id) {
            Some(e) if e.details.is_some() || e.fetch != Fetch::Idle => None,
            _ => Some(self.read_file(id)),
        };
        let entry = self.entries.entry(id).or_insert(Entry {
            details: None,
            fetch: Fetch::Idle,
            asked,
        });
        entry.asked = asked;
        let mut stale = entry.details.is_none();
        if let Some(Some((details, fetched_at))) = file {
            entry.details = Some(Arc::new(details));
            // A time from the future (a clock that was wrong) counts as now.
            stale = now_unix.saturating_sub(fetched_at.min(now_unix)) > fresh_secs(id);
        }
        if stale && entry.fetch == Fetch::Idle {
            self.wait(id);
        }
        self.forget_old();
        self.start_waiting(now_unix);
    }

    /// Enter on a page whose details failed: asks again.
    pub fn retry(&mut self, id: TitleId, now_unix: i64) {
        if let Some(e) = self.entries.get_mut(&id) {
            if e.fetch == Fetch::Failed {
                e.fetch = Fetch::Idle;
                self.wait(id);
                self.start_waiting(now_unix);
            }
        }
    }

    /// What the page of `id` has.
    pub fn get(&self, id: TitleId) -> DetailState {
        let Some(e) = self.entries.get(&id) else {
            return DetailState::Loading;
        };
        match (&e.details, e.fetch) {
            (Some(details), fetch) => DetailState::Ready {
                details: details.clone(),
                refreshing: matches!(fetch, Fetch::Waiting | Fetch::InFlight),
            },
            (None, Fetch::Failed) => DetailState::Failed,
            (None, _) => DetailState::Loading,
        }
    }

    /// Requests on their way to the portal.
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// Takes in what came back and starts what waits; once per frame.
    pub fn drain(&mut self, now_unix: i64) {
        let arrived = std::mem::take(&mut *self.done.lock());
        for Arrived { id, answer } in arrived {
            self.in_flight = self.in_flight.saturating_sub(1);
            let Some(e) = self.entries.get_mut(&id) else {
                continue;
            };
            match answer {
                Ok(details) => {
                    e.details = Some(Arc::new(details));
                    e.fetch = Fetch::Idle;
                }
                // An old file keeps showing; without one the page says so.
                Err(_) if e.details.is_some() => e.fetch = Fetch::Idle,
                Err(_) => e.fetch = Fetch::Failed,
            }
        }
        self.start_waiting(now_unix);
    }

    /// Puts `id` at the newest end of the waiting requests.
    fn wait(&mut self, id: TitleId) {
        self.waiting.retain(|w| *w != id);
        self.waiting.push(id);
        if let Some(e) = self.entries.get_mut(&id) {
            e.fetch = Fetch::Waiting;
        }
        while self.waiting.len() > WAITING {
            let dropped = self.waiting.remove(0);
            if let Some(e) = self.entries.get_mut(&dropped) {
                e.fetch = Fetch::Idle;
            }
        }
    }

    /// Starts the newest waiting requests while a place is free.
    fn start_waiting(&mut self, now_unix: i64) {
        while self.in_flight < AT_ONCE {
            let Some(id) = self.waiting.pop() else {
                break;
            };
            if let Some(e) = self.entries.get_mut(&id) {
                e.fetch = Fetch::InFlight;
            }
            self.in_flight += 1;
            self.start(id, now_unix);
        }
    }

    /// Asks the portal for `id`'s details in the background; the answer is
    /// kept as its file before it is handed over.
    fn start(&self, id: TitleId, now_unix: i64) {
        let (portal, done) = (self.portal.clone(), self.done.clone());
        let path = self.dir.as_ref().map(|d| d.join(file_name(id)));
        tokio::spawn(async move {
            let t0 = std::time::Instant::now();
            let answer = match id.kind {
                Kind::Film => portal.fetch_vod_info(id.id).await,
                Kind::Series => portal.fetch_series_info(id.id).await,
            };
            let answer = match answer {
                Ok(details) => {
                    tracing::info!(
                        "vod: details of {} fetched in {} ms",
                        log_name(id),
                        t0.elapsed().as_millis()
                    );
                    if let Some(path) = path {
                        let file = DetailFile {
                            version: VERSION,
                            fetched_at: now_unix,
                            details: without_query_images(details.clone()),
                        };
                        let _ = tokio::task::spawn_blocking(move || save(&path, &file, id)).await;
                    }
                    Ok(details)
                }
                Err(e) => {
                    tracing::warn!("vod: details of {} not fetched ({e})", log_name(id));
                    Err(e.to_string())
                }
            };
            done.lock().push(Arrived { id, answer });
        });
    }

    /// `id`'s file and when it was fetched; none when there is none, or it is
    /// no details file of this version (it is asked again and written anew).
    fn read_file(&self, id: TitleId) -> Option<(VodDetails, i64)> {
        let path = self.dir.as_ref()?.join(file_name(id));
        let bytes = std::fs::read(path).ok()?;
        match serde_json::from_slice::<DetailFile>(&bytes) {
            Ok(file) if file.version == VERSION => Some((file.details, file.fetched_at)),
            _ => None,
        }
    }

    /// Keeps the [`IN_MEMORY`] titles asked last; one that waits or is on its
    /// way stays.
    fn forget_old(&mut self) {
        while self.entries.len() > IN_MEMORY {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, e)| matches!(e.fetch, Fetch::Idle | Fetch::Failed))
                .min_by_key(|(_, e)| e.asked)
                .map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    self.entries.remove(&id);
                }
                None => break,
            }
        }
    }
}

/// `details` as their file keeps them: an image address with a query is
/// emptied, since `redact` sees no token there. Memory keeps it, so the page
/// shows the image.
fn without_query_images(mut details: VodDetails) -> VodDetails {
    let clear = |url: &mut String| {
        if url.contains('?') {
            url.clear();
        }
    };
    clear(&mut details.backdrop);
    for s in &mut details.seasons {
        clear(&mut s.cover);
    }
    for e in &mut details.episodes {
        clear(&mut e.still);
    }
    details
}

/// Writes `file` to `path` through a temporary file (`write_atomic_with`); a
/// failure is logged.
fn save(path: &Path, file: &DetailFile, id: TitleId) {
    let written = write_atomic_with(path, |out| {
        serde_json::to_writer(out, file).map_err(std::io::Error::other)
    });
    if let Err(e) = written {
        tracing::warn!("vod: details of {} not kept ({e})", log_name(id));
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `ct --test test_vod_details`
Expected: all pass (11).

- [ ] **Step 5: Format, lint, full suite**

- Run `fmt src/vod/mod.rs tests/test_vod_details.rs`. Expected: no output.
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 6: Commit**

Message file (in the scratchpad):
```
feat(rust): the details store: asked when a page opens, 2 at a time with the newest first, a file per title, fresh for a day or 30 days

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/vod/details.rs rust/src/vod/mod.rs rust/tests/test_vod_details.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/vod/details.rs rust/src/vod/mod.rs rust/tests/test_vod_details.rs`

---

### Task 9: The cover store

`CoverStore` (spec §4.4, Rulings 39–43): the covers a frame asks for, read from their kept files or downloaded (6 at a time), dropped when a frame no longer asks before they come, decoded on the blocking pool into the spec's fixed sizes, and kept as textures up to 96 MB, the one shown longest ago going first. The texture LRU is generic over a small trait, so its tests use fake textures and no GPU. `image` gains its JPEG decoder. Nothing creates a `CoverStore` before phase 2.

**Files:**
- Create: `rust/src/vod/covers.rs`
- Modify: `rust/src/vod/mod.rs` (`pub mod covers;`), `rust/src/main.rs` (the folder pruned at a start), `rust/Cargo.toml` (`image` with `jpeg`), `rust/Cargo.lock` (cargo adds `jpeg-decoder`)
- Test: `rust/tests/test_vod_covers.rs` (new)

**Interfaces:**
- Consumes: `guide::logos::host_of`; `ring::digest`; `tokio::sync::Semaphore`; `egui::{Context, TextureHandle, ColorImage}`.
- Produces:
  ```rust
  pub const AT_ONCE: usize;           // 6 downloads
  pub const READS_AT_ONCE: usize;     // 6 reads of kept files
  pub const TEXTURE_BYTES: usize;     // 96 MB
  pub const DISK_BYTES: u64;          // 500 MB
  pub const UPLOADS_PER_FRAME: usize; // 8
  pub enum Shape { Poster, Landscape, Backdrop }
  impl Shape { pub fn sizes(self) -> &'static [[u32; 2]]; pub fn size_for(self, shown: [f32; 2]) -> [u32; 2]; }
  pub struct CoverKey { pub hash: u64, pub size: [u32; 2] }
  pub fn url_hash(url: &str) -> u64;
  pub fn file_stem(url: &str) -> String;                      // 16 hex digits
  pub fn placeholder_tint(name: &str) -> egui::Color32;
  pub trait CoverTexture { fn byte_size(&self) -> usize; }   // egui::TextureHandle has it
  pub trait Uploader { type Texture: CoverTexture; fn upload(&self, name: String, image: egui::ColorImage) -> Self::Texture; }  // egui::Context
  pub struct TextureLru<T>;
  impl<T: CoverTexture> TextureLru<T> { pub fn new(cap: usize) -> Self; pub fn touch(&mut self, key: &CoverKey) -> bool;
      pub fn peek(&self, key: &CoverKey) -> Option<&T>; pub fn insert(&mut self, key: CoverKey, texture: T);
      pub fn used(&self) -> usize; pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool; }
  pub struct CoverStore<U: Uploader>;
  impl<U: Uploader> CoverStore<U> {
      pub fn new(dir: PathBuf, uploader: U) -> Self;
      pub fn cover(&mut self, url: &str, shape: Shape, shown: [f32; 2]) -> Option<&U::Texture>;
      pub fn is_loading(&self, url: &str, shape: Shape, shown: [f32; 2]) -> bool;
      pub fn downloads_in_flight(&self) -> usize;
      pub fn texture_bytes(&self) -> usize;
      pub fn end_frame(&mut self);                           // once per frame, after the covers were asked
  }
  pub fn decode(bytes: &[u8], size: [u32; 2]) -> Option<egui::ColorImage>;
  pub fn prune(dir: &Path, cap: u64) -> std::io::Result<(usize, u64)>;
  pub fn prune_in_background(dir: PathBuf);
  ```
  Log lines: debug `cover: <host> could not be fetched - the placeholder shows`, `cover: <host> is no JPEG or PNG - the placeholder shows`; warn `cover: no HTTP client (<e>) - the placeholders show`; info `covers: <n> oldest files removed, <m> MB left`, warn `covers: folder not pruned (<e>)`.

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_vod_covers.rs`:

```rust
//! Covers (vod/covers.rs, spec 2026-10-15 §4.4): the sizes, the texture LRU
//! with fake textures, fetching, keeping, decoding and dropping covers, the
//! disk cache's cap, and the placeholder's colour. Every server is local.

use bongo_player::vod::covers::{
    decode, file_stem, placeholder_tint, prune, CoverKey, CoverStore, CoverTexture, Shape,
    TextureLru, Uploader, AT_ONCE,
};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

const MB: usize = 1024 * 1024;

/// A texture that only says its size: no GPU, no memory.
#[derive(Debug, Clone)]
struct FakeTexture {
    name: String,
    size: [usize; 2],
    bytes: usize,
}

impl CoverTexture for FakeTexture {
    fn byte_size(&self) -> usize {
        self.bytes
    }
}

/// Makes fake textures and remembers their names.
#[derive(Clone, Default)]
struct FakeUploader {
    made: Arc<Mutex<Vec<String>>>,
}

impl Uploader for FakeUploader {
    type Texture = FakeTexture;

    fn upload(&self, name: String, image: egui::ColorImage) -> FakeTexture {
        self.made.lock().push(name.clone());
        let [w, h] = image.size;
        FakeTexture {
            name,
            size: image.size,
            bytes: w * h * 4,
        }
    }
}

fn fake(mb: usize) -> FakeTexture {
    FakeTexture {
        name: String::new(),
        size: [0, 0],
        bytes: mb * MB,
    }
}

fn key(n: u64) -> CoverKey {
    CoverKey {
        hash: n,
        size: [300, 450],
    }
}

fn picture(w: u32, h: u32, format: image::ImageOutputFormat) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 40, 40]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, format)
        .unwrap();
    out.into_inner()
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    picture(w, h, image::ImageOutputFormat::Jpeg(80))
}

fn png(w: u32, h: u32) -> Vec<u8> {
    picture(w, h, image::ImageOutputFormat::Png)
}

#[test]
fn a_cover_is_made_in_the_first_size_at_least_as_big_as_its_tile() {
    assert_eq!(Shape::Poster.size_for([150.0, 225.0]), [200, 300]);
    assert_eq!(Shape::Poster.size_for([200.0, 300.0]), [200, 300]);
    assert_eq!(Shape::Poster.size_for([201.0, 301.0]), [300, 450]);
    assert_eq!(
        Shape::Poster.size_for([900.0, 1350.0]),
        [400, 600],
        "the largest"
    );
    assert_eq!(Shape::Landscape.size_for([400.0, 225.0]), [480, 270]);
    assert_eq!(Shape::Backdrop.size_for([1300.0, 700.0]), [1920, 1080]);
}

#[test]
fn the_textures_keep_96_mb_and_the_one_shown_longest_ago_goes_first() {
    let mut lru = TextureLru::new(96 * MB);
    lru.insert(key(1), fake(30));
    lru.insert(key(2), fake(30));
    lru.insert(key(3), fake(30));
    assert!(lru.touch(&key(1)), "shown again");
    lru.insert(key(4), fake(30));
    assert_eq!(lru.used(), 90 * MB);
    assert!(lru.peek(&key(2)).is_none(), "shown longest ago");
    assert!(
        lru.peek(&key(1)).is_some() && lru.peek(&key(3)).is_some() && lru.peek(&key(4)).is_some()
    );
    assert!(!lru.touch(&key(2)));
    // One too big for the cap alone stays alone.
    lru.insert(key(5), fake(100));
    assert_eq!(lru.len(), 1);
    assert_eq!(lru.used(), 100 * MB);
    // The same key again replaces its texture.
    lru.insert(key(5), fake(10));
    assert_eq!((lru.len(), lru.used()), (1, 10 * MB));
}

#[test]
fn jpeg_and_png_are_made_to_fill_the_size_and_anything_else_is_no_cover() {
    for bytes in [jpeg(600, 900), png(640, 900), jpeg(100, 100)] {
        let image = decode(&bytes, [300, 450]).unwrap();
        assert_eq!(image.size, [300, 450]);
    }
    assert!(decode(b"GIF89a....", [300, 450]).is_none());
    assert!(decode(b"<html>busy</html>", [300, 450]).is_none());
    assert!(
        decode(&[0xff, 0xd8, 0xff, 0x00], [300, 450]).is_none(),
        "a cut-off JPEG"
    );
    assert!(
        decode(&png(8193, 2), [300, 450]).is_none(),
        "a canvas too big"
    );
}

#[test]
fn a_title_without_a_cover_always_gets_its_own_colour() {
    let a = placeholder_tint("The Matrix");
    assert_eq!(a, placeholder_tint("The Matrix"));
    assert_ne!(a, placeholder_tint("Dark"));
    assert_eq!(a.a(), 255);
    assert!(
        a.r() < 160 && a.g() < 160 && a.b() < 160,
        "dark enough for white letters: {a:?}"
    );
}

#[test]
fn a_cover_file_is_named_after_a_hash_of_its_address() {
    let url = "http://h:80/images/u/p/1.jpg?token=secret";
    let stem = file_stem(url);
    assert_eq!(stem.len(), 16);
    assert!(stem.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(stem, file_stem(url));
    assert_ne!(stem, file_stem("http://h:80/images/u/p/2.jpg"));
}

/// Ends frames, each asking for these covers again, until `done` says so.
async fn frames_until(
    store: &mut CoverStore<FakeUploader>,
    ask: &[String],
    mut done: impl FnMut(&mut CoverStore<FakeUploader>) -> bool,
) {
    for _ in 0..400 {
        for url in ask {
            store.cover(url, Shape::Poster, [300.0, 450.0]);
        }
        store.end_frame();
        if done(store) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the covers never came");
}

fn shown(store: &mut CoverStore<FakeUploader>, url: &str) -> Option<FakeTexture> {
    store.cover(url, Shape::Poster, [300.0, 450.0]).cloned()
}

#[tokio::test]
async fn a_cover_is_fetched_once_kept_as_its_bytes_and_read_from_the_file_next_time() {
    let server = MockServer::start().await;
    Mock::given(path("/c.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(600, 900)))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/c.jpg?token=secret", server.uri());
    let uploader = FakeUploader::default();
    let mut store = CoverStore::new(dir.path().to_path_buf(), uploader.clone());
    assert!(
        shown(&mut store, &url).is_none(),
        "asked for, not there yet"
    );
    let ask = [url.clone()];
    frames_until(&mut store, &ask, |s| shown(s, &url).is_some()).await;
    let texture = shown(&mut store, &url).unwrap();
    assert_eq!(texture.size, [300, 450]);
    assert_eq!(
        texture.name,
        format!("cover {} 300x450", file_stem(&url)),
        "no address in it"
    );
    let kept = dir.path().join(format!("{}.jpg", file_stem(&url)));
    assert_eq!(
        std::fs::read(&kept).unwrap(),
        jpeg(600, 900),
        "the bytes as they came"
    );
    // Another session, another size: read from the file (expect(1) holds).
    let mut again = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    let big = [url.clone()];
    for _ in 0..400 {
        if again
            .cover(&big[0], Shape::Poster, [400.0, 600.0])
            .is_some()
        {
            break;
        }
        again.end_frame();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        again
            .cover(&big[0], Shape::Poster, [400.0, 600.0])
            .unwrap()
            .size,
        [400, 600]
    );
}

#[tokio::test]
async fn an_error_page_or_a_gif_is_no_cover_is_not_kept_and_is_not_asked_again() {
    let server = MockServer::start().await;
    Mock::given(path("/a.gif"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"GIF89a....".to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/busy.jpg"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let urls = [
        format!("{}/a.gif", server.uri()),
        format!("{}/busy.jpg", server.uri()),
    ];
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    frames_until(&mut store, &urls, |s| {
        urls.iter()
            .all(|u| !s.is_loading(u, Shape::Poster, [300.0, 450.0]))
    })
    .await;
    for _ in 0..5 {
        for url in &urls {
            assert!(store.cover(url, Shape::Poster, [300.0, 450.0]).is_none());
        }
        store.end_frame();
    }
    assert_eq!(
        std::fs::read_dir(dir.path()).map_or(0, |d| d.count()),
        0,
        "nothing kept"
    );
}

#[tokio::test]
async fn six_downloads_at_a_time_and_a_cover_that_leaves_the_screen_is_dropped() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(jpeg(60, 90))
                .set_delay(Duration::from_millis(600)),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let urls: Vec<String> = (0..10)
        .map(|i| format!("{}/{i}.jpg", server.uri()))
        .collect();
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    for url in &urls {
        store.cover(url, Shape::Poster, [300.0, 450.0]);
    }
    store.end_frame();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(store.downloads_in_flight(), AT_ONCE);
    // The page scrolls on: no frame asks for these any more.
    store.end_frame();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(store.downloads_in_flight(), 0, "dropped, their places free");
    assert!(urls
        .iter()
        .all(|u| !store.is_loading(u, Shape::Poster, [300.0, 450.0])));
}

#[tokio::test]
async fn no_more_than_eight_textures_are_made_in_a_frame() {
    let dir = tempfile::tempdir().unwrap();
    let urls: Vec<String> = (0..12)
        .map(|i| format!("http://covers.example/{i}.png"))
        .collect();
    for url in &urls {
        std::fs::write(
            dir.path().join(format!("{}.png", file_stem(url))),
            png(60, 90),
        )
        .unwrap();
    }
    let uploader = FakeUploader::default();
    let mut store = CoverStore::new(dir.path().to_path_buf(), uploader.clone());
    let frame = |store: &mut CoverStore<FakeUploader>| {
        for url in &urls {
            store.cover(url, Shape::Poster, [300.0, 450.0]);
        }
        store.end_frame();
    };
    frame(&mut store);
    // Twelve small files are read and decoded well within this.
    tokio::time::sleep(Duration::from_millis(500)).await;
    frame(&mut store);
    assert_eq!(uploader.made.lock().len(), 8, "one frame's worth");
    frame(&mut store);
    assert_eq!(uploader.made.lock().len(), 12, "the rest in the next frame");
}

#[test]
fn the_folder_is_pruned_to_its_cap_the_oldest_files_first() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    for (name, age_days) in [("old.jpg", 30u64), ("mid.jpg", 10), ("new.jpg", 1)] {
        let file = dir.path().join(name);
        std::fs::write(&file, vec![0u8; 1000]).unwrap();
        let written = now - Duration::from_secs(age_days * 86_400);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(written)
            .unwrap();
    }
    assert_eq!(
        prune(dir.path(), 3000).unwrap(),
        (0, 3000),
        "within the cap"
    );
    assert_eq!(prune(dir.path(), 2500).unwrap(), (1, 2000));
    assert!(!dir.path().join("old.jpg").exists());
    assert!(dir.path().join("mid.jpg").exists() && dir.path().join("new.jpg").exists());
    assert_eq!(prune(&dir.path().join("missing"), 0).unwrap(), (0, 0));
}

#[tokio::test]
async fn a_kept_cover_needs_no_server_and_costs_its_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let url = "http://covers.example/kept.png".to_string();
    std::fs::write(
        dir.path().join(format!("{}.png", file_stem(&url))),
        png(60, 90),
    )
    .unwrap();
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    assert_eq!(store.texture_bytes(), 0);
    frames_until(&mut store, std::slice::from_ref(&url), |s| {
        shown(s, &url).is_some()
    })
    .await;
    assert_eq!(store.texture_bytes(), 300 * 450 * 4);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_vod_covers`
Expected: compile errors: there is no module `vod::covers` (and the JPEG encoder of the tests' pictures is missing).

- [ ] **Step 3: JPEG, and the store**

In `rust/Cargo.toml`, the `image` line and its comment. Replace

```toml
# PNG logos in the guide. eframe pulls in this very version with these very
# features already, so nothing new is compiled in.
image = { version = "0.24", default-features = false, features = ["png"] }
```

with

```toml
# PNG logos in the guide; PNG and JPEG covers in the thuisbioscoop. eframe
# pulls in this very version with PNG already; JPEG adds its decoder
# (jpeg-decoder), no crate of our own.
image = { version = "0.24", default-features = false, features = ["png", "jpeg"] }
```

In `rust/src/vod/mod.rs`, declare the cover store: before the line `pub mod details;`, add:

```rust
pub mod covers;
```

Create `rust/src/vod/covers.rs`:

```rust
//! Covers, backdrops and stills (spec 2026-10-15 §4.4). Only what a page asks
//! for in a frame is fetched: what is on screen and what comes next. A cover is
//! read from its file when it is kept, else downloaded (6 at a time); one that
//! a frame no longer asks for before it comes is dropped. What came is kept as
//! the bytes it was, in `<16 hex of sha256(address)>.jpg` or `.png`, decoded
//! on tokio's blocking pool into one of a few fixed sizes, and kept as a
//! texture: 96 MB of them at most, the one shown longest ago going first. The
//! folder is kept to 500 MB by `prune`, the oldest files first.
//!
//! An address may hold the portal's account: it is never logged (only its
//! host), never a file's name and never a texture's (a hash). A title without
//! a cover shows its name on a colour of its own (`placeholder_tint`).

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Downloads at the same time.
pub const AT_ONCE: usize = 6;
/// Covers read from their files and decoded at the same time: beside the
/// downloads, so a slow server never holds up a kept cover.
pub const READS_AT_ONCE: usize = 6;
/// The textures' bytes together (width × height × 4).
pub const TEXTURE_BYTES: usize = 96 * 1024 * 1024;
/// The covers folder's bytes, kept by `prune`.
pub const DISK_BYTES: u64 = 500 * 1024 * 1024;
/// Textures made per frame: a screenful read from the disk spreads over a few
/// frames instead of making one long one.
pub const UPLOADS_PER_FRAME: usize = 8;
/// A body bigger than this is no cover.
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// A canvas wider or taller than this is no cover.
const MAX_CANVAS: u32 = 8192;

/// What a picture is drawn as, and so the sizes it is made in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    /// A standing cover, 2:3.
    Poster,
    /// A lying tile or an episode's still, 16:9.
    Landscape,
    /// A page's background, 16:9.
    Backdrop,
}

impl Shape {
    /// The sizes it is made in, smallest first (spec §4.4's table).
    pub fn sizes(self) -> &'static [[u32; 2]] {
        match self {
            Shape::Poster => &[[200, 300], [300, 450], [400, 600]],
            Shape::Landscape => &[[320, 180], [480, 270], [640, 360]],
            Shape::Backdrop => &[[1280, 720], [1920, 1080]],
        }
    }

    /// The first size at least as big as a tile of `shown` pixels (physical
    /// pixels: points times the screen's scale), the largest for a bigger one.
    /// A window of another size so finds its covers made already.
    pub fn size_for(self, shown: [f32; 2]) -> [u32; 2] {
        let sizes = self.sizes();
        sizes
            .iter()
            .copied()
            .find(|[w, h]| *w as f32 >= shown[0] && *h as f32 >= shown[1])
            .unwrap_or(sizes[sizes.len() - 1])
    }
}

/// A cover at one size: the hash of its address (its file's name too) and the
/// size it is made in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoverKey {
    pub hash: u64,
    pub size: [u32; 2],
}

/// The first 8 bytes of the SHA-256 of `url`.
pub fn url_hash(url: &str) -> u64 {
    let digest = ring::digest::digest(&ring::digest::SHA256, url.as_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest.as_ref()[..8]);
    u64::from_be_bytes(first)
}

/// The name a cover's file has before its extension: 16 hex digits of the
/// SHA-256 of its address.
pub fn file_stem(url: &str) -> String {
    format!("{:016x}", url_hash(url))
}

/// The colour of a title without a cover: its hue from the first bytes of the
/// SHA-256 of its name, so a title always has the same one; muted and dark
/// enough for white letters.
pub fn placeholder_tint(name: &str) -> egui::Color32 {
    let digest = ring::digest::digest(&ring::digest::SHA256, name.as_bytes());
    let b = digest.as_ref();
    let hue = f32::from(u16::from_be_bytes([b[0], b[1]])) / 65_536.0;
    hsv(hue, 0.45, 0.35)
}

/// A colour by its hue (0 to 1), saturation and value, as the screen shows it.
fn hsv(h: f32, s: f32, v: f32) -> egui::Color32 {
    let sector = (h * 6.0).floor();
    let f = h * 6.0 - sector;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match sector as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let byte = |x: f32| (x * 255.0).round() as u8;
    egui::Color32::from_rgb(byte(r), byte(g), byte(b))
}

/// A texture as the covers keep it: its size in bytes is what it costs.
pub trait CoverTexture {
    fn byte_size(&self) -> usize;
}

impl CoverTexture for egui::TextureHandle {
    fn byte_size(&self) -> usize {
        let [w, h] = self.size();
        w * h * 4
    }
}

/// What makes a texture of a decoded cover: egui's context in the app, a fake
/// in the tests, which have no GPU.
pub trait Uploader {
    type Texture: CoverTexture;
    /// A texture of `image`, called `name` (a hash, never an address).
    fn upload(&self, name: String, image: egui::ColorImage) -> Self::Texture;
}

impl Uploader for egui::Context {
    type Texture = egui::TextureHandle;

    fn upload(&self, name: String, image: egui::ColorImage) -> egui::TextureHandle {
        self.load_texture(name, image, egui::TextureOptions::LINEAR)
    }
}

/// The textures of the covers shown, `cap` bytes together at most: the one
/// shown longest ago goes first.
pub struct TextureLru<T> {
    cap: usize,
    used: usize,
    clock: u64,
    entries: HashMap<CoverKey, (T, u64)>,
}

impl<T: CoverTexture> TextureLru<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            used: 0,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    /// Marks `key`'s texture as shown now; whether there is one.
    pub fn touch(&mut self, key: &CoverKey) -> bool {
        self.clock += 1;
        match self.entries.get_mut(key) {
            Some((_, shown)) => {
                *shown = self.clock;
                true
            }
            None => false,
        }
    }

    /// `key`'s texture, as it is.
    pub fn peek(&self, key: &CoverKey) -> Option<&T> {
        self.entries.get(key).map(|(t, _)| t)
    }

    /// Keeps `texture` as shown now. Those shown longest ago go until it fits
    /// (one bigger than the cap alone stays alone).
    pub fn insert(&mut self, key: CoverKey, texture: T) {
        self.clock += 1;
        let bytes = texture.byte_size();
        if let Some((old, _)) = self.entries.insert(key, (texture, self.clock)) {
            self.used -= old.byte_size();
        }
        self.used += bytes;
        while self.used > self.cap && self.entries.len() > 1 {
            let oldest = self
                .entries
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, (_, shown))| *shown)
                .map(|(k, _)| *k);
            let Some(oldest) = oldest else { break };
            if let Some((gone, _)) = self.entries.remove(&oldest) {
                self.used -= gone.byte_size();
            }
        }
    }

    /// The bytes kept.
    pub fn used(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

enum Slot {
    /// On its way; dropped when a frame no longer asks for it.
    Loading(tokio::task::AbortHandle),
    /// No cover at this address: not asked again this session.
    Missing,
}

/// What a cover's task came back with.
struct Arrived {
    key: CoverKey,
    image: Option<egui::ColorImage>,
}

/// The covers of the pages that show.
pub struct CoverStore<U: Uploader> {
    dir: PathBuf,
    uploader: U,
    /// Built when the first cover is asked for: a session that never opens
    /// the thuisbioscoop pays nothing for it.
    client: Option<reqwest::Client>,
    client_failed: bool,
    textures: TextureLru<U::Texture>,
    slots: HashMap<CoverKey, Slot>,
    /// Asked for in this frame.
    wanted: HashSet<CoverKey>,
    downloads: Arc<Semaphore>,
    reads: Arc<Semaphore>,
    done: Arc<Mutex<Vec<Arrived>>>,
    /// Decoded, waiting for a frame with room to make its texture.
    decoded: Vec<Arrived>,
}

impl<U: Uploader> CoverStore<U> {
    /// Covers kept as files in `dir`, made into textures by `uploader`.
    pub fn new(dir: PathBuf, uploader: U) -> Self {
        Self {
            dir,
            uploader,
            client: None,
            client_failed: false,
            textures: TextureLru::new(TEXTURE_BYTES),
            slots: HashMap::new(),
            wanted: HashSet::new(),
            downloads: Arc::new(Semaphore::new(AT_ONCE)),
            reads: Arc::new(Semaphore::new(READS_AT_ONCE)),
            done: Arc::new(Mutex::new(Vec::new())),
            decoded: Vec::new(),
        }
    }

    /// The texture of the cover at `url`, made for a tile of `shown` pixels,
    /// once it is there; asked for when it is not. None while it comes and for
    /// no cover: the tile shows its placeholder, of the same size.
    pub fn cover(&mut self, url: &str, shape: Shape, shown: [f32; 2]) -> Option<&U::Texture> {
        let key = CoverKey {
            hash: url_hash(url),
            size: shape.size_for(shown),
        };
        self.wanted.insert(key);
        if self.textures.touch(&key) {
            return self.textures.peek(&key);
        }
        if !self.slots.contains_key(&key) {
            self.start(key, url);
        }
        None
    }

    /// Whether the cover at `url` for a tile of `shown` pixels is on its way.
    pub fn is_loading(&self, url: &str, shape: Shape, shown: [f32; 2]) -> bool {
        let key = CoverKey {
            hash: url_hash(url),
            size: shape.size_for(shown),
        };
        matches!(self.slots.get(&key), Some(Slot::Loading(_)))
    }

    /// Downloads on their way now.
    pub fn downloads_in_flight(&self) -> usize {
        AT_ONCE - self.downloads.available_permits()
    }

    /// The textures' bytes.
    pub fn texture_bytes(&self) -> usize {
        self.textures.used()
    }

    /// After a frame has asked for its covers: what it did not ask for and is
    /// still on its way is dropped, and what came is made into textures, at
    /// most [`UPLOADS_PER_FRAME`].
    pub fn end_frame(&mut self) {
        let wanted = std::mem::take(&mut self.wanted);
        self.slots.retain(|key, slot| match slot {
            Slot::Loading(task) if !wanted.contains(key) => {
                task.abort();
                false
            }
            _ => true,
        });
        self.decoded.extend(std::mem::take(&mut *self.done.lock()));
        let mut made = 0;
        let mut later = Vec::new();
        for arrived in std::mem::take(&mut self.decoded) {
            // Dropped while it came: of no use any more.
            if !matches!(self.slots.get(&arrived.key), Some(Slot::Loading(_))) {
                continue;
            }
            match arrived.image {
                None => {
                    self.slots.insert(arrived.key, Slot::Missing);
                }
                Some(_) if made >= UPLOADS_PER_FRAME => later.push(arrived),
                Some(image) => {
                    let [w, h] = arrived.key.size;
                    let name = format!("cover {:016x} {w}x{h}", arrived.key.hash);
                    let texture = self.uploader.upload(name, image);
                    self.textures.insert(arrived.key, texture);
                    self.slots.remove(&arrived.key);
                    made += 1;
                }
            }
        }
        self.decoded = later;
    }

    /// Reads or fetches the cover in the background.
    fn start(&mut self, key: CoverKey, url: &str) {
        let Some(client) = self.http_client() else {
            self.slots.insert(key, Slot::Missing);
            return;
        };
        let (url, dir, done) = (url.to_owned(), self.dir.clone(), self.done.clone());
        let (downloads, reads) = (self.downloads.clone(), self.reads.clone());
        let task = tokio::spawn(async move {
            let image = load_cover(&client, &url, &dir, key, &downloads, &reads).await;
            done.lock().push(Arrived { key, image });
        });
        self.slots.insert(key, Slot::Loading(task.abort_handle()));
    }

    /// The HTTP client, built on first use; none when it cannot be built
    /// (warned once): the placeholders show.
    fn http_client(&mut self) -> Option<reqwest::Client> {
        if self.client.is_none() && !self.client_failed {
            // A cover address may hold a token: no more than 3 redirects, and
            // none hands it on as `Referer`.
            let built = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent(crate::APP_ID)
                .redirect(reqwest::redirect::Policy::limited(3))
                .referer(false)
                .build();
            match built {
                Ok(client) => self.client = Some(client),
                Err(e) => {
                    self.client_failed = true;
                    tracing::warn!("cover: no HTTP client ({e}) - the placeholders show");
                }
            }
        }
        self.client.clone()
    }
}

/// The cover from its file when that decodes, else from the web (kept as a
/// file only once it decodes), made in `key.size`. None for no cover.
async fn load_cover(
    client: &reqwest::Client,
    url: &str,
    dir: &Path,
    key: CoverKey,
    downloads: &Semaphore,
    reads: &Semaphore,
) -> Option<egui::ColorImage> {
    let stem = format!("{:016x}", key.hash);
    {
        let _place = reads.acquire().await.ok()?;
        let (dir, stem) = (dir.to_path_buf(), stem.clone());
        let kept = tokio::task::spawn_blocking(move || read_kept(&dir, &stem, key.size))
            .await
            .ok()
            .flatten();
        if kept.is_some() {
            return kept;
        }
    }
    let host = crate::guide::logos::host_of(url).to_owned();
    let bytes = {
        let _place = downloads.acquire().await.ok()?;
        download(client, url).await
    };
    let Some(bytes) = bytes else {
        tracing::debug!("cover: {host} could not be fetched - the placeholder shows");
        return None;
    };
    let dir = dir.to_path_buf();
    let image = tokio::task::spawn_blocking(move || keep(&bytes, &dir, &stem, key.size))
        .await
        .ok()
        .flatten();
    if image.is_none() {
        tracing::debug!("cover: {host} is no JPEG or PNG - the placeholder shows");
    }
    image
}

/// The cover in a kept file (`<stem>.jpg` or `<stem>.png`), made in `size`;
/// none when there is none or it does not decode.
fn read_kept(dir: &Path, stem: &str, size: [u32; 2]) -> Option<egui::ColorImage> {
    ["jpg", "png"].iter().find_map(|ext| {
        let bytes = std::fs::read(dir.join(format!("{stem}.{ext}"))).ok()?;
        decode(&bytes, size)
    })
}

/// Decodes a downloaded body and keeps it as a file, but only when it decodes:
/// a kept file is trusted next time. A file that cannot be written costs the
/// next session a download, not this cover.
fn keep(bytes: &[u8], dir: &Path, stem: &str, size: [u32; 2]) -> Option<egui::ColorImage> {
    let (_, ext) = format_of(bytes)?;
    let image = decode(bytes, size)?;
    let _ = std::fs::create_dir_all(dir);
    if let Err(e) = std::fs::write(dir.join(format!("{stem}.{ext}")), bytes) {
        tracing::debug!("cover: could not be kept as a file ({e})");
    }
    Some(image)
}

/// JPEG or PNG, by its first bytes; none for anything else.
fn format_of(bytes: &[u8]) -> Option<(image::ImageFormat, &'static str)> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some((image::ImageFormat::Jpeg, "jpg"))
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some((image::ImageFormat::Png, "png"))
    } else {
        None
    }
}

/// A JPEG or PNG made to fill `size` exactly (cut to its shape at the middle);
/// none for anything else, and for a canvas over `MAX_CANVAS` either way.
pub fn decode(bytes: &[u8], size: [u32; 2]) -> Option<egui::ColorImage> {
    let (format, _) = format_of(bytes)?;
    let mut reader = image::io::Reader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(MAX_CANVAS);
    limits.max_image_height = Some(MAX_CANVAS);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let filled = image.resize_to_fill(size[0], size[1], image::imageops::FilterType::CatmullRom);
    let rgba = filled.into_rgba8();
    let dims = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        dims,
        rgba.as_raw(),
    ))
}

/// `url`'s body, at most [`MAX_BYTES`]; none on an error status, a failure,
/// or more.
async fn download(client: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let mut response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_BYTES {
            return None;
        }
    }
    Some(bytes)
}

/// Removes the oldest files of `dir` (by when they were written) until it
/// holds `cap` bytes at most; how many went and the bytes left. A missing
/// folder holds nothing.
pub fn prune(dir: &Path, cap: u64) -> std::io::Result<(usize, u64)> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(e) => return Err(e),
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort_by_key(|(written, _, _)| *written);
    let mut removed = 0;
    for (_, len, path) in files {
        if total <= cap {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
            removed += 1;
        }
    }
    Ok((removed, total))
}

/// [`prune`] of `dir` to [`DISK_BYTES`] on the blocking pool, with a line in
/// the log when it removed any: the app runs it once at a start.
pub fn prune_in_background(dir: PathBuf) {
    tokio::task::spawn_blocking(move || match prune(&dir, DISK_BYTES) {
        Ok((0, _)) => {}
        Ok((n, left)) => tracing::info!(
            "covers: {n} oldest files removed, {} MB left",
            left / (1024 * 1024)
        ),
        Err(e) => tracing::warn!("covers: folder not pruned ({e})"),
    });
}
```

The first build adds `jpeg-decoder` 0.3.2 to `Cargo.lock` (and nothing else: `git diff rust/Cargo.lock` shows that one package).

- [ ] **Step 4: The folder kept to its 500 MB at every start**

In `rust/src/main.rs`, in `main`, after the runtime is entered. After

```rust
    let rt = Runtime::new()?;
    let _rt_guard = rt.enter();
```

add:

```rust
    // The thuisbioscoop's covers folder keeps to its 500 MB (spec 2026-10-15
    // §4.4): the oldest files go, in the background.
    bongo_player::vod::covers::prune_in_background(storage.cache_dir().join("covers"));
```

A folder that is not there yet (nothing writes covers before phase 2) costs one failed `read_dir` on a blocking thread, and no line in the log.

- [ ] **Step 5: Run the tests to see them pass**

Run: `ct --test test_vod_covers`
Expected: all pass (11).

- [ ] **Step 6: Format, lint, full suite**

- Run `fmt src/vod/mod.rs src/main.rs tests/test_vod_covers.rs`. Expected: no output.
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 7: Commit**

Message file (in the scratchpad):
```
feat(rust): the cover store: what a frame asks for, 6 downloads at a time, dropped off screen, decoded on the blocking pool into fixed sizes, a 96 MB texture LRU, a 500 MB folder; image gains jpeg

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/vod/covers.rs rust/src/vod/mod.rs rust/src/main.rs rust/Cargo.toml rust/Cargo.lock rust/tests/test_vod_covers.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/vod/covers.rs rust/src/vod/mod.rs rust/src/main.rs rust/Cargo.toml rust/Cargo.lock rust/tests/test_vod_covers.rs`

---

### Task 10: The player: the VOD load and its reports

The player's side of the thuisbioscoop (spec §7, Rulings 44–47): a VOD load whose URL goes to mpv byte for byte with `start=` and `audio-file=` as options of that one file and whose log line names its host only; the audio and subtitle tracks chosen by id; mute; and, only while a VOD load plays, its position, length and tracks reported as `Event::Vod`, from mpv's start of its file on (Ruling 47). The app's exhaustive match over the events gains one empty arm.

**Files:**
- Modify: `rust/src/player/mod.rs`, `rust/src/player/events.rs`, `rust/src/player/mpv.rs`, `rust/src/player/mpv_sys.rs`
- Modify: `rust/src/app.rs` (`TvApp::drain_events`)
- Test: `rust/tests/test_player_vod.rs` (new), `rust/tests/test_mpv_smoke.rs` (an ignored smoke test)

**Interfaces:**
- Consumes: `loadfile_command`, `option_value`, `Mpv::{observe_property, set_property, command}`, `events::property_change`, `crate::redact::url_host`.
- Produces:
  ```rust
  // player/mod.rs
  pub enum Cmd { /* … */ LoadVod(VodLoad), /* … */ SetAudio(Option<i64>), SetSubtitles(Option<i64>), SetMute(bool), /* … */ }
  #[cfg_attr(test, derive(Debug))] #[derive(Clone, PartialEq)]
  pub struct VodLoad { pub url: String, pub start: f64, pub audio_file: Option<String> }
  pub enum Event { /* … */ Vod(VodEvent) }
  pub enum VodEvent { Position(f64), Duration(f64), Tracks(Vec<Track>) }
  pub struct Track { pub id: i64, pub kind: TrackKind, pub lang: String, pub title: String, pub selected: bool }
  pub enum TrackKind { Audio, Subtitle }
  pub const OBSERVE_VOD: u64;   // 2
  pub struct SecondGate;        // pass(&mut self, pos: f64) -> bool; reset(&mut self)
  pub fn vod_loadfile_args(load: &VodLoad) -> Vec<String>;
  // player/events.rs
  pub unsafe fn vod_report(evt: *mut mpv_event) -> Option<VodEvent>;
  pub fn parse_tracks(json: &str) -> Vec<Track>;
  // player/mpv.rs
  impl Mpv { pub fn unobserve_property(&self, userdata: u64) -> Result<(), MpvError>; }
  ```
  Log lines: info `loadfile vod from <host>`, `loadfile vod from <host> (start <s> s, audio file)`; debug `aid <id|no>`, `sid <id|no>`, `mute <bool>`; warn `observe <name> failed: <e>`, `unobserve failed: <e>`.

- [ ] **Step 1: Write the failing tests**

Create `rust/tests/test_player_vod.rs`:

```rust
//! The player's side of the thuisbioscoop (player/, spec 2026-10-15 §7): a
//! VOD load with options of that one file and a line without its address,
//! mpv's track list, and the once-a-second position.

use bongo_player::player::events::parse_tracks;
use bongo_player::player::{
    loadfile_command, vod_loadfile_args, Cmd, SecondGate, Track, TrackKind, VodLoad,
};

fn load(url: &str, start: f64, audio_file: Option<&str>) -> VodLoad {
    VodLoad {
        url: url.into(),
        start,
        audio_file: audio_file.map(str::to_owned),
    }
}

#[test]
fn a_film_goes_to_mpv_byte_for_byte_with_its_start_as_an_option_of_that_file() {
    // A password may hold a `|`: the URL goes as it is.
    let url = "http://h:80/movie/u/pa|ss/5001.mkv";
    assert_eq!(
        vod_loadfile_args(&load(url, 4380.0, None)),
        ["loadfile", url, "replace", "-1", "start=4380"]
    );
    assert_eq!(vod_loadfile_args(&load(url, 12.5, None))[4], "start=12.5");
    // From the start: a plain loadfile, as a film from `f` is.
    assert_eq!(vod_loadfile_args(&load(url, 0.0, None)), ["loadfile", url]);
    for odd in [-5.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            vod_loadfile_args(&load(url, odd, None)),
            ["loadfile", url],
            "{odd}"
        );
    }
}

#[test]
fn a_trailer_takes_its_sound_as_an_audio_file_counted_in_bytes() {
    let video = "https://rr1.example/videoplayback?expire=1&sig=a,b";
    let audio = "https://rr2.example/videoplayback?itag=140&sig=c,d";
    let args = vod_loadfile_args(&load(video, 0.0, Some(audio)));
    assert_eq!(
        args,
        [
            "loadfile".to_owned(),
            video.to_owned(),
            "replace".to_owned(),
            "-1".to_owned(),
            format!("audio-file=%{}%{audio}", audio.len()),
        ]
    );
    let both = vod_loadfile_args(&load(video, 30.0, Some(audio)));
    assert!(both[4].starts_with("start=30,audio-file=%"), "{}", both[4]);
}

#[test]
fn the_line_of_a_vod_load_names_its_host_never_its_address() {
    let film = Cmd::LoadVod(load("http://h:80/movie/user/secret/5001.mkv", 4380.0, None));
    let (args, line) = loadfile_command(&film).unwrap();
    assert_eq!(args[1], "http://h:80/movie/user/secret/5001.mkv");
    assert_eq!(line, "loadfile vod from h (start 4380 s)");
    let trailer = Cmd::LoadVod(load(
        "https://rr1.example/videoplayback?token=abc",
        0.0,
        Some("https://rr2.example/a?token=def"),
    ));
    let (_, line) = loadfile_command(&trailer).unwrap();
    assert_eq!(line, "loadfile vod from rr1.example (audio file)");
    let (_, line) = loadfile_command(&Cmd::LoadVod(load(
        "http://h:80/series/u/p/7.mp4",
        0.0,
        None,
    )))
    .unwrap();
    assert_eq!(line, "loadfile vod from h");
    assert!(loadfile_command(&Cmd::SetMute(true)).is_none());
}

#[test]
fn mpvs_track_list_gives_the_audio_and_subtitle_tracks() {
    // As the vendored libmpv gives `track-list` as a string (see the smoke test).
    let json = r#"[
        {"id":1,"type":"video","src-id":0,"selected":true,"codec":"h264"},
        {"id":1,"type":"audio","lang":"nld","title":"Nederlands","selected":true,"default":true},
        {"id":2,"type":"audio","lang":"eng","selected":false},
        {"id":1,"type":"sub","lang":"nld","title":"Nederlands (SDH)","selected":false},
        {"type":"audio"},
        "odd"
    ]"#;
    let track = |id, kind, lang: &str, title: &str, selected| Track {
        id,
        kind,
        lang: lang.into(),
        title: title.into(),
        selected,
    };
    assert_eq!(
        parse_tracks(json),
        [
            track(1, TrackKind::Audio, "nld", "Nederlands", true),
            track(2, TrackKind::Audio, "eng", "", false),
            track(1, TrackKind::Subtitle, "nld", "Nederlands (SDH)", false),
        ]
    );
    assert!(parse_tracks("Video: (1) h264").is_empty());
    assert!(parse_tracks("").is_empty());
}

#[test]
fn a_position_is_news_once_a_second_and_after_a_seek() {
    let mut gate = SecondGate::default();
    assert!(gate.pass(0.04));
    assert!(!gate.pass(0.5));
    assert!(gate.pass(1.0));
    assert!(!gate.pass(1.96));
    assert!(gate.pass(600.2), "a seek");
    assert!(gate.pass(12.0), "and back");
    assert!(!gate.pass(f64::NAN));
    gate.reset();
    assert!(gate.pass(12.3), "a new file");
}
```

In `rust/tests/test_mpv_smoke.rs`, at the end of the file, after `the_defect_check_sends_mpvs_own_user_agent`. After

```rust
    assert_eq!(
        mpv.get_property_string("user-agent").as_deref(),
        Some(bongo_player::stream_check::MPV_USER_AGENT),
        "set stream_check::MPV_USER_AGENT to what mpv says"
    );
}
```

add:

```rust

#[test]
#[ignore]
fn a_vod_load_reports_its_length_position_and_tracks_and_live_tv_reports_none() {
    use bongo_player::player::{spawn, Cmd, Event, TrackKind, VodEvent, VodLoad};
    use std::time::{Duration, Instant};
    let player = spawn(320, 240).expect("player");
    // No sound on the developer's speakers.
    player.cmd_tx.send(Cmd::SetMute(true)).unwrap();
    let vod = VodLoad {
        url: "av://lavfi:testsrc2=size=320x240:rate=25:duration=8".into(),
        start: 1.0,
        audio_file: Some("av://lavfi:sine=frequency=440:duration=8".into()),
    };
    player.cmd_tx.send(Cmd::LoadVod(vod)).unwrap();
    let mut events = player.evt_rx.lock();
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut length, mut first, mut audio) = (None, None, false);
    while Instant::now() < deadline && !(length.is_some() && first.is_some() && audio) {
        match events.try_recv() {
            Ok(Event::Vod(VodEvent::Duration(d))) => length = Some(d),
            Ok(Event::Vod(VodEvent::Position(p))) => {
                first.get_or_insert(p);
            }
            Ok(Event::Vod(VodEvent::Tracks(tracks))) => {
                audio |= tracks.iter().any(|t| t.kind == TrackKind::Audio);
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    // A lavfi source has no length up front: mpv tells it as it reads it.
    assert!(length.is_some_and(|d| d > 0.0), "{length:?}");
    assert!(
        first.is_some_and(|p| p >= 0.9),
        "start=1 is kept: {first:?}"
    );
    assert!(
        audio,
        "the audio file is a track of mpv's track-list, read as JSON"
    );
    // `Cmd::SetAudio(None)` sets aid=no: the track list says no audio plays.
    // (`SetSubtitles` gets no check: lavfi has no subtitle track.)
    while events.try_recv().is_ok() {}
    player.cmd_tx.send(Cmd::SetAudio(None)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut silent = false;
    while Instant::now() < deadline && !silent {
        match events.try_recv() {
            Ok(Event::Vod(VodEvent::Tracks(tracks))) => {
                let audio: Vec<_> = tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Audio)
                    .collect();
                silent = !audio.is_empty() && audio.iter().all(|t| !t.selected);
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    assert!(silent, "Cmd::SetAudio(None) sets aid=no");
    // A VOD load after another reports its own position, never the first
    // file's. It goes right after a position of the first, so no other comes
    // in before the player takes it.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match events.try_recv() {
            Ok(Event::Vod(VodEvent::Position(_))) => break,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    while events.try_recv().is_ok() {}
    let second = VodLoad {
        url: "av://lavfi:testsrc2=size=320x240:rate=25:duration=8".into(),
        start: 0.0,
        audio_file: None,
    };
    player.cmd_tx.send(Cmd::LoadVod(second)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut first = None;
    while Instant::now() < deadline && first.is_none() {
        match events.try_recv() {
            Ok(Event::Vod(VodEvent::Position(p))) => first = Some(p),
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    assert!(
        first.is_some_and(|p| p < 1.0),
        "the second load's own position: {first:?}"
    );
    // Live TV reports none of it.
    let live = "av://lavfi:smptebars=size=320x240:rate=25:duration=3";
    player.cmd_tx.send(Cmd::LoadUrl(live.into())).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    while events.try_recv().is_ok() {}
    let quiet = Instant::now() + Duration::from_secs(2);
    while Instant::now() < quiet {
        if let Ok(Event::Vod(report)) = events.try_recv() {
            panic!("live TV reported {report:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    player.cmd_tx.send(Cmd::Quit).unwrap();
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `ct --test test_player_vod`
Expected: compile errors: there is no `VodLoad`, `vod_loadfile_args`, `SecondGate` or `events::parse_tracks`.

- [ ] **Step 3: The FFI and the reader**

In `rust/src/player/mpv_sys.rs`, after `mpv_observe_property` in the `extern` block. After

```rust
    pub fn mpv_observe_property(
        ctx: *mut mpv_handle,
        reply_userdata: u64,
        name: *const c_char,
        format: c_int,
    ) -> c_int;
```

add:

```rust
    pub fn mpv_unobserve_property(ctx: *mut mpv_handle, registered_reply_userdata: u64) -> c_int;
```

In `rust/src/player/mpv.rs`, in `impl Mpv`, after `observe_property`. After

```rust
        check(unsafe { sys::mpv_observe_property(self.handle, userdata, n.as_ptr(), format) })
    }
```

add:

```rust

    /// Stops observing every property observed with `userdata`.
    pub fn unobserve_property(&self, userdata: u64) -> Result<(), MpvError> {
        check(unsafe { sys::mpv_unobserve_property(self.handle, userdata) })
    }
```

In `rust/src/player/events.rs`, the imports. Replace

```rust
use super::Event;
```

with

```rust
use super::{Event, Track, TrackKind, VodEvent};
```

In `rust/src/player/events.rs`, before `#[cfg(test)] mod tests`, after `property_change`. Before

```rust
#[cfg(test)]
mod tests {
    use super::reason_label;
```

add:

```rust
/// What a property change of a VOD load says (see `player::OBSERVE_VOD`):
/// `time-pos` and `duration` as doubles, `track-list` as mpv's JSON. None for
/// another property, and for one that has no value now (nothing plays).
///
/// # Safety
/// Same constraints as [`from_mpv`]. Callers should only pass events whose
/// `event_id == MPV_EVENT_PROPERTY_CHANGE`.
pub unsafe fn vod_report(evt: *mut sys::mpv_event) -> Option<VodEvent> {
    let data = (*evt).data as *mut sys::mpv_event_property;
    if data.is_null() || (*data).data.is_null() {
        return None;
    }
    let name = CStr::from_ptr((*data).name).to_str().ok()?;
    match ((*data).format, name) {
        (sys::MPV_FORMAT_DOUBLE, "time-pos") => {
            Some(VodEvent::Position(*((*data).data as *const f64)))
        }
        (sys::MPV_FORMAT_DOUBLE, "duration") => {
            Some(VodEvent::Duration(*((*data).data as *const f64)))
        }
        (sys::MPV_FORMAT_STRING, "track-list") => {
            let p = *((*data).data as *const *const std::os::raw::c_char);
            if p.is_null() {
                return None;
            }
            Some(VodEvent::Tracks(parse_tracks(
                &CStr::from_ptr(p).to_string_lossy(),
            )))
        }
        _ => None,
    }
}

/// The audio and subtitle tracks of mpv's `track-list`, read as a string: a
/// JSON list of objects with `id`, `type`, `lang`, `title` and `selected`.
/// Video tracks, and anything that is no such object, are left out.
pub fn parse_tracks(json: &str) -> Vec<Track> {
    let Ok(serde_json::Value::Array(list)) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let kind = match t.get("type")?.as_str()? {
                "audio" => TrackKind::Audio,
                "sub" => TrackKind::Subtitle,
                _ => return None,
            };
            let text = |key: &str| t.get(key).and_then(|v| v.as_str()).unwrap_or("").to_owned();
            Some(Track {
                id: t.get("id")?.as_i64()?,
                kind,
                lang: text("lang"),
                title: text("title"),
                selected: t.get("selected").and_then(|v| v.as_bool()).unwrap_or(false),
            })
        })
        .collect()
}

```

- [ ] **Step 4: The commands, the reports and the thread**

In `rust/src/player/mod.rs`, `enum Cmd` gains four commands and `enum Event` one report; after `Event` add the VOD types, the observing id and the second gate (replace from `LoadWithHeaders`'s doc to the end of `enum Event`). Replace

```rust
    /// Load this URL of an M3U list's stream, which may carry its headers:
    /// `url|User-Agent=…&Referer=…` (`loadfile_args`).
    LoadWithHeaders(String),
    Pause(bool),
    /// Stop playing; nothing is loaded afterwards.
    Stop,
    SeekRelative(f64),
    CycleAudio,
    CycleSubtitle,
    SetWindowSize(u32, u32),
    Quit,
}

#[derive(Debug, Clone)]
pub enum Event {
    FileLoaded,
    PlaybackStarted,
    EndOfFile { reason: String },
    Error { msg: String },
    PropertyChanged { name: String, value: String },
}

```

with

```rust
    /// Load this URL of an M3U list's stream, which may carry its headers:
    /// `url|User-Agent=…&Referer=…` (`loadfile_args`).
    LoadWithHeaders(String),
    /// Load a film, an episode or a trailer of the thuisbioscoop (spec
    /// 2026-10-15 §7): its URL byte for byte, with options of that one file.
    /// While it plays the player reports it (`Event::Vod`).
    LoadVod(VodLoad),
    Pause(bool),
    /// Stop playing; nothing is loaded afterwards.
    Stop,
    SeekRelative(f64),
    CycleAudio,
    CycleSubtitle,
    /// Plays audio track `id` (mpv's `aid`, an id of `Event::Vod`'s tracks);
    /// none: no sound track.
    SetAudio(Option<i64>),
    /// Shows subtitle track `id` (mpv's `sid`); none: no subtitles.
    SetSubtitles(Option<i64>),
    /// Mutes or unmutes (mpv's `mute`).
    SetMute(bool),
    SetWindowSize(u32, u32),
    Quit,
}

/// A load of the thuisbioscoop. `Debug` for the tests only: its URL holds the
/// account.
#[cfg_attr(test, derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct VodLoad {
    /// What plays, as it is.
    pub url: String,
    /// Where it starts, in seconds; 0 from the start.
    pub start: f64,
    /// A separate sound track: a trailer's audio, as yt-dlp gives it.
    pub audio_file: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Event {
    FileLoaded,
    PlaybackStarted,
    EndOfFile { reason: String },
    Error { msg: String },
    PropertyChanged { name: String, value: String },
    Vod(VodEvent),
}

/// What the player reports of a `Cmd::LoadVod` while it plays
/// (`Event::Vod`), and of nothing else.
#[derive(Debug, Clone, PartialEq)]
pub enum VodEvent {
    /// Where it plays, in seconds: when the whole second changes, so once a
    /// second at most while it plays, and at once after a seek.
    Position(f64),
    /// How long it lasts, in seconds, when the whole second changes: a stream
    /// that tells its length as it is read (a TS) tells it once a second.
    Duration(f64),
    /// Its audio and subtitle tracks, as mpv lists them.
    Tracks(Vec<Track>),
}

/// An audio or subtitle track of what plays (mpv's `track-list`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// What `Cmd::SetAudio` and `Cmd::SetSubtitles` take.
    pub id: i64,
    pub kind: TrackKind,
    /// Its language as the file names it ("nld", "eng"); empty when none.
    pub lang: String,
    /// Its title as the file names it; empty when none.
    pub title: String,
    /// It plays or shows now.
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Subtitle,
}

/// The `reply_userdata` the properties of a VOD load are observed with:
/// `time-pos`, `duration` and `track-list`, all unobserved at once by it. 1 is
/// `paused-for-cache`'s.
pub const OBSERVE_VOD: u64 = 2;

/// Lets a position through when its whole second differs from the last one's.
#[derive(Debug, Default)]
pub struct SecondGate {
    last: Option<i64>,
}

impl SecondGate {
    /// Whether `pos` (seconds) is news.
    pub fn pass(&mut self, pos: f64) -> bool {
        if !pos.is_finite() {
            return false;
        }
        let second = pos.floor() as i64;
        let news = self.last != Some(second);
        self.last = Some(second);
        news
    }

    /// A new file: its first position is news.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

```

In `rust/src/player/mod.rs`, in `spawn`'s thread, after `info!("mpv player thread started");`, and the load arm takes the VOD load. Replace

```rust
            info!("mpv player thread started");

            loop {
                loop {
                    match cmd_rx.try_recv() {
                        Ok(load @ (Cmd::LoadUrl(_) | Cmd::LoadWithHeaders(_))) => {
                            if let Some((args, line)) = loadfile_command(&load) {
```

with

```rust
            info!("mpv player thread started");
            // A VOD load is playing: its properties are observed and reported.
            let mut vod = false;
            // Its file has started (`MPV_EVENT_START_FILE`): what the properties
            // say is its own from then on, not the file before it's.
            let mut vod_started = false;
            let (mut seconds, mut lengths) = (SecondGate::default(), SecondGate::default());

            loop {
                loop {
                    match cmd_rx.try_recv() {
                        Ok(
                            load @ (Cmd::LoadUrl(_) | Cmd::LoadWithHeaders(_) | Cmd::LoadVod(_)),
                        ) => {
                            follow_vod(&mpv, &mut vod, matches!(load, Cmd::LoadVod(_)));
                            vod_started = false;
                            seconds.reset();
                            lengths.reset();
                            if let Some((args, line)) = loadfile_command(&load) {
```

In `rust/src/player/mod.rs`, in the same `match`, the `Stop` arm, and after it the three new commands. Replace

```rust
                        Ok(Cmd::Stop) => {
                            info!("stop");
                            let _ = mpv.command(&["stop"]);
                        }
```

with

```rust
                        Ok(Cmd::Stop) => {
                            info!("stop");
                            follow_vod(&mpv, &mut vod, false);
                            vod_started = false;
                            let _ = mpv.command(&["stop"]);
                        }
                        Ok(Cmd::SetAudio(id)) => {
                            let value = id.map_or_else(|| "no".to_owned(), |id| id.to_string());
                            debug!("aid {}", value);
                            let _ = mpv.set_property("aid", &value);
                        }
                        Ok(Cmd::SetSubtitles(id)) => {
                            let value = id.map_or_else(|| "no".to_owned(), |id| id.to_string());
                            debug!("sid {}", value);
                            let _ = mpv.set_property("sid", &value);
                        }
                        Ok(Cmd::SetMute(on)) => {
                            debug!("mute {}", on);
                            let _ = mpv.set_property("mute", if on { "yes" } else { "no" });
                        }
```

In `rust/src/player/mod.rs`, in the event `match`, before the `MPV_EVENT_FILE_LOADED` arm, an arm for the start of a file: before the line `mpv_sys::MPV_EVENT_FILE_LOADED => {`, add:

```rust
                    mpv_sys::MPV_EVENT_START_FILE => {
                        // What the VOD properties say from here on is this load's.
                        vod_started = true;
                        seconds.reset();
                        lengths.reset();
                    }
```

In `rust/src/player/mod.rs`, at the top of the `MPV_EVENT_END_FILE` arm: after the line `mpv_sys::MPV_EVENT_END_FILE => {`, add:

```rust
                        vod_started = false;
```

In `rust/src/player/mod.rs`, in the event `match`, before the `MPV_EVENT_PROPERTY_CHANGE` arm, add an arm for the VOD properties. Before

```rust
                    mpv_sys::MPV_EVENT_PROPERTY_CHANGE => {
                        if let Some((n, v)) = unsafe { events::property_change(evt) } {
```

add:

```rust
                    mpv_sys::MPV_EVENT_PROPERTY_CHANGE
                        if unsafe { (*evt).reply_userdata } == OBSERVE_VOD =>
                    {
                        // A late change of a load that is no VOD load any more,
                        // or of the file before this one, says nothing.
                        let report = match vod && vod_started {
                            true => unsafe { events::vod_report(evt) },
                            false => None,
                        };
                        let news = match &report {
                            Some(VodEvent::Position(pos)) => seconds.pass(*pos),
                            Some(VodEvent::Duration(length)) => lengths.pass(*length),
                            Some(_) => true,
                            None => false,
                        };
                        if let (true, Some(report)) = (news, report) {
                            let _ = evt_tx.send(Event::Vod(report));
                        }
                    }
```

In `rust/src/player/mod.rs`, `fn loadfile_command`: `follow_vod` before it, its doc's last sentence and the VOD load first. Replace

```rust
/// What the player thread runs for a load, and the line it logs (`None` for a
/// command that is no load). A plain load hands mpv its URL as it is, as 0.6.1
/// did; only a load with headers has them split off (`loadfile_args`). The line
/// names the headers a load sends, never their values, and its address goes
/// through `redact`.
pub fn loadfile_command(cmd: &Cmd) -> Option<(Vec<String>, String)> {
```

with

```rust
/// Starts or stops observing a VOD load's properties when what loads is or is
/// no VOD load: live TV reports nothing of them, so it costs nothing.
fn follow_vod(mpv: &Mpv, vod: &mut bool, now_vod: bool) {
    if now_vod == *vod {
        return;
    }
    *vod = now_vod;
    if now_vod {
        for (name, format) in [
            ("time-pos", mpv_sys::MPV_FORMAT_DOUBLE),
            ("duration", mpv_sys::MPV_FORMAT_DOUBLE),
            ("track-list", mpv_sys::MPV_FORMAT_STRING),
        ] {
            if let Err(e) = mpv.observe_property(name, format, OBSERVE_VOD) {
                warn!("observe {} failed: {}", name, e);
            }
        }
    } else if let Err(e) = mpv.unobserve_property(OBSERVE_VOD) {
        warn!("unobserve failed: {}", e);
    }
}

/// What the player thread runs for a load, and the line it logs (`None` for a
/// command that is no load). A plain load hands mpv its URL as it is, as 0.6.1
/// did; only a load with headers has them split off (`loadfile_args`). The line
/// names the headers a load sends, never their values, and its address goes
/// through `redact`. A VOD load's line names its host and options only: a
/// trailer's address holds tokens `redact` does not know.
pub fn loadfile_command(cmd: &Cmd) -> Option<(Vec<String>, String)> {
    if let Cmd::LoadVod(load) = cmd {
        let mut said = Vec::new();
        if load.start.is_finite() && load.start > 0.0 {
            said.push(format!("start {} s", load.start));
        }
        if load.audio_file.is_some() {
            said.push("audio file".to_owned());
        }
        let host = crate::redact::url_host(&load.url);
        let line = match said.is_empty() {
            true => format!("loadfile vod from {host}"),
            false => format!("loadfile vod from {host} ({})", said.join(", ")),
        };
        return Some((vod_loadfile_args(load), line));
    }
```

In `rust/src/player/mod.rs`, before `fn option_value` (with its doc), add the VOD load's arguments. Before

```rust
/// An option value as mpv's `key=value,…` lists take any text, commas and
```

add:

```rust
/// The `loadfile` command of a VOD load: its URL as it is, byte for byte, and
/// `start` and `audio-file` as options of that one file (mpv puts them back
/// after it). A load from the start without a sound track is a plain loadfile,
/// as a film from `f` is.
pub fn vod_loadfile_args(load: &VodLoad) -> Vec<String> {
    let mut options = Vec::new();
    if load.start.is_finite() && load.start > 0.0 {
        options.push(format!("start={}", load.start));
    }
    if let Some(audio) = &load.audio_file {
        options.push(format!("audio-file={}", option_value(audio)));
    }
    let mut args = vec!["loadfile".to_owned(), load.url.clone()];
    if !options.is_empty() {
        args.extend(["replace".to_owned(), "-1".to_owned(), options.join(",")]);
    }
    args
}

```

- [ ] **Step 5: The app's match**

In `rust/src/app.rs`, in `TvApp::drain_events`, before the arm `Event::PropertyChanged { name, value } => {`. Before

```rust
                Event::PropertyChanged { name, value } => {
                    if name == "paused-for-cache" {
```

add:

```rust
                // What a load of the thuisbioscoop reports: none loads yet.
                Event::Vod(_) => {}
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `ct --test test_player_vod`, then `ct --test test_mpv_smoke -- --ignored`
Expected: all pass (5 in `test_player_vod`; the 5 smoke tests, the new one among them). The smoke tests run the vendored libmpv without a window, the new one muted; it pins what the plan measured: `track-list` comes as JSON, `start=` holds, `Cmd::SetAudio(None)` sets `aid=no`, a VOD load after another reports its own position, and live TV reports nothing. `Cmd::SetSubtitles` gets no check: lavfi has no subtitle track.

- [ ] **Step 7: Format, lint, full suite**

- Run `fmt src/player/events.rs src/player/mpv.rs src/player/mpv_sys.rs tests/test_player_vod.rs tests/test_mpv_smoke.rs`. Expected: no output.
- Run `hunks src/player/mod.rs`. Expected: the count Task 1 Step 1 recorded (`2` at c81d7b0).
- Run `hunks src/app.rs`. Expected: the count Task 1 Step 1 recorded (`7` at c81d7b0).
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 8: Commit**

Message file (in the scratchpad):
```
feat(rust): the player's VOD load: the URL byte for byte with start and audio-file of that one file, aid, sid and mute, and position, length and tracks reported only while one plays

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/player/mod.rs rust/src/player/events.rs rust/src/player/mpv.rs rust/src/player/mpv_sys.rs rust/src/app.rs rust/tests/test_player_vod.rs rust/tests/test_mpv_smoke.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/player/mod.rs rust/src/player/events.rs rust/src/player/mpv.rs rust/src/player/mpv_sys.rs rust/src/app.rs rust/tests/test_player_vod.rs rust/tests/test_mpv_smoke.rs`

---

### Task 11: The baseline: `f`'s line and the timing test

Spec §5's yardstick. `f` logs one line per search it runs, what it cost and how many results, never what was typed: the only change to `f` (Ruling 48). An ignored test, run in release, makes a catalog of Pandora's size, times the index's build as the catalog makes it, from the lists it keeps and what they gave up beside them (Ruling 53), prints what the build holds on the heap while it runs and what it keeps, the index's size beside the covers' and the list records' (Ruling 22), times the parse of the film and series lists as Pandora sends them (Ruling 28), and per keystroke times the index's search with filters and counts beside `f`'s search on the same catalog (Ruling 49). It asserts no speed: the numbers go to the user.

**Files:**
- Modify: `rust/src/app.rs` (`TvApp::paint_search`; a test)
- Test: `rust/src/app.rs` (`mod tests`), `rust/tests/test_vod_timing.rs` (new, ignored)

**Interfaces:**
- Consumes: `CatalogStore::{from_catalog, search}`, `TitleIndex::{build_with, search, stats, heap_bytes, cover_bytes, len}` and `Extras::take` (Task 5), `Query`, `Sort`, `Years`, `Genre`; the tests' `Harness`, `library_catalog`, `f_pressed`, `LogCapture`.
- Produces: the log line (info) `search: <n> results in <ms> ms`; the test `a_catalog_of_pandoras_size_timed` printing `index build: …`, `index build memory: …`, `what the lists give up for it: …`, `index size: …`, `list records: …`, `film list parse: …`, one line per keystroke and `worst keystroke: …`.

- [ ] **Step 1: Write the failing test**

In `rust/src/app.rs`, in `mod tests`, before `fn f_pressed` (with its doc; after Task 6b's test), add the test. Before

```rust
    /// The press of `f` as a keyboard makes it: the key and its letter, together.
    fn f_pressed() -> Vec<egui::Event> {
```

add:

```rust
    #[tokio::test]
    async fn the_search_of_f_logs_what_each_keystroke_cost_and_nothing_that_was_typed() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        let log = LogCapture::start();
        h.frame_with(f_pressed());
        h.type_text("zeb");
        let text = log.text();
        let lines: Vec<&str> = text.lines().filter(|l| l.contains("search: ")).collect();
        assert_eq!(lines.len(), 4, "one for the empty box, one a key: {text}");
        assert!(lines[3].contains("search: 2 results in "), "{}", lines[3]);
        assert!(
            lines
                .iter()
                .all(|l| l.ends_with(" ms") && !l.contains("zeb")),
            "{text}"
        );
    }

```

- [ ] **Step 2: Run it to see it fail**

Run: `ct --lib app::tests::the_search_of_f_logs`
Expected: FAIL: no line says `search: `.

- [ ] **Step 3: The line**

In `rust/src/app.rs`, in `TvApp::paint_search`, where the search runs when the query changed. Replace

```rust
                        if self.search_hits_for.as_ref() != Some(&key) {
                            self.search_hits = self.catalog.search(&self.search_query);
                            self.search_hits_for = Some(key);
```

with

```rust
                        if self.search_hits_for.as_ref() != Some(&key) {
                            let t0 = Instant::now();
                            self.search_hits = self.catalog.search(&self.search_query);
                            // The baseline of the thuisbioscoop's search (spec
                            // 2026-10-15 §5): what a keystroke costs, and
                            // nothing of what was typed.
                            tracing::info!(
                                "search: {} results in {:.1} ms",
                                self.search_hits.len(),
                                t0.elapsed().as_secs_f64() * 1000.0
                            );
                            self.search_hits_for = Some(key);
```

- [ ] **Step 4: Run it to see it pass**

Run: `ct --lib app::tests::the_search_of_f_logs`
Expected: PASS.

- [ ] **Step 5: The timing test**

Create `rust/tests/test_vod_timing.rs`:

```rust
//! The thuisbioscoop's timing test (spec 2026-10-15 §5, "Een tijdtest in
//! release"): a made-up catalog of Pandora's size, 47,188 films and 10,589
//! series beside 13,000 live channels. It times the title index's build as the
//! catalog makes it, from the lists it keeps and what they gave up beside them
//! (`Extras`), prints what the build holds on the heap while it runs and what
//! it keeps, the index's size, the covers' and the list records', times the
//! parse of the film and series lists as Pandora sends them, and per keystroke
//! the index's search with filters and counts next to the search of `f` on the
//! same catalog. It asserts no speed (a busy machine would fail it): the
//! numbers go to the user. `test_f_timing.rs` times `f` alone, before phase 1
//! and after.
//!
//! `cargo test --release --test test_vod_timing -- --ignored --nocapture`

use bongo_player::args::XtreamCreds;
use bongo_player::catalog::CatalogStore;
use bongo_player::portal::xtream::XtreamPortal;
use bongo_player::portal::{Catalog, Categories, Category, LiveChannel, Movie, Portal, Series};
use bongo_player::vod::genres::Genre;
use bongo_player::vod::index::{Extras, Query, Sort, TitleIndex, Years};
use bongo_player::vod::Kind;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Instant;

/// This test's heap, counted while `COUNTING`: what the build of the index
/// holds while it runs, and what it keeps.
struct Counted;

static COUNTING: AtomicBool = AtomicBool::new(false);
static HELD: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

fn count(bytes: isize) {
    if COUNTING.load(Relaxed) {
        let held = HELD.fetch_add(bytes, Relaxed) + bytes;
        PEAK.fetch_max(held, Relaxed);
    }
}

unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            count(layout.size() as isize);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        System.dealloc(p, layout);
        count(-(layout.size() as isize));
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = System.realloc(p, layout, size);
        if !moved.is_null() {
            count(size as isize - layout.size() as isize);
        }
        moved
    }
}

#[global_allocator]
static HEAP: Counted = Counted;

/// What `work` made, the most it held on the heap beyond what was there before
/// it ran, and what it left there, in bytes.
fn heap_of<T>(work: impl FnOnce() -> T) -> (T, isize, isize) {
    HELD.store(0, Relaxed);
    PEAK.store(0, Relaxed);
    COUNTING.store(true, Relaxed);
    let made = work();
    COUNTING.store(false, Relaxed);
    (made, PEAK.load(Relaxed), HELD.load(Relaxed))
}

const FILMS: usize = 47_188;
const SERIES: usize = 10_589;
const LIVE: usize = 13_000;
const MAX_YEAR: u16 = 2027;

/// Words for made-up titles: with the numbers, about as long as a portal's.
const WORDS: [&str; 24] = [
    "the", "last", "night", "of", "dark", "matrix", "love", "story", "dead", "city", "secret",
    "man", "woman", "king", "lost", "house", "blood", "river", "star", "war", "pokémon", "zomer",
    "liefde", "nacht",
];

/// A number from `i`, the same on every run.
fn mix(i: usize) -> usize {
    let mut x = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x ^= x >> 29;
    (x.wrapping_mul(0xbf58_476d_1ce4_e5b9) >> 32) as usize
}

fn title_of(i: usize) -> String {
    let n = 2 + mix(i) % 3;
    let words: Vec<&str> = (0..n)
        .map(|k| WORDS[mix(i * 7 + k) % WORDS.len()])
        .collect();
    let mut t = words.join(" ");
    if let Some(first) = t.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    format!("{t} {}", i % 997)
}

fn cover(i: usize) -> Option<String> {
    (!mix(i).is_multiple_of(10)).then(|| {
        format!(
            "https://image.tmdb.org/t/p/w600_and_h900_bestv2/{:027x}.jpg",
            mix(i)
        )
    })
}

/// Pandora-like films: a prefix, a title, a year and a label; one in ten is
/// another version of the film before it.
fn films() -> Vec<Movie> {
    let prefixes = ["", "NL | ", "EN | ", "4K | "];
    let labels = ["", " 1080p", " 4K", " NL SUBS", " [NL]", " HD"];
    (0..FILMS)
        .map(|i| {
            let base = if i % 10 == 9 { i - 1 } else { i };
            Movie {
                stream_id: 100_000 + i as i64,
                name: format!(
                    "{}{} ({}){}",
                    prefixes[mix(base) % 4],
                    title_of(base),
                    1950 + mix(base) % 77,
                    labels[mix(i + 3) % labels.len()]
                ),
                container_extension: Some(["mkv", "mp4"][i % 2].into()),
                is_adult: i % 97 == 0,
                stream_icon: cover(i),
                rating: (mix(i) % 101) as u8,
                added: 1_700_000_000 + (mix(i) % 90_000_000) as i64,
                category_id: 1 + (mix(i) % 200) as i64,
            }
        })
        .collect()
}

fn shows() -> Vec<Series> {
    let genres = [
        "Drama",
        "Comedy",
        "Crime, Drama",
        "Sci-Fi & Fantasy",
        "Kids",
        "Reality",
        "",
    ];
    (0..SERIES)
        .map(|i| Series {
            series_id: 1 + i as i64,
            name: title_of(FILMS + i),
            is_adult: i % 89 == 0,
            cover: cover(FILMS + i),
            rating: (mix(i + 11) % 101) as u8,
            release_date_camel: Some(format!("{}-05-01", 1990 + mix(i) % 36)),
            genre: Some(genres[mix(i) % genres.len()].into()),
            category_id: 1 + (mix(i) % 100) as i64,
            last_modified: 1_700_000_000 + (mix(i) % 90_000_000) as i64,
            ..Series::default()
        })
        .collect()
}

fn categories() -> Categories {
    let names = [
        "NL | Komedie",
        "EN | Action",
        "NL | Kids",
        "4K Films",
        "Nieuw",
        "EN | Drama",
        "Horror",
        "Documentaire",
    ];
    let cat = |i: usize| Category {
        category_id: 1 + i as i64,
        category_name: format!("{} {}", names[i % names.len()], i),
    };
    Categories {
        vod: (0..200).map(cat).collect(),
        series: (0..100).map(cat).collect(),
    }
}

fn live() -> Vec<LiveChannel> {
    (0..LIVE)
        .map(|i| LiveChannel {
            stream_id: i as i64,
            name: format!("NL | Kanaal {i} HD"),
            category_id: None,
            epg_channel_id: None,
            tv_archive: 0,
            tv_archive_duration: 0,
            is_adult: false,
            stream_icon: None,
        })
        .collect()
}

/// The film list as Pandora sends it (`get_vod_streams`): every value a
/// string, and the fields the app does not keep beside those it does.
fn film_list_body(movies: &[Movie]) -> String {
    let list: Vec<serde_json::Value> = movies
        .iter()
        .enumerate()
        .map(|(i, m)| {
            serde_json::json!({
                "num": i + 1,
                "name": m.name,
                "stream_type": "movie",
                "stream_id": m.stream_id.to_string(),
                "stream_icon": m.stream_icon.clone().unwrap_or_default(),
                "rating": format!("{:.1}", f64::from(m.rating) / 10.0),
                "rating_5based": f64::from(m.rating) / 20.0,
                "added": m.added.to_string(),
                "is_adult": if m.is_adult { "1" } else { "0" },
                "category_id": m.category_id.to_string(),
                "container_extension": m.container_extension.clone().unwrap_or_default(),
                "custom_sid": "",
                "direct_source": ""
            })
        })
        .collect();
    serde_json::to_string(&list).unwrap()
}

/// The series list as Pandora sends it (`get_series`), with the plot, cast and
/// backdrops the app does not keep.
fn series_list_body(series: &[Series]) -> String {
    let list: Vec<serde_json::Value> = series
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let cover = s.cover.clone().unwrap_or_default();
            serde_json::json!({
                "num": i + 1,
                "name": s.name,
                "series_id": s.series_id,
                "cover": cover,
                "plot": "A family's secrets come out when a child goes missing in a small town.",
                "cast": "An Actor, Another Actor, A Third Actor",
                "director": "A Director",
                "genre": s.genre.clone().unwrap_or_default(),
                "releaseDate": s.release_date_camel.clone().unwrap_or_default(),
                "last_modified": s.last_modified.to_string(),
                "rating": format!("{:.1}", f64::from(s.rating) / 10.0),
                "rating_5based": f64::from(s.rating) / 20.0,
                "backdrop_path": [cover],
                "youtube_trailer": "",
                "episode_run_time": "45",
                "category_id": s.category_id.to_string()
            })
        })
        .collect();
    serde_json::to_string(&list).unwrap()
}

/// `work` timed `n` times: the first, best, mean and worst, in ms.
fn time(n: usize, mut work: impl FnMut()) -> (f64, f64, f64, f64) {
    let ms: Vec<f64> = (0..n)
        .map(|_| {
            let t0 = Instant::now();
            work();
            t0.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    let best = ms.iter().copied().fold(f64::MAX, f64::min);
    let worst = ms.iter().copied().fold(0.0, f64::max);
    (ms[0], best, ms.iter().sum::<f64>() / n as f64, worst)
}

#[test]
#[ignore]
fn a_catalog_of_pandoras_size_timed() {
    let (movies, series, categories) = (films(), shows(), categories());
    // The lists as the catalog keeps them, and what they give up for the index
    // beside them, as `catalog::StoredCatalog::new` takes it out.
    let (mut kept_movies, mut kept_series) = (movies.clone(), series.clone());
    let (extras, _, given_up) = heap_of(|| Extras::take(&mut kept_movies, &mut kept_series));
    let build = || {
        TitleIndex::build_with(
            black_box(&kept_movies),
            black_box(&kept_series),
            &extras,
            &categories,
            MAX_YEAR,
        )
    };
    let (first, best, mean, worst) = time(5, || {
        black_box(build());
    });
    let (index, peak, kept) = heap_of(build);
    let s = index.stats();
    println!(
        "index build: {} titles ({} series, {} films from {} versions): first {first:.1} ms, best {best:.1} ms, mean {mean:.1} ms, worst {worst:.1} ms",
        index.len(),
        s.series,
        s.films,
        s.versions
    );
    println!(
        "index build memory: {:.2} MB at its peak, {:.2} MB kept (the index), {:.2} MB passing (its own working lists); no list copied",
        peak as f64 / 1e6,
        kept as f64 / 1e6,
        (peak - kept) as f64 / 1e6
    );
    println!(
        "what the lists give up for it: {:.2} MB of handles (the strings move), held from the catalog's put until its titles are built",
        given_up as f64 / 1e6
    );
    println!(
        "index size: {:.2} MB (budget 5 MB), covers beside it {:.2} MB",
        index.heap_bytes() as f64 / 1e6,
        index.cover_bytes() as f64 / 1e6
    );
    println!(
        "list records: Movie {} B x {FILMS}, Series {} B x {SERIES} = {:.2} MB",
        std::mem::size_of::<Movie>(),
        std::mem::size_of::<Series>(),
        (std::mem::size_of::<Movie>() * FILMS + std::mem::size_of::<Series>() * SERIES) as f64
            / 1e6
    );
    let (film_body, series_body) = (film_list_body(&movies), series_list_body(&series));
    let (_, film_ms, _, _) = time(5, || {
        let parsed: Vec<Movie> = serde_json::from_slice(black_box(film_body.as_bytes())).unwrap();
        black_box(parsed);
    });
    let (_, series_ms, _, _) = time(5, || {
        let parsed: Vec<Series> =
            serde_json::from_slice(black_box(series_body.as_bytes())).unwrap();
        black_box(parsed);
    });
    println!(
        "film list parse: {film_ms:.1} ms, series list parse: {series_ms:.1} ms (on the fetch's worker, as before)"
    );

    let portal: Arc<dyn Portal> = Arc::new(XtreamPortal::new(XtreamCreds {
        username: "u".into(),
        password: "p".into(),
        host: "h".into(),
        port: 80,
    }));
    let store = CatalogStore::from_catalog(
        portal,
        Catalog {
            live: live(),
            movies: movies.clone(),
            series: series.clone(),
        },
    );
    let filters = Query {
        kind: Some(Kind::Film),
        genres: Genre::Comedy.bit() | Genre::Action.bit(),
        years: Years::Since2020.bit() | Years::The2010s.bit(),
        sort: Sort::Rating,
        ..Query::default()
    };
    println!("keystroke      | index: text      | index: + filters | f today");
    let typed = "the matrix pokemon";
    let (mut worst_index, mut worst_f) = (0.0f64, 0.0f64);
    for end in 1..=typed.len() {
        let text = &typed[..end];
        let plain = Query {
            text: text.into(),
            ..Query::default()
        };
        let filtered = Query {
            text: text.into(),
            ..filters.clone()
        };
        let mut n_plain = 0;
        let (_, plain_ms, _, _) = time(5, || {
            n_plain = black_box(index.search(&plain, false)).titles.len()
        });
        let mut n_filtered = 0;
        let (_, filtered_ms, _, _) = time(5, || {
            n_filtered = black_box(index.search(&filtered, false)).titles.len()
        });
        let mut n_f = 0;
        let (_, f_ms, _, _) = time(5, || n_f = black_box(store.search(text)).len());
        worst_index = worst_index.max(plain_ms).max(filtered_ms);
        worst_f = worst_f.max(f_ms);
        println!(
            "{:<14} | {plain_ms:6.2} ms {n_plain:>6} | {filtered_ms:6.2} ms {n_filtered:>6} | {f_ms:6.2} ms {n_f:>3}",
            format!("{text:?}")
        );
    }
    println!("worst keystroke: index {worst_index:.2} ms (budget 16 ms, and no slower than f), f {worst_f:.2} ms");
}
```

Run: `ct --test test_vod_timing`. Expected: it is ignored (1 ignored); it runs in Task 12, in release.

- [ ] **Step 6: Format, lint, full suite**

- Run `fmt tests/test_vod_timing.rs`. Expected: no output.
- Run `hunks src/app.rs`. Expected: the count Task 1 Step 1 recorded (`7` at c81d7b0).
- Run `cc`. Expected: only the 2 old warnings.
- Run `ct`. Expected: everything passes: the baseline plus this task's tests.

- [ ] **Step 7: Commit**

Message file (in the scratchpad):
```
feat(rust): the baseline of the thuisbioscoop's search: f logs what each keystroke cost and nothing that was typed; an ignored release test times the index beside f

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/src/app.rs rust/tests/test_vod_timing.rs && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/src/app.rs rust/tests/test_vod_timing.rs`

---

### Task 11b: The A/B's catalog of Pandora's size

The startup A/B (Task 12 Step 9) runs against the mock portal, which answers a handful of films and no series, in a fresh config folder every run: it would measure none of what phase 1 adds at a start, the index build, the cache write and the cache read after the first frame (spec §5: "met een catalogus van ±58.000 titels"). The mock gains a catalog of Pandora's size (`MOCK_VOD_BIG=1`), and `measure-startup.ps1` a switch that serves it to both exes and keeps the new exe's config folder between its runs (Ruling 50). These edits are written against the scripts as they are at b963236; the zenderlijst's Task 9 and the rename edit them too, so find each place by what it says and keep its meaning (the controller checks these places after the rebase).

**Files:**
- Modify: `rust/e2e/mock_portal.py` (`MOCK_VOD_BIG`), `rust/e2e/measure-startup.ps1` (`-VodBig`), `rust/e2e/README.md`

**Interfaces:**
- Consumes: `run-scenario.ps1`'s `-ConfigDir` and `-MockEnv`.
- Produces: `MOCK_VOD_BIG=1` (the mock answers `get_vod_streams`, `get_series`, `get_vod_categories` and `get_series_categories` with 47,188 films, 10,589 series, 200 and 100 categories, every value a string; made once and kept in the temp folder, `bongo-mock-vod-big-1`, so a mock that starts reads them before it answers); `measure-startup.ps1 -VodBig` (both exes on it; the new exe's config folder `%TEMP%\bongo-measure-vod` kept between its runs and emptied before the first).

- [ ] **Step 1: The mock's catalog**

In `rust/e2e/mock_portal.py`, the docstring, after `MOCK_M3U_BIG`. After

```python
  MOCK_M3U_BIG=N     /m3u/big.m3u: N channels "GROOT nnnnn" in iptv-org's form (logos, groups,
                     an x-tvg-url); the first 18 play like M3U 01..18 (the startup measurement)
```

add:

```python
  MOCK_VOD_BIG=1     a film and series catalog of Pandora's size, as the portal sends it (every
                     value a string): 47,188 films (one in ten another version of the one before),
                     10,589 series, 200 film and 100 series categories. Made once (about half a
                     second) and kept in the temp folder (bongo-mock-vod-big-1), so a mock that
                     starts reads them before it answers. It takes the place of MOCK_ADULT's and
                     MOCK_XXX's films (measure-startup.ps1 -VodBig)
```

In `rust/e2e/mock_portal.py`, the imports. Replace

```python
import sys
import threading
```

with

```python
import sys
import tempfile
import threading
```

In `rust/e2e/mock_portal.py`, before `def news_epg`, the big lists and their maker: before the line `def news_epg():`, add:

```python
WORDS = ["the", "last", "night", "of", "dark", "matrix", "love", "story", "dead", "city", "secret",
         "man", "woman", "king", "lost", "house", "blood", "river", "star", "war", "pokémon", "zomer",
         "liefde", "nacht"]
VOD_BIG_ACTIONS = ["get_vod_streams", "get_series", "get_vod_categories", "get_series_categories"]


def mix(i):
    """A number from i, the same on every run (as tests/test_vod_timing.rs has it)."""
    m64 = (1 << 64) - 1
    x = (i * 0x9E3779B97F4A7C15) & m64
    x ^= x >> 29
    return ((x * 0xBF58476D1CE4E5B9) & m64) >> 32


def big_title(i):
    t = " ".join(WORDS[mix(i * 7 + k) % len(WORDS)] for k in range(2 + mix(i) % 3))
    return f"{t[:1].upper()}{t[1:]} {i % 997}"


def big_cover(m):
    # A cover for nine titles in ten; m comes from mix.
    return f"https://image.tmdb.org/t/p/w600_and_h900_bestv2/{m:027x}.jpg" if m % 10 else ""


def make_vod_big():
    """MOCK_VOD_BIG's lists as JSON bytes, by action."""
    films_n = 47188
    prefixes = ["", "NL | ", "EN | ", "4K | "]
    labels = ["", " 1080p", " 4K", " NL SUBS", " [NL]", " HD"]
    films = []
    for i in range(films_n):
        base = i - 1 if i % 10 == 9 else i
        m, mb = mix(i), mix(base)
        r = m % 101
        name = f"{prefixes[mb % 4]}{big_title(base)} ({1950 + mb % 77}){labels[mix(i + 3) % 6]}"
        films.append({"num": i + 1, "name": name, "stream_type": "movie", "stream_id": str(100000 + i),
                      "stream_icon": big_cover(m), "rating": f"{r / 10:.1f}", "rating_5based": r / 20,
                      "added": str(1700000000 + m % 90000000), "is_adult": "1" if i % 97 == 0 else "0",
                      "category_id": str(1 + m % 200), "container_extension": ("mkv", "mp4")[i % 2],
                      "custom_sid": "", "direct_source": ""})
    genres = ["Drama", "Comedy", "Crime, Drama", "Sci-Fi & Fantasy", "Kids", "Reality", ""]
    series = []
    for i in range(10589):
        m = mix(i)
        r = mix(i + 11) % 101
        c = big_cover(mix(films_n + i))
        series.append({"num": i + 1, "name": big_title(films_n + i), "series_id": 1 + i, "cover": c,
                       "plot": "A family's secrets come out when a child goes missing in a small town.",
                       "cast": "An Actor, Another Actor, A Third Actor", "director": "A Director",
                       "genre": genres[m % 7], "releaseDate": f"{1990 + m % 36}-05-01",
                       "last_modified": str(1700000000 + m % 90000000), "rating": f"{r / 10:.1f}",
                       "rating_5based": r / 20, "backdrop_path": [c] if c else [], "youtube_trailer": "",
                       "episode_run_time": "45", "category_id": str(1 + m % 100),
                       "is_adult": "1" if i % 89 == 0 else "0"})
    names = ["NL | Komedie", "EN | Action", "NL | Kids", "4K Films", "Nieuw", "EN | Drama", "Horror",
             "Documentaire"]

    def categories(n):
        return [{"category_id": str(1 + i), "category_name": f"{names[i % 8]} {i}", "parent_id": 0}
                for i in range(n)]

    lists = [films, series, categories(200), categories(100)]
    return {a: json.dumps(l, separators=(",", ":")).encode() for a, l in zip(VOD_BIG_ACTIONS, lists)}


def vod_big():
    """MOCK_VOD_BIG's lists, read from the temp folder; made and kept there the first
    time, so no later start of the mock pays for them."""
    folder = os.path.join(tempfile.gettempdir(), "bongo-mock-vod-big-1")
    try:
        lists = {}
        for a in VOD_BIG_ACTIONS:
            with open(os.path.join(folder, a + ".json"), "rb") as f:
                lists[a] = f.read()
        return lists
    except OSError:
        pass
    lists = make_vod_big()
    os.makedirs(folder, exist_ok=True)
    for a, body in lists.items():
        with open(os.path.join(folder, a + ".json.tmp"), "wb") as f:
            f.write(body)
        os.replace(os.path.join(folder, a + ".json.tmp"), os.path.join(folder, a + ".json"))
    return lists


VOD_BIG = vod_big() if ENV("MOCK_VOD_BIG") == "1" else {}


```

In `rust/e2e/mock_portal.py`, in `do_GET`, the big lists before the live channels' list. Before

```python
            if action == "get_live_streams":
                return self.json(live_streams())
```

add:

```python
            if action in VOD_BIG:
                return self.reply(200, VOD_BIG[action], "application/json")
```

- [ ] **Step 2: The measurement's switch**

In `rust/e2e/measure-startup.ps1`, the comment at the top, after the `-M3u` paragraph. After

```powershell
# starts about 0.4 s ahead for that reason alone: report both numbers as they
# come, and read the -M3u comparison with that in mind.
```

add:

```powershell
#
# -VodBig serves both exes a film and series catalog of Pandora's size
# (MOCK_VOD_BIG: 47,188 films, 10,589 series and their categories), so the new
# exe builds its title index at every start, as on the real portal. Its config
# folder is kept between its runs, as -M3u keeps it (and emptied before the
# first), so from the second run on it also reads its index cache after the
# first frame and writes it anew; the baseline's is fresh every run.
```

In `rust/e2e/measure-startup.ps1`, the parameters. Replace

```powershell
    [string]$Baseline = '',
    [switch]$M3u
)
```

with

```powershell
    [string]$Baseline = '',
    [switch]$M3u,
    [switch]$VodBig
)
```

In `rust/e2e/measure-startup.ps1`, `Measure-Run`: the big catalog and the kept folder. Replace

```powershell
# One run of the exe; the seconds from "starting" to the first picture, or nothing.
function Measure-Run([string]$ExePath, [bool]$AsM3u) {
```

with

```powershell
# One run of the exe; the seconds from "starting" to the first picture, or nothing.
# $Keep: in the -VodBig folder that is kept between runs.
function Measure-Run([string]$ExePath, [bool]$AsM3u, [bool]$Keep) {
```

In `rust/e2e/measure-startup.ps1`, in `Measure-Run`, the Xtream run. Replace

```powershell
    } else {
        $out = & (Join-Path $PSScriptRoot 'run-scenario.ps1') @common -Resume 3 -MockEnv @{ MOCK_LATENCY = '1' }
    }
```

with

```powershell
    } else {
        $mockEnv = @{ MOCK_LATENCY = '1' }
        if ($VodBig) { $mockEnv.MOCK_VOD_BIG = '1' }
        $kept = @{}
        if ($Keep) { $kept.ConfigDir = $vodDir }
        $out = & (Join-Path $PSScriptRoot 'run-scenario.ps1') @common @kept -Resume 3 -MockEnv $mockEnv
    }
```

In `rust/e2e/measure-startup.ps1`, the targets and their runs. Replace

```powershell
$targets = @(@{ Label = ''; Exe = $Exe; M3u = [bool]$M3u; Times = @() })
if ($Baseline) {
    if (-not (Test-Path $Baseline)) { throw "baseline exe not found: $Baseline" }
    $targets = @(@{ Label = 'baseline'; Exe = $Baseline; M3u = $false; Times = @() },
                 @{ Label = 'new'; Exe = $Exe; M3u = [bool]$M3u; Times = @() })
}
```

with

```powershell
if ($M3u -and $VodBig) { throw '-M3u and -VodBig measure the new exe on different sources: run them apart' }
$vodDir = Join-Path $env:TEMP 'bongo-measure-vod'
if ($VodBig) { Remove-Item -Recurse -Force $vodDir -ErrorAction SilentlyContinue }
$targets = @(@{ Label = ''; Exe = $Exe; M3u = [bool]$M3u; Keep = [bool]$VodBig; Times = @() })
if ($Baseline) {
    if (-not (Test-Path $Baseline)) { throw "baseline exe not found: $Baseline" }
    $targets = @(@{ Label = 'baseline'; Exe = $Baseline; M3u = $false; Keep = $false; Times = @() },
                 @{ Label = 'new'; Exe = $Exe; M3u = [bool]$M3u; Keep = [bool]$VodBig; Times = @() })
}
```

In `rust/e2e/measure-startup.ps1`, in the loop over the runs. Replace

```powershell
        $secs = Measure-Run $t.Exe $t.M3u
```

with

```powershell
        $secs = Measure-Run $t.Exe $t.M3u $t.Keep
```

- [ ] **Step 3: The README**

In `rust/e2e/README.md`, after the sentence on `MOCK_M3U_BIG`. Replace

```markdown
404 first. `MOCK_M3U_BIG=N` serves `/m3u/big.m3u`
with N channels, for `measure-startup.ps1 -M3u`. An M3U scenario starts with
```

with

```markdown
404 first. `MOCK_M3U_BIG=N` serves `/m3u/big.m3u`
with N channels, for `measure-startup.ps1 -M3u`. `MOCK_VOD_BIG=1` serves a film
and series catalog of Pandora's size (47,188 films, 10,589 series, 200 and 100
categories, every value a string as the portal sends it), for
`measure-startup.ps1 -VodBig`: both exes load it at every start, and the new
one keeps its config folder, so from the second run on it reads its index cache
too. The mock makes the lists once and keeps them in the temp folder
(`bongo-mock-vod-big-1`). An M3U scenario starts with
```

- [ ] **Step 4: Check them, without a window**

Write this check to a file in the scratchpad (here `mockcheck.py`) and run it from `C:\Dropbox\Sources\tv` with `py -3 <scratchpad>\mockcheck.py`. It starts the mock on a port of its own, with an empty folder for its segments, asks for the four lists and stops it:

```python
import json, os, subprocess, sys, tempfile, time, urllib.request

segs = tempfile.mkdtemp()
env = dict(os.environ, MOCK_VOD_BIG="1")
log = os.path.join(segs, "requests.log")
mock = subprocess.Popen([sys.executable, "rust/e2e/mock_portal.py", "18097", segs, log], env=env)
try:
    base = "http://127.0.0.1:18097/player_api.php?username=tester&password=secret&action="
    for _ in range(100):
        try:
            urllib.request.urlopen(base + "get_live_streams", timeout=1).read()
            break
        except OSError:
            time.sleep(0.1)
    for action in ["get_vod_streams", "get_series", "get_vod_categories", "get_series_categories"]:
        rows = json.load(urllib.request.urlopen(base + action, timeout=30))
        print(action, len(rows), type(rows[0]["category_id"]).__name__)
finally:
    mock.terminate()
```

Expected: `get_vod_streams 47188 str`, `get_series 10589 str`, `get_vod_categories 200 str`, `get_series_categories 100 str`.

Then, in PowerShell, that the script parses: `$e = $null; [void][System.Management.Automation.Language.Parser]::ParseFile('C:\Dropbox\Sources\tv\rust\e2e\measure-startup.ps1', [ref]$null, [ref]$e); $e.Count`. Expected: `0`. Do not run the measurement: it opens the app's window, and Task 12 Step 9 is the controller's, with the user's consent.

- [ ] **Step 5: Commit**

Message file (in the scratchpad):
```
test(rust): the startup A/B on a catalog of Pandora's size: the mock's MOCK_VOD_BIG and measure-startup's -VodBig, which keeps the new exe's config folder

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add rust/e2e/mock_portal.py rust/e2e/measure-startup.ps1 rust/e2e/README.md && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- rust/e2e/mock_portal.py rust/e2e/measure-startup.ps1 rust/e2e/README.md`

---

### Task 12: Docs, the release measurement, full verification

The docs name the new module and what changed around it; then the full suite, the release timing test, a release build, and, by the controller with the user's consent, the startup A/B.

**Files:**
- Modify: `CLAUDE.md`, `rust/README.md`, `docs/VALIDATION.md`

**Interfaces:**
- Consumes everything above. The zenderlijst's later tasks may have rewritten parts of these docs: find each place by the bullet or heading named, and keep its meaning if the wording differs.

- [ ] **Step 1: CLAUDE.md**

In `CLAUDE.md`, in the `player/` bullet, the two lines that list `Cmd` and `Event`. Replace

```markdown
  - `Cmd` (LoadUrl, LoadWithHeaders, Pause, Stop, SeekRelative, CycleAudio/Subtitle, SetWindowSize, Quit) flows in.
  - `Event` (FileLoaded, PlaybackStarted, EndOfFile, Error, PropertyChanged) flows out.
```

with

```markdown
  - `Cmd` (LoadUrl, LoadWithHeaders, LoadVod, Pause, Stop, SeekRelative, CycleAudio/Subtitle, SetAudio/SetSubtitles, SetMute, SetWindowSize, Quit) flows in.
  - `Event` (FileLoaded, PlaybackStarted, EndOfFile, Error, PropertyChanged, Vod) flows out.
  - A `Cmd::LoadVod` (the thuisbioscoop's films, episodes and trailers) goes to mpv byte for byte, with `start=` and `audio-file=` as options of that one file (`vod_loadfile_args`); its log line names the host only. While one plays the thread observes `time-pos`, `duration` and `track-list` (a string, which libmpv gives as JSON) under `OBSERVE_VOD` and reports them as `Event::Vod`, a position or length once its whole second changes; any other load or a stop unobserves them, so live TV reports nothing and costs nothing.
```

In `CLAUDE.md`, at the end of the `portal/` bullet. Replace

```markdown
`types.rs` holds the wire types (`LiveChannel`, etc.). HTTP is async via `reqwest` on a tokio runtime.
```

with

```markdown
`types.rs` holds the wire types (`LiveChannel`, etc.). HTTP is async via `reqwest` on a tokio runtime. Films and series carry what the thuisbioscoop needs (covers, ratings read into tenths, add days, categories; a series' genres, and its `releaseDate` and `release_date` as two fields, since an alias fails the list on an entry with both). `Portal::fetch_categories`, `fetch_vod_info` and `fetch_series_info` have defaults (an M3U list keeps them: none); Xtream's read `get_vod_info` and `get_series_info` tolerantly (`vod_info.rs`) into `VodDetails`, dropping an image address that `redact` would change; their errors name no address (`api_json`).
```

In `CLAUDE.md`, at the end of the `catalog.rs` bullet, and after it a `vod/` bullet. Replace

```markdown
The guide's merge (`guide_channels_of`) returns at once for a list with no catch-up channel, which an M3U list never has.
```

with

```markdown
The guide's merge (`guide_channels_of`) returns at once for a list with no catch-up channel, which an M3U list never has. The fetch task asks the two category lists beside the three lists, in a task of their own, and puts the catalog as soon as the three lists are in, so zapping, `f` and the guide never wait for the categories; what only the title index reads (covers, a series' genre and release year) moves out of the lists there (`vod::index::Extras`, handed on in a `TitleInput`). The titles wait for the categories, 15 s at most (`CATEGORIES_WAIT`; then they go without, with a warn line), and are then built on the blocking pool from the stored lists, which the build reads and copies none of (`TitleIndex::build_with`; ~80 ms for Pandora's 58,000 titles). One `vod:` line says what the lists gave (covers, ratings, years, genres, languages), and the index is put beside the catalog (`put_titles`, counted in `titles_revision`; titles of a catalog that was replaced meanwhile are not put). The index leaves the store only as `Titles` (`CatalogStore::titles`), which apply the adult lock. The store writes the index to its cache file after every build and reads that file once, when the app asks after its first frame (`with_vod_cache`, `read_vod_cache`); the file's titles are put only while the store has none, and the catalog's own take their place.
- **`vod/`** — the thuisbioscoop's data, drawing nothing (spec 2026-10-15; phase 1 wires none of it to a screen). `genres.rs`: the 24 genres with their aliases, the labels and language markers (they count only where labels stand: a prefix, a group in brackets, the end, and a category's leading words), the year of a name (`parse_name` allocates nothing: it reads ~58,000 names per catalog). `versions.rs`: films merged by folded title and year, the highest quality the default. `index.rs` (`TitleIndex`): 20-byte records in (kind, id) order, the names, search names and covers in text tables; search with filters, counts per value and four sorts in one pass, `more_like`, `learn` for what details add, and `build_with`, which builds it from a catalog's stored lists with what they gave up (`Extras`), copying none of them. `cache.rs`: the index as `config\cache\vod-<host>_<port>.json`, written as it is serialized; no adult title, no cover address with an account or a query, and what a file says is checked as it is read (genre bits, a category's place). `mod.rs`'s `write_atomic_with` writes the cache, watching and details files through a temporary file. `watching.rs`: per source `watching.json` / `watching-<id>.json` (progress, Mijn lijst, thumbs; the 60 s and 90 % rules; writes 5 s apart and at a stop; an unreadable file kept as `.bad`). `details.rs` (`DetailStore`: 2 at a time, the newest first, a file per title without an image address that has a query, 24 h for series and 30 days for films). `covers.rs` (`CoverStore`: 6 downloads and 6 file reads at once, dropped when off screen, decoded on the blocking pool into fixed sizes, a 96 MB texture LRU generic over `CoverTexture`/`Uploader` so it is tested without a GPU, `prune` to 500 MB at every start (`main`), `placeholder_tint`).
```

In `CLAUDE.md`, the `search.rs` bullet. Replace

```markdown
- **`search.rs`** cross-catalog search over live / VOD / series (`ItemKind`).
```

with

```markdown
- **`search.rs`** cross-catalog search over live / VOD / series (`ItemKind`). `f` logs `search: <n> results in <ms> ms` per keystroke (`app.rs::paint_search`), never what was typed: the baseline the thuisbioscoop's search is held to (`tests/test_vod_timing.rs`, ignored, release); `tests/test_f_timing.rs` times `f` alone, before phase 1 and after it.
```

In `CLAUDE.md`, the `favorites.rs` / `storage.rs` / `presets.rs` bullet. Replace

```markdown
- **`favorites.rs` / `storage.rs` / `presets.rs`** handle persistence (favorites, last-watched-for-resume) under the platform config dir.
```

with

```markdown
- **`favorites.rs` / `storage.rs` / `presets.rs`** handle persistence (favorites, last-watched-for-resume) under the platform config dir. `Storage::watching_path` is the thuisbioscoop's watching file of the source in use; `remove_source_files` removes it with its `.bad` copy.
```

In `CLAUDE.md`, at the end of the gotcha "The catalog is the only adult filter". Replace

```markdown
a pick from it marks the playing stream adult as well (`current_adult_hint`), for Lock and for `last_watched.json`.
```

with

```markdown
a pick from it marks the playing stream adult as well (`current_adult_hint`), for Lock and for `last_watched.json`. The thuisbioscoop's title index keeps adult titles with a flag and leaves the catalog only as `Titles`, whose every answer leaves them out while locked; a `Titles` asked while locked stays locked. Its cache file never holds an adult title.
```

- [ ] **Step 2: rust/README.md**

In `rust/README.md`, in "## Where the app keeps its data", after the item on saved M3U lists. After

```markdown
- Saved M3U lists: `cache\m3u-<id>.m3u` in the config folder, shown at once on
  the next start while the fresh list downloads.
```

add:

```markdown
- The films and series of an Xtream portal, for the coming thuisbioscoop:
  `cache\vod-<host>_<port>.json` in the config folder, written after every
  catalog load and read at the next start. It holds titles, covers and
  ratings, never an adult title, a user name or a password.
```

- [ ] **Step 3: docs/VALIDATION.md**

In `docs/VALIDATION.md`, after the section "## Search". Replace

```markdown
- [ ] Esc dismisses search

## Settings (o) and language
```

with

```markdown
- [ ] Esc dismisses search

## Thuisbioscoop, phase 1 (the log only)
- [ ] Start on the Xtream portal and wait for the catalog: the log says `vod: <n> titles (… series, … films in … versions, … left out), … categories - cover … %, rating … %, year … %, genre … %, Dutch … %, English … % - index … KB, covers … KB, built in … ms`. Note the line for the user: the shares say how good the portal's data is, and the build stays within 150 ms
- [ ] The catalog comes first and the titles after it: zapping, the guide and `f` work as soon as `catalog: loaded in …` is logged, before the `vod:` line
- [ ] Then `vod: cache written (<n> titles, … KB)`, and `%APPDATA%\BongoIPTVPlayer\config\cache\vod-<host>_<port>.json` exists; it holds no user name, no password and no adult title
- [ ] Quit and start again: soon after the window shows, `vod: cache read (<n> titles, written at …) in … ms`
- [ ] `f`, type a few letters: one line per keystroke, `search: <n> results in <ms> ms`, never the letters typed; the search finds and plays as before
- [ ] On an M3U source: no `vod: <n> titles` line and no cache file
- [ ] Zapping, the guide and a film from `f` play as before

## Settings (o) and language
```

- [ ] **Step 4: Full verification**

- Run `ct`. Expected: all pass. Report the total and how many are new since Task 1's baseline (123 new tests: 14 + 11 + 6 + 12 + 10 + 12 + 14 + 14 + 11 + 11 + 5 in the new files, 2 in `app.rs` and 1 in `catalog.rs`; and 3 new ignored ones, the smoke test and the two timing tests).
- Run `cc`. Expected: the 2 old warnings only.
- Run `fmt src/vod/mod.rs src/storage.rs src/i18n.rs src/main.rs src/portal/types.rs src/player/events.rs src/player/mpv.rs src/player/mpv_sys.rs tests/test_vod_portal.rs tests/test_vod_genres.rs tests/test_vod_versions.rs tests/test_vod_index.rs tests/test_vod_search.rs tests/test_vod_catalog.rs tests/test_vod_cache.rs tests/test_vod_watching.rs tests/test_vod_details.rs tests/test_vod_covers.rs tests/test_player_vod.rs tests/test_vod_timing.rs tests/test_f_timing.rs tests/test_storage.rs tests/test_catalog_adult.rs tests/test_mpv_smoke.rs`. Expected: no output.
- The hunk counts, file by file as in Task 1's Step 1. Expected: the lines Task 1 Step 1 recorded.
- Run `bash rust/e2e/test-check-staged.sh` from `C:\Dropbox\Sources\tv`. Expected: every case `ok`.

- [ ] **Step 5: The timing tests in release**

Run: `cd /c/Dropbox/Sources/tv/rust && CARGO_TARGET_DIR='C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt-thb' cargo test -j 4 --release --test test_vod_timing -- --ignored --nocapture`

Expected: it passes and prints `index build: <n> titles (…): first … ms, best … ms, mean … ms, worst … ms`, `index build memory: … MB at its peak, … MB kept (the index), … MB passing (its own working lists); no list copied`, `what the lists give up for it: … MB of handles (the strings move), held from the catalog's put until its titles are built`, `index size: <x> MB (budget 5 MB), covers beside it <y> MB`, `list records: Movie … B x 47188, Series … B x 10589 = … MB`, `film list parse: … ms, series list parse: … ms (on the fetch's worker, as before)`, a table of keystrokes (`index: text`, `index: + filters`, `f today`) and `worst keystroke: index … ms (budget 16 ms, and no slower than f), f … ms`. When this plan was written, on the user's PC: build 74–83 ms (67–85 ms with the follow-up rulings), index 4.16 MB (covers 4.02 MB beside it), the build at its peak 15.3 MB on the heap, of which 8.2 MB kept and 7.1 MB passing, the lists' handles 1.6 MB, a keystroke 1.3–2.7 ms for the index and 8–16 ms for `f`.

Then `f` against the base: `cd /c/Dropbox/Sources/tv/rust && CARGO_TARGET_DIR='C:\Users\brnck\AppData\Local\Temp\claude\C--Dropbox-Sources-tv\7bade0af-c82f-473e-9f05-1727a9a2ac83\scratchpad\tgt-thb' cargo test -j 4 --release --test test_f_timing -- --ignored --nocapture`. Expected: its worst keystroke within ±10 % of what Task 1 Step 0 printed.

Write all of it in your report: what phase 1 adds to memory is the index, the covers and the list records together (the remedies, should the user count them: a prefix table for the cover addresses, narrower stored lists; Ruling 22); beside them pass the build's own working lists while it runs and the handles of what the lists gave up until the titles are built. The build is what the laaddraad spends, and the lists' parse stands beside it. Over a budget (150 ms for the build, 5 MB, 16 ms or slower than `f` for a keystroke), or `f` more than 10 % slower than at the base: report it and change nothing; the controller takes the numbers to the user first (spec §5).

- [ ] **Step 6: The release build**

Run without `CARGO_TARGET_DIR`: `cd /c/Dropbox/Sources/tv/rust && cargo build --release -j 4` (`build.rs` copies `rust\vendor\libmpv\libmpv-2.dll` next to the exe; Global Constraints, Machine quirks). Then `--selftest` through `Start-Process -Wait -RedirectStandardOutput`; it must say it passed.
- Never open, print or touch a `BongoPlayer.ini` or `BongoIPTVPlayer.ini`, and do not look into `rust\target` beyond the exe.

- [ ] **Step 7: The 0.6.1 baseline for the controller**

```powershell
$b = Join-Path $env:TEMP 'bongo-baseline-0.6.1'
if (-not (Test-Path $b)) { Expand-Archive 'C:\Dropbox\Sources\tv\rust\dist\BongoPlayer-v0.6.1.zip' -DestinationPath $b }
$exe = Get-ChildItem $b -Recurse -Filter BongoPlayer.exe | Select-Object -First 1
$o = Join-Path $env:TEMP 'bongo-baseline-version.txt'
Start-Process $exe.FullName -ArgumentList '--version' -Wait -NoNewWindow -RedirectStandardOutput $o
Get-Content $o
```

Expected: `Bongo Player 0.6.1`. Write the exe's path in the report. (Reading the zip from the main checkout changes nothing there.)

- [ ] **Step 8: Commit**

Message file (in the scratchpad):
```
docs: the thuisbioscoop's data layer (phase 1) in CLAUDE.md, the README and VALIDATION

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WsPJfuisYwZ6smLPWrbS2D
```
Run from `C:\Dropbox\Sources\tv`: `git add CLAUDE.md rust/README.md docs/VALIDATION.md && bash rust/e2e/check-staged.sh && git commit -q -F <msgfile> -- CLAUDE.md rust/README.md docs/VALIDATION.md`

- [ ] **Step 9: (the controller, with the user's consent) The startup A/B**

Ask the user first: these runs open the app's window 60 times. Task 11b's check made the mock's big lists in the temp folder; should it have been emptied since, run that check again first, so that no measured run waits for them.

```powershell
cd C:\Dropbox\Sources\tv\rust
powershell -NoProfile -ExecutionPolicy Bypass -File e2e\measure-startup.ps1 -Baseline <the 0.6.1 exe> -Runs 10
powershell -NoProfile -ExecutionPolicy Bypass -File e2e\measure-startup.ps1 -Baseline <the 0.6.1 exe> -Runs 10 -VodBig
powershell -NoProfile -ExecutionPolicy Bypass -File e2e\measure-startup.ps1 -Baseline $env:TEMP\bongo-baseline-base\BongoIPTVPlayer.exe -Runs 10 -VodBig
```

- The new exe (`rust\target\release\BongoIPTVPlayer.exe`, the script's default) against 0.6.1, on the mock portal (an Xtream source, as the spec asks): once as before, and once on the catalog of Pandora's size (`-VodBig`, Task 11b), where it builds its index at every start and, from the second run on, reads its cache. In both, its median and minimum are within ±10 % of the baseline's, or better. A slower result stops the phase: investigate before going on, and take the numbers to the user (spec §5).
- The third run, against the rebased base's own release (Task 1 Step 0), is for information: it shows phase 1's own share beside what the zenderlijst and the rename moved. Report it with the others.
- Before the release test, the user checks the log lines of `docs/VALIDATION.md`'s new section on the real portal; the `vod:` line's shares are the data-quality report of spec §11.

---
