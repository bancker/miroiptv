# Kwaliteit-switchen met `+` / `-` — ontwerp

**Datum:** 2026-06-17
**Status:** ontwerp, lean-first

## Doel

Tijdens het kijken naar een live-kanaal springt `+` naar een hogere-kwaliteit
variant van hetzelfde kanaal en `-` naar een lagere (SD → HD → FHD → UHD).
Handmatige voorloper van een later freeze-gedreven auto-downgrade.

## Beslissingen

- **Aan de grens:** stoppen + toast (geen wrap). Boven op de hoogste → "al op
  hoogste"; onder op de laagste → "al op laagste".
- **Geen geheugen:** elk kanaal start op de beste variant (huidig zap-gedrag);
  `+`/`-` geldt alleen voor wat je nu kijkt. Wegzappen en terugkomen reset naar
  best.
- **Alleen live-kanalen** (VOD/films hebben geen varianten → toast).
- **Eén representant per kwaliteits-tier** (matcht het HD→FHD→UHD-model).
- **Gevolg:** na een gewone zap zit je al op de beste variant, dus `+` meldt
  "al op hoogste"; `-` stapt omlaag, `+` klimt weer terug.

## Performance (bindend — zie [[feedback_performance_priority]])

- `+`/`-` als text-events in de bestaande per-frame input-scan: twee extra
  `.any()` over `i.events` (die lijst wordt al doorlopen voor `*`/`?`). Geen
  nieuw werk in de paint/render-loop.
- Kwaliteit-logica draait alleen op een toetsdruk.
- **Geen volledige catalog-clone:** `CatalogStore::quality_nav(sid)` leest
  `inner` één keer onder read-lock en geeft alleen de kleine per-kanaal-ladder +
  huidige positie terug.
- Hergebruikt `zap_to`; geen nieuwe threads/async/dependencies.

## Architectuur

Pure logica in `catalog.rs` (naast `quality_rank`/`normalize`/`dedupe_zap`):

- `pub fn quality_rank` — bestaand, wordt `pub`.
- `pub fn quality_ladder(channels: &[LiveChannel], key: &str) -> Vec<LiveChannel>`
  — live (`tv_archive == 0`) varianten met genormaliseerde naam == `key`, één per
  distinct kwaliteits-tier, oplopend gesorteerd op rank. Binnen een tier wint de
  laagste `stream_id` (deterministisch).
- `pub fn quality_pos(ladder: &[LiveChannel], cur_sid: i64, cur_rank: u8) -> Option<usize>`
  — index van de huidige entry: eerst op `stream_id`, anders op matchende tier-rank.
- `pub fn quality_label(rank: u8) -> &'static str` — UHD/FHD/HD/(?)/SD, voor de HUD.

CatalogStore:

- `pub fn quality_nav(&self, sid: i64) -> QualityNav { ladder, pos }` — één
  read-lock: vind huidig kanaal op sid, normaliseer, bouw ladder, bereken pos.
  Geeft kleine data terug (geen volledige live-clone).

`app.rs`:

- `fn quality_step(&mut self, dir: i32)`:
  - gate: `current_stream_id` aanwezig, anders toast "kwaliteit: geen live kanaal".
  - `nav = catalog.quality_nav(sid)`; `nav.ladder.len() <= 1` → "kwaliteit: geen andere variant".
  - `pos = nav.pos` (anders return).
  - `new = pos + dir`; `< 0` → "al op laagste"; `>= len` → "al op hoogste";
    anders `target = ladder[new]`; `zap_to(target.sid, &target.name, self.current_idx)`.
- `handle_keys`: `plus`/`minus` bools (text-events `"+"`/`"-"`) → `quality_step(±1)`.
  Beschermd door de bestaande early-returns (zoekbalk / news-picker / help).
- debug HUD: regel `quality: {pos+1}/{len} ({label})` als er een ladder is.

Docs: in-app help-keymap (`app.rs` ~1654) + README KEYS (`build-zip.ps1`):
`+ / -  hogere / lagere kwaliteit`.

## Data-flow

toets `+` → `handle_keys` ziet `Text("+")` → `quality_step(1)` →
`catalog.quality_nav(current_sid)` → stap → `zap_to(target)` → toast
`[TV] <naam … FHD/UHD>`.

## Tests (TDD, pure in `catalog.rs`)

- `quality_ladder`: alleen live-varianten van de key; andere kanalen uitgesloten;
  archive-twins (`tv_archive == 1`) uitgesloten; één per tier; oplopend gesorteerd;
  enkele variant → len 1; dubbele same-tier gecollapst (laagste sid wint).
- `quality_pos`: vindt op sid; valt terug op rank als sid afwezig; `None` als geen van beide.
- `quality_label`: rank → label.

## Edge cases

- `current_stream_id` gezet maar geen live catalog-entry (news/film-label) →
  lege ladder → "geen andere variant" (graceful).
- `+`/`-` inert in zoekbalk / news-picker / help (early returns bestaan al).
- `zap_to` re-kickt EPG (zelfde kanaal) — async, non-blocking, acceptabel.

## Geraakte bestanden

- `catalog.rs`: `quality_rank` → `pub`; + `quality_ladder`, `quality_pos`,
  `quality_label`, `QualityNav`, `CatalogStore::quality_nav`; + tests.
- `app.rs`: `quality_step`; `+`/`-` key-bools + dispatch; help-keymap regel; HUD-regel.
- `build-zip.ps1`: README KEYS-entry.
- dedup / `events.rs`: ongemoeid.
