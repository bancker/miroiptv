# Thuisbioscoop phase 2 ("kijken"): implementation plan, for review

> **Status: for the user's review. Nothing is built.** This plan describes each task's files, interfaces, behaviour, tests and finish line. The code for each step follows after the review, once the decisions in §0 are made, as phase 1's plan did after its spec review. The decisions can still change the tasks.

**Goal:** The first phase the user can test. Press `h` on an Xtream source, search by series name, filter, pick a season and an episode, and watch. Resume where you were, follow on to the next episode, and pick audio and subtitles from a menu. Every speed rule of spec §5 holds. The search page of `h` is shown to be at least as fast as `f`.

**Spec:** `docs/superpowers/specs/2026-10-15-thuisbioscoop-design.md`, §3.1, 3.3–3.6, §5, §6, §7, §8 and §9 (Fase 2). Phase 3 keeps the following:
- Thuis and its rows;
- the Series, Films and Mijn lijst tabs;
- the ＋, 👍 and 👎 buttons;
- the trailer.

**Builds on:** `main` at 44f9548, which is phase 1 complete and verified. The data layer is there and draws nothing:
- `vod::index::TitleIndex` and `Titles`;
- `DetailStore`, `CoverStore`, `Watching` and the index cache;
- `Cmd::LoadVod` and `Event::Vod`, plus `TrackChoice` and `Cmd::SetAudio`/`SetSubtitles` in `player/`.

**Architecture:**
- **New screens in `rust/src/home_ui/`:** `HomeView` and `HomeAction`, as `GuideView`/`GuideAction` and `settings_ui` work. A view draws and returns actions, and `app.rs` acts on them.
- **New pure logic in `rust/src/vod/`:**
  - `playback.rs`: what to play, from where, and what comes next;
  - `episodes.rs`: seasons and episodes in order, and "Extra's".
- **No new thread and no new crate.** The fonts are bytes in the exe (spec §7).

---

## 0. Decisions for the user before the build

**D1. Pandora's data has a year for 30 % of its titles and a genre for 32 %.**
- **Source:** VALIDATION's `vod:` line on the real portal, 2026-10-20 (memory "pandora-vod-data-quality"). Covers are at 91 % and ratings at 81 %.
- **Effect:** Z2's Jaar and Genre filters then reach only a third of the titles. A title's details do carry a year and genres, and `TitleIndex::learn` adds them once its page has been opened, so the share grows with use but starts low.
- **Options:**
  - **(a)** Build Z2 as specced. The counts show honestly how few titles a filter reaches.
  - **(b)** As (a), and a filter group whose share is under 50 % says so under its heading ("jaar bekend bij 30 %").
  - **(c)** First design C: OMDb or TMDB as a supplement. That is your business decision: keys, TMDB's attribution, and a licence for commercial use.
- **Recommendation:** (b) now, and C as its own design after you have tested phase 2.

**D2. A 0.7.1 for the PNG crash.**
- **The problem:** Task 9's review found that an 81-byte PNG with an impossible palette length makes `png` 0.17 panic. The release profile has `panic=abort`, so the app closes. The guide's logos in 0.7.0 are hit by this today, if a portal ever serves such a logo.
- **The fix** is on `main` (d6f8a1f). Phase 1 shows nothing, so `main` as it is could ship as 0.7.1.
- **Your call:** version, tag and push are yours. This plan does not depend on it.

**D3. The ＋/👍/👎 buttons on the pages.**
- Spec §9 puts "＋, 👍 en 👎" in phase 3 and leaves only the Trailer button out of phase 2 by name.
- **This plan leaves all four buttons out of phase 2.** Without Thuis there is no row for them to feed, and Mijn lijst has no tab yet. Say so if you want them now: they are one small task, since `Watching` already stores them.

**D4. Choices this plan makes. Say so if you want otherwise.**
- **One account, one index file.** The index cache becomes `vod-<host>_<port>-<account>.json`, where `<account>` is the first 8 hex digits of SHA-256 of the user name, never the name itself. A second account on the same server then never shows the first one's titles. This was the review's m3 for Task 6b, where it is a privacy gap between two accounts.
- **Adult titles are kept in memory only.** An adult title's details and cover are never written to disk, as with the guide's adult logos (Task 8 m4, Task 9 i4).
- **An empty answer is remembered for the session.** A title whose details came back empty is not asked again in that session, and nothing is kept on disk for it (Task 8, "what phase 2 must do", item 1).

