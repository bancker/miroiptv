# Freeze-detectie via `paused-for-cache` — ontwerp

**Datum:** 2026-06-17
**Status:** ontwerp, lean-first

## Doel

Detecteren wanneer live-video tijdelijk bevriest (buffer-underrun) en dat
verifieerbaar maken: elke freeze altijd loggen, en in het debug-paneel (toets
`d`) de live-status + tellers tonen. Dit is tevens de basis voor een later
alternatief (recovery/UX-pad) dat op dezelfde detectie kan haken.

## Scope (lean-first)

- **IN:** volledige bevriezingen = mpv `paused-for-cache`-episodes
  (event-driven, nul polling tijdens normaal afspelen).
- **UITGESTELD (genoteerde keuze):** dropped-frame stutter/judder zonder
  volledige stilstand. Pas oppakken als freeze-only onvoldoende blijkt. De app
  doet bewust eerst lean.
- Opstart-/zap-buffering telt **niet** mee — alleen freezes ná het eerste frame
  van de huidige stream.
- Tellers zijn **per stream**: ze resetten bij het landen in een stream
  (`load_url`), niet per sessie.

## Niet-doelen

- De bestaande `check_stall` auto-reload-watchdog blijft ongewijzigd. Die is
  complementair: de watchdog *herstelt* (herlaadt na ~10s), deze feature *meet
  en toont*.
- Geen continue polling van cache-seconden/buffer% — dat zou een per-frame
  firehose aan events zijn, ook bij normaal beeld. De live-timer leiden we af
  uit de flag-flips + de UI-klok.

## Architectuur

Nieuw mini-module `rust/src/playback_health.rs` — pure logica, geen egui/mpv-
afhankelijkheid, los unit-testbaar. Dit is het schone aanhechtpunt voor het
toekomstige alternatief.

```rust
struct PlaybackHealth {
    buffering: bool,                       // rauwe live-flag (paused-for-cache)
    started: bool,                         // eerste frame van huidige stream gezien
    current_freeze_start: Option<Instant>, // Some alleen voor een getelde freeze
    freeze_count: u32,
    total_frozen: Duration,
    last_freeze: Option<Duration>,
}
```

API:

- `reset()` — nieuwe stream: alles op nul, `started = false`.
- `mark_started()` — `started = true` (idempotent).
- `on_buffering_changed(now: Instant, on: bool)`
  - naar `true`: `buffering = true`; als `started` → `current_freeze_start = Some(now)`
    (opstart, `started == false`, wordt genegeerd voor tellen).
  - naar `false`: `buffering = false`; als `current_freeze_start` gezet was →
    `d = now - start; count += 1; total += d; last = Some(d)`.
- `elapsed(now) -> Option<Duration>` = `current_freeze_start.map(|s| now - s)`.
- accessors: `is_buffering()`, `freeze_count()`, `total_frozen()`, `last_freeze()`.

## Data-flow (via bestaande kanalen)

1. `player/mod.rs` spawn (na `RenderCtx::new`):
   `mpv.observe_property("paused-for-cache", MPV_FORMAT_STRING, 1)`.
2. mpv → `MPV_EVENT_PROPERTY_CHANGE` → bestaande handler (`mod.rs:147`) →
   `Event::PropertyChanged { name, value }`. **Ongewijzigd** — de STRING-parser in
   `events.rs:84` levert `"yes"`/`"no"`.
3. `app.rs`:
   - `load_url()`: `self.health.reset()`.
   - `Event::PlaybackStarted`: `self.health.mark_started()`.
   - `Event::PropertyChanged` met `name == "paused-for-cache"`:
     `on = value == "yes"`; transitie loggen; `health.on_buffering_changed(Instant::now(), on)`.
4. `paint_debug_hud`: live-status + tellers renderen.

## Logging (altijd aan, los van de HUD)

- freeze-start (geteld): `info!("playback froze (cache underrun)")`
- freeze-eind (geteld): `info!("playback resumed after {:.1}s — freeze #{n}, total {:.1}s")`
- opstart-buffering: alleen `debug!` (niet geteld).

## Debug HUD (alleen als `show_debug`)

Twee regels toegevoegd aan `paint_debug_hud`.

Regel 1 — live playback-status, drie varianten:

```
playback: ● live                 (niet aan het bufferen)
playback: ⏳ BUFFERING  2.3s      (getelde freeze; 2.3s loopt live op via elapsed(now))
playback: ⏳ buffering (startup)  (opstart/zap, started == false, niet geteld)
```

Regel 2 — tellers (altijd), `last` toont `—` zolang er nog geen freeze was:

```
freezes: 3   total 11.4s   last 4.1s
freezes: 0   total 0.0s    last —
```

## Edge cases

- Handmatige pauze gebruikt `pause`, niet `paused-for-cache` → telt niet mee.
- Rebuffer ná een handmatige seek (VOD/catch-up) kan meetellen (`started` is dan
  al `true`). Acceptabel voor lean; later verfijnen als het ruis geeft.
- Verlate flag-events over een zap heen worden onderdrukt (`started == false` na
  `reset()` tot het nieuwe eerste frame).

## Testen

Unit-tests in `playback_health.rs`:

- enkele freeze → `count == 1`, `last == duur`, `total == duur`.
- meerdere freezes accumuleren `count`/`total`, `last` volgt de laatste.
- opstart-buffering (geen `mark_started`) → niet geteld.
- `mark_started()` daarna freeze → wél geteld.
- idempotent: tweemaal naar `true` start niet dubbel; naar `false` zonder lopende
  freeze is een no-op.
- `elapsed()` is `Some` tijdens getelde freeze, anders `None`.
- `reset()` zet tellers en `started` terug.

## Geraakte bestanden

- **NIEUW** `rust/src/playback_health.rs`
- `rust/src/lib.rs` — `pub mod playback_health;` (alfabetisch tussen `favorites` en `player`)
- `rust/src/player/mod.rs` — één `observe_property`-regel
- `rust/src/app.rs` — veld, `reset()` in `load_url`, twee event-armen, twee HUD-regels
- `rust/src/player/events.rs` — **ongewijzigd**