---

## 1. What phase 1 handed to this plan

Every item that a phase 1 review carried to phase 2, and the task that takes it.

| From | Item | Task |
|---|---|---|
| Task 5 m9(c) | Compare the adult lock per frame, not only when the catalog changes | 3 |
| Task 5 carry | On an M3U source, titles of `None` mean there is no thuisbioscoop, not that it is still loading | 3 |
| Task 5 m9(b) | `Titles::categories()` is not filtered by the lock | Phase 3: its category rows read it; Z2 does not |
| Task 6b m3 | A second account on the same host and port shows the first account's titles | 1 |
| Task 7 m3/m5 | No flush on drop; a position past the duration; film keys; a leftover `.tmp`; a repointed source keeps its history | 10 |
| Task 8 list 1–6, m2 | A fourth state for an empty answer; when to call `request`, `retry` and `drain`; where the store lives; the adult question; texts; the warn line becomes debug | 6 |
| Task 8 m4, m7 | An adult title's details in memory only; bound the memory by size as well | 6 |
| Task 9 i4 | Pages call `cover_in_memory` for adult titles | 4 |
| Task 9 m4 | Nothing asks for a repaint when a cover arrives; `Missing` lasts the whole session; two sizes of one address download twice; a token in the address breaks the file name | 4 (repaint, sizes), 15 (notes) |
| Task 9 m3, m7 | Prune by last use, not by write time; a pixel cap of 36 MP beside the side cap | 4 (pixel cap); prune to phase 3 |
| Task 10 m1 | A load number in `Event::Vod` | 9 |
| Task 11 m6 | An empty query returns at once | 5 |
| c30f319d | Pandora's data quality | §0 D1 |
| 6a | redact's pass-skipping (an option) | Not taken: no measurement asks for it |

The following stay where they were carried: phase 3 (6a m2, Task 8 m6) and the backlog (`sync_all` before a rename, a single-instance guard).

---

## 2. Global constraints

Phase 1's `plan-globals.md` rules apply, together with CLAUDE.md:

- **Performance comes first (spec §5).**
  - A closed thuisbioscoop costs nothing per frame: no work, no memory, no network.
  - The fonts are registered when `h` first opens, never at start.
  - `f` and zapping do not change.
- **Strings:** every text the user sees is in `i18n.rs`, in NL, EN and DE. Log lines are in English.
- **Secrets:**
  - A stream address is built only at play time (`movie_stream_url`, `series_stream_url`).
  - It goes to mpv byte for byte through `Cmd::LoadVod`.
  - It never appears in a log, a cache or `Debug` output. New types that hold one get `cfg_attr(test, derive(Debug))`.
- **Adult content:** the catalog's lock is the only adult filter. An open adult page closes when the lock goes on.
- **The machine:**
  - Tests are TDD: red, then green.
  - The `ct`/`cc`/`fmt` gates as in phase 1, with the rustfmt baseline unchanged.
  - Builds go in a target outside Dropbox (never `rust\target`).
  - Commits go through `rust/e2e/check-staged.sh`.
  - Window runs only with the user's consent, against the mock only, with posted keys only.
- **`app.rs` is the bottleneck.** Tasks 3, 9, 11, 12 and 13 touch it, and they run one after another.

## 3. Files

| File | Task | What |
|---|---|---|
| `rust/assets/fonts/` + `OFL.txt` | 2 | Bebas Neue and Inter (Regular and Bold), subset to Latin and Latin-1 plus ★ ✓ ▶ ⏸ ↺ ✕ ▾, ≤ 400 KB together |
| `home_ui/theme.rs` | 2 | Colours, sizes and fonts; `ensure_fonts(ctx)` registers them once |
| `home_ui/mod.rs` | 3 | `HomeView`, `HomeAction`, `Page` (Search, Series, Film), and the back stack |
| `home_ui/widgets.rs` | 4 | The cover tile, the focus ring with its 107 % grow, labels, ★, the progress bar and buttons |
| `home_ui/search.rs` | 5 | Z2 |
| `home_ui/series_page.rs` | 7 | Opzet 2 |
| `home_ui/film_page.rs` | 8 | F2 |
| `home_ui/osd.rs` | 11, 12 | The bar, the track menu and the next-episode card |
| `vod/episodes.rs` | 7 | Seasons in order, "Extra's", the next episode, and crossing to the next season |
| `vod/playback.rs` | 9, 12, 13 | What to play and from where: resume, the 60 s and 90 % rules, reload once |
| `vod/cache.rs` | 1 | The account in the file name |
| `vod/details.rs` | 6 | `Empty`, adult in memory only, a size bound |
| `vod/watching.rs` | 10 | Task 7's follow-ups |
| `player/` | 9 | The load number in `Event::Vod` |
| `app.rs` | 3, 9, 11–13 | `h`, the TV behind it, playback, the bar's keys, next episode, reload |
| `i18n.rs` | all | New keys |
| `rust/e2e/mock_portal.py` and scenarios | 15 | Films with versions, series with seasons, details, covers |

---

## Task 1: The account in the index cache's name (D4)

**Files:** `vod/cache.rs`, `catalog.rs` (only the call that builds the path), `tests/test_vod_cache.rs`.

**Build:**
- `cache::path_for(cache_dir, creds)` returns `vod-<host>_<port>-<acct>.json`, where `acct` is the first 8 hex digits of `sha256(username)`. A file written under the old name `vod-<host>_<port>.json` is removed the first time the new one is written, and it is never read.
- Details (`cache\vod\<host>_<port>\`) and covers stay shared. They describe the portal's titles, not the account's package.

**Tests:**
- `two_accounts_on_one_server_have_two_files`
- `the_name_holds_no_user_name`: it searches for the user name and the password, in any case.
- `the_old_file_is_removed_when_the_new_one_is_written`
- `an_account_whose_titles_never_came_shows_none_of_another`: this is 6b's m3 scenario, with a refused login.

**Done when:** those tests are green. The rest of ct stays green, and the startup A/B is unchanged: the read stays the same size and is made once.

## Task 2: Fonts and theme

**Files:** `rust/assets/fonts/*` (`.ttf`, subset), `OFL.txt`, `home_ui/theme.rs`, and a size check in `tests/test_home_theme.rs`.

**Build:**
- Bebas Neue for titles; Inter Regular and Bold for text.
- The subset is made once with `pyftsubset`. The command goes in the file's doc comment, so the subset can be made again.
- `include_bytes!` puts them in the exe. `theme::ensure_fonts(&egui::Context)` adds them as new font families on the first `h`, never at start, and the rest of the app keeps its own fonts.
- `Theme` holds the colours (the sketches' dark grey, white and the gold focus) and the sizes: tiles 2:3, stills 16:9, the 107 % focus.

**Tests:**
- `the_fonts_together_are_at_most_400_kb`
- `ensure_fonts_adds_the_families_once` (headless `Context`)
- `the_app_without_h_never_registers_them`: a Harness start with frames, and the fonts are absent.

**Done when:** the exe grows by at most 400 KB and the startup A/B is unchanged.

## Task 3: `h` opens and closes; the TV behind it

**Files:** `home_ui/mod.rs`, `app.rs`, `i18n.rs`, and the tests in `app.rs` (Harness).

**Build:**
- **`h` on an Xtream source** opens `HomeView` on the search page, the only tab in phase 2. The search field has the focus.
- **`h` on an M3U source** shows a toast: "De thuisbioscoop werkt met een Xtream-bron". Titles of `None` there mean "no thuisbioscoop", never "loading" (Task 5 carry).
- **While it is open:**
  - `Cmd::SetMute(true)` goes to the player.
  - The app does not upload TV frames to the texture: the `FrameBus` is not drained into it. mpv keeps fetching, so going back is instant.
  - On close, `SetMute(false)` goes to the player and the picture is drawn again.
- **Esc goes one step back:** from a page to where you came from, and from the search page it closes the thuisbioscoop.
- **`h` as a key:**
  - On a series page or a film page, `h` closes directly.
  - On the search page, `h` is a letter, because typing always goes to the field (spec §3.3 with §3.1). Esc closes there.
- **The back stack** is in `HomeView`: `Vec<Page>`, at most 16 deep, and the oldest entry drops off.
- **The adult lock:**
  - `HomeView` compares `catalog.adult_unlocked()` with the value from the frame before, in every open frame (Task 5 m9(c)).
  - On a change it computes the results again.
  - It closes an open page of an adult title when the lock goes on (spec §6).
- **Closed, `HomeView` is `None`.** `update_ui` does no work for it, holds no memory and makes no request.
- **Switching source** closes the thuisbioscoop (spec §6).

**Tests:**
- `h_opens_on_search_and_esc_closes`
- `h_on_an_m3u_source_says_it_needs_xtream`
- `the_tv_is_muted_and_not_uploaded_while_open_and_back_after`: this counts uploads in the fake uploader.
- `h_closes_from_a_page_but_types_in_the_field`
- `the_lock_going_on_closes_an_adult_page`
- `a_closed_home_view_costs_no_frame_work`: allocations counted over 100 frames, the same before and after `h` is closed.
- `switching_source_closes_it`
- `a_zap_key_while_open_does_not_zap`

**Done when:** the tests are green and `f` and zapping are unchanged (their tests, plus `test_f_timing`).

## Task 4: The tile and the widgets

**Files:** `home_ui/widgets.rs`, `vod/covers.rs` (a pixel cap), and `tests/test_home_widgets.rs`.

**Build:**
- **`cover_tile(ui, store, title, focused) -> Response`.**
  - The tile is 2:3, with the title, ★ and the year.
  - A title without a cover gets a placeholder in `placeholder_tint(name)` with the title on it.
  - The label is SERIE or FILM, and NIEUW for a title added in the last 7 days.
  - ★ is never shown as "★ 0" (spec §6).
- **Adult titles** are asked for with `cover_in_memory` (Task 9 i4).
- **Repaint while loading:** while any tile on screen `is_loading`, the view asks egui for a repaint (`request_repaint_after(50 ms)`), so an arriving cover shows without input (Task 9 m4).
- **Focus:** a white ring and growth to 107 % over 120 ms (`ctx.animate_value_with_time`).
- **The progress bar** is yellow on dark, and shows "NOG 23 MIN".
- **The pixel cap:** `MAX_PIXELS` is 36 MP beside the side cap of 8192, so a real cover of 4000×6000 still decodes and the hostile worst case drops from 576 MB to about 300 MB (Task 9 m7).
- **One address at two sizes downloads once:** the download is keyed by address, and the decode by size (Task 9 m4).

**Tests:**
- `a_tile_without_a_cover_shows_its_title_on_the_tint`
- `no_star_for_a_rating_of_zero`
- `an_adult_tile_asks_cover_in_memory`
- `a_loading_tile_asks_for_a_repaint`
- `a_picture_over_36_megapixels_is_no_cover`
- `two_sizes_of_one_address_download_once`

**Done when:** the tests are green, and a frame of 60 tiles costs less than 1 ms in the timing test.

## Task 5: The search page (Z2)

**Files:** `home_ui/search.rs`, `i18n.rs`, `tests/test_home_search.rs`, and `tests/test_vod_timing.rs` (Z2's keystroke).

**Build:**
- **Left column (21 %):**
  - the field;
  - the filter groups with counts: Soort, Genre (the 24 of spec §4.2), Jaar (four ranges), Waardering (≥ 8, ≥ 7), Taal (NL, EN, andere).
- **Counts:** a group's counts apply to the query plus the other groups' filters, in one pass (`TitleIndex::search` with counts, as phase 1 built and timed it).
- **D1 (b):** a group whose share of known values is under 50 % shows it under the heading.
- **Right side:**
  - the number of titles;
  - the active filters as yellow chips with ✕;
  - Sorteer: Relevantie, Best gewaardeerd, Nieuwst or Titel (A–Z);
  - the grid of tiles (Task 4).
- **Keys (§3.3):**
  - Typing always goes to the field, and ⌫ works there.
  - Enter in the field opens the top hit.
  - ↓ goes to the filters. There, ↑/↓ moves, Space or Enter toggles, and → goes to the grid.
  - In the grid, the arrows move and Enter opens a page. ← in the first column goes back to the filters.
- **The log line:** each keystroke and each toggle logs `search: <n> results in <ms> ms`, the same line as `f`, so the two stand side by side. Nothing of what was typed is logged.
- **An empty query** returns the whole sorted list from the index's sort order, without comparing any name (Task 11 m6).
- **The drawing** shows the visible rows only (`ScrollArea::show_rows`).

**Tests:**
- `typing_anywhere_goes_to_the_field`
- `enter_opens_the_top_hit`
- `a_filter_toggles_with_space_and_its_count_follows_the_query`
- `pokemon_finds_pokemon_with_accents`
- `relevance_puts_a_prefix_first_then_the_rating`
- `a_chip_removes_its_filter`
- `the_log_line_names_no_query`
- `the_empty_query_compares_no_name`, timed in the timing test
- `a_group_known_for_under_half_says_so`

**Timing (release, ignored):** Z2's keystroke with counts on 58,000 titles stays within 16 ms and no slower than `f`, measured with the same rule as phase 1 (best of 5, three rounds).

## Task 6: Details on a page (Task 8's list)

**Files:** `vod/details.rs`, `home_ui/mod.rs`, `tests/test_vod_details.rs`, `i18n.rs`.

**Build:**
- **`DetailState::Empty`:** the portal answered, with nothing a page shows.
  - The page keeps what the index knows and shows no error line. Enter has nothing to ask again.
  - The verdict lasts the session (D4), and the warn line becomes a debug line (Task 8 m2).
- **When to call the store:**
  - `request` once when a page opens, and again when it comes back to the front, never from a draw.
  - `retry` only on a fresh Enter press, never on a key repeat.
  - `drain(now_unix)` once per frame, before `get`.
- **The store lives on the screen's thread.** `install_portal` makes a new one with `dir_for(cache_dir, creds)`.
- **Adult titles:** `request(id, now, adult)`. An adult title's details are never written to a file (D4, Task 8 m4).
- **Memory:** what stays in memory is bounded by size as well as by count. 64 entries or 8 MB of text, whichever comes first (Task 8 m7).
- **`Failed`** shows "Konden niet geladen worden, Enter probeert opnieuw" in NL, EN and DE.
- **`learn`:** what a title's details add (year, genres) goes into the index (`TitleIndex::learn`), so Z2's counts grow with use.

**Tests:**
- `an_empty_answer_is_empty_not_failed_and_is_not_asked_again`
- `request_is_not_called_from_a_draw`: this counts asks over 100 frames of an open page, and expects 1.
- `a_held_enter_retries_once`
- `an_adult_titles_details_never_reach_the_disk`
- `memory_stays_under_8_mb_of_text`
- `what_details_add_shows_in_the_counts`

## Task 7: The series page (opzet 2)

**Files:** `vod/episodes.rs`, `home_ui/series_page.rs`, `tests/test_vod_episodes.rs`, `tests/test_home_series.rs`.

**Build:**
- **`episodes::Seasons::from(details)`:**
  - The seasons are in order. Season 0 is called "Extra's" and comes last.
  - A series without a season split gets one tab, "Afleveringen".
  - `next_after(episode)` gives the next episode, which may be episode 1 of the next season, or `None` after the last episode.
- **The page's left column:**
  - the title, ★ with its source (PANDORA), the year or years, the age rating, the number of seasons and the genre;
  - the main button, which is one of:
    - ▶ Verder kijken · S·A, when there is progress;
    - ▶ Afspelen · S1 A1, when nothing is seen;
    - ↺ Opnieuw kijken, when everything is seen;
  - ↺ Vanaf het begin, when there is progress;
  - the description, "Met: …" and "Genre: …".
  - There is no Trailer button and there are no ＋/👍/👎 (D3).
- **The page's right column:**
  - the season tabs;
  - per episode: the still (16:9), the number, the title or "Aflevering <n>", the duration and two lines of description;
  - ✓, a yellow bar, "NOG 23 MIN" and NIEUW.
- **Focus and keys:**
  - The focus starts on the main button. → or Tab goes to the list, at the current episode.
  - In the list, ←/→ changes the season, ↑/↓ chooses an episode, and Enter plays it from its saved place.
  - This is the channel list's focus model (groups ↔ channels).
- **Missing images:** an empty backdrop, season cover or still shows a placeholder (Task 8, Ruling 9's consequence).

**Tests:**
- `season_zero_is_extras_and_last`
- `a_series_without_seasons_has_one_tab`
- `the_next_episode_crosses_to_the_next_season`
- `after_the_last_there_is_none`
- `the_main_button_reads_continue_play_or_again`
- `enter_on_an_episode_plays_from_its_place`
- `left_right_changes_season_in_the_list`
- `an_episode_without_a_title_is_aflevering_n`

## Task 8: The film page (F2)

**Files:** `home_ui/film_page.rs`, `tests/test_home_film.rs`.

**Build:**
- **The page's left column:**
  - the title, ★ with its source, the year, the age rating, the duration, and 4K/HDR and 5.1 when the portal gives them;
  - ▶ Verder kijken or Afspelen;
  - the progress line ("nog 1 u 34 min · ↺ Vanaf het begin");
  - "Versie: 4K HDR ▾". `v` opens the version choice, which is the versions of `versions.rs`;
  - the description, the director and the cast.
- **The page's right column:** "Meer zoals dit · <genre>", 6 titles from the same genre or category, best rated first.
  - It leaves out titles already seen and titles with 👎. The thumbs exist in `Watching`, even though the buttons come in phase 3.
- **Keys:** → goes to the grid, Enter opens or plays, and `v` opens the version choice.

**Tests:**
- `the_version_choice_lists_every_version_best_first`
- `more_like_this_leaves_out_seen_and_thumbs_down`
- `a_film_without_details_shows_what_the_index_knows`
- `v_opens_the_versions_and_enter_plays_that_one`

## Task 9: Playing from a page

**Files:** `vod/playback.rs`, `player/` (the load number), `app.rs`, `tests/test_vod_playback.rs`, and Harness tests.

**Build:**
- **The address:** `playback::start(title, version_or_episode, watching)` gives the address, built only now (`movie_stream_url`/`series_stream_url`), and the start position:
  - a place under 60 s is not started, so it plays from 0;
  - a seen item plays from 0.
- **Starting it:**
  - The app sends `Cmd::LoadVod` byte for byte with `start=`.
  - Starting stops the TV, because the portal allows one session.
  - Closing the thuisbioscoop afterwards reloads the last channel, as a zap.
- **The load number (Task 10 m1):**
  - `Event::Vod` carries the number of the load the thread took.
  - The app keeps the number of the load it sent, and drops a report of an older load.
- **The watching store:**
  - It is written every 5 s, only on a change, and at a stop.
  - An item counts as seen from 90 %, or at the end of the file.
- **A refused load:** you stay on the page with "Kan dit niet afspelen". When the film has another version, the page adds "Probeer de versie <label> (Enter)".
- **`f` is unchanged.** A film started with `f` plays as today, without the bar or the menu.

**Tests:**
- `a_place_under_60_s_plays_from_the_start`
- `what_goes_to_mpv_is_the_address_byte_for_byte_and_start`
- `a_report_of_an_older_load_is_dropped`
- `the_place_is_written_every_5_s_on_a_change_and_at_stop`
- `90_percent_is_seen`
- `a_refused_film_offers_its_other_version`
- `closing_after_a_film_zaps_back_to_the_last_channel`
- `a_film_from_f_plays_as_before`

## Task 10: The watching store's follow-ups (Task 7 m3/m5)

**Files:** `vod/watching.rs`, `storage.rs`, `tests/test_vod_watching.rs`.

**Build:**
- **Write on close:** the app flushes the store at its exit and when the source switches, which is a call and not a `Drop`, so a panic-abort writes nothing half.
- **A position past the duration** is clamped to the duration.
- **Film keys** use the version's stream id, and progress shows per title (its most recent version). This is unchanged and is written down in the doc.
- **A leftover `watching.json.tmp`** is removed at load.
- **A source saved with another address** drops its watching file, as `bad_streams` does: the history belongs to the old address.

**Tests:**
- `a_flush_at_exit_writes_the_last_place`
- `a_position_past_the_end_is_the_end`
- `a_leftover_tmp_is_removed`
- `a_repointed_source_starts_with_no_history`

## Task 11: The bar and the keys while watching; the track menu

**Files:** `home_ui/osd.rs`, `app.rs`, `i18n.rs`, `tests/test_home_osd.rs`, and Harness tests.

**Build (spec §3.6):**
- **The bar** appears on a key press or a mouse move and hides after 3 s.
  - **Top:** ← and the title ("Serie · S2 A5 · Titel").
  - **Bottom:**
    - the time bar with the position and the buffer;
    - "1:13:04 · nog 1:33:12";
    - ⏪ ⏯ ⏩, Audio en ondertitels, Volgende aflevering (an episode only), and full screen.
- **Keys:**

  | Key | What it does |
  |---|---|
  | Space | Pause and resume |
  | ←/→ | ∓10 s; holding the key repeats it |
  | ↑/↓ | ∓60 s, and never a zap: no key zaps during VOD |
  | `a`/`s` | Open the menu of the real tracks (`track-list`: `lang` and `title`). ↑/↓ and Enter choose, and the choice is `Cmd::SetAudio`/`SetSubtitles` |
  | `n` | The next episode |
  | Esc | Stop, save the place, and go back to the page |

- **Live catch-up** keeps its 30 s jumps.

**Tests:**
- `the_bar_hides_after_3_s_and_shows_on_a_key`
- `up_down_seek_60_s_and_never_zap`
- `a_opens_the_menu_with_the_real_tracks`
- `enter_in_the_menu_sends_set_audio`
- `esc_stops_saves_and_goes_back`
- `held_right_repeats_10_s_jumps`

## Task 12: The next episode

**Files:** `home_ui/osd.rs`, `vod/episodes.rs`, `app.rs`, and Harness tests.

**Build:**
- **The card** appears when 30 s or less are left: "VOLGENDE AFLEVERING · S2 · A6 · Titel", with the still, the duration and a 10 s countdown.
  - Enter plays the next episode now.
  - Esc hides the card.
  - After the countdown, or at the end of the file, the next episode starts from 0.
- **After the last episode of a season** comes episode 1 of the next season.
- **After the last episode of the series**, you go back to the series page with ↺ Opnieuw kijken.

**Tests:**
- `the_card_shows_at_30_s_left_and_counts_down_10`
- `the_countdown_plays_the_next_from_zero`
- `esc_hides_the_card_and_the_credits_play_on`
- `the_last_of_a_season_goes_to_the_next_season`
- `after_the_last_episode_the_page_offers_again`

## Task 13: Reload once

**Files:** `vod/playback.rs`, `app.rs`, and Harness tests.

**Build:**
- **An unexpected eof before 95 %** (end of file, reason eof) makes the app reload once with `start=<last place>`, and the user sees nothing.
- **A second one within the same minute** brings a toast and goes back to the page.
- **The stall watchdog** stays off for VOD, as it is now.

**Tests:**
- `an_eof_before_95_percent_reloads_once_at_the_place`
- `a_second_eof_within_a_minute_goes_back_with_a_toast`
- `an_eof_after_95_percent_is_the_end`

## Task 14: Help and the texts

**Files:** `i18n.rs`, `app.rs` (the help).

**Build:**
- The help (`?`) gains `h` and the thuisbioscoop's keys.
- All new keys are in NL, EN and DE. The table does not compile with one missing.

**Tests:** `the_help_lists_h_and_its_keys`, plus the i18n table's own build.

## Task 15: The mock portal and the window scenarios

**Files:** `rust/e2e/mock_portal.py`, `rust/e2e/scenarios/H-*`, and `rust/e2e/README.md`.

**Build:**
- **The mock gains:**
  - films with duplicate versions;
  - series with seasons, season 0 and episodes;
  - the categories;
  - `get_vod_info` and `get_series_info`;
  - local covers, as PNG and JPEG;
  - a short video stream for an episode.
- **The scenarios**, with posted keys only and with the user's consent:
  - **H-open:** `h`, Esc.
  - **H-search:** type a name, use a filter, Enter.
  - **H-series:** a season, an episode, Enter, wait, Esc, open again. It resumes.
  - **H-next:** the card and the countdown.
  - **H-m3u:** the toast.
- **Notes for phase 3 go in the README** (Task 9 m4):
  - `Missing` lasts the whole session;
  - a token in a cover address breaks its file name.

## Task 16: Measurements, docs, release

**Steps:**
1. **The debug lines of `h` (spec §5.3):** from `h` to the first frame of the search page (norm 100 ms), and from Enter to the first frame of a page (norm 100 ms).
2. **The timing test in release:** Z2's keystroke against `f`, and a grid frame of 60 tiles.
3. **Memory on the user's PC:** textures at most 96 MB.
4. **Gates and checks:**
   - `ct`, `cc` and `fmt` at the baseline;
   - `test-check-staged`;
   - the release build in a target outside Dropbox;
   - `--selftest`.
5. **The startup A/B against 0.7.0** (and against `main` at 44f9548, for information), within ±10 %.
6. **Docs:** CLAUDE.md (`home_ui/`, `vod/episodes.rs`, `vod/playback.rs`, `h`), VALIDATION (a section for the user's test under `h`) and the README.
7. **For the user:** a release build to test under `h` on the real portal. That is phase 2's test, and the user's.

**A norm that is not met** means the numbers go to the user first, together with what can be done about them (spec §5).
