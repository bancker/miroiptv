# Bongo Player manual validation checklist

Run after `rust\build-zip.ps1` + `rust\verify.ps1` pass. Use a known-good Xtream portal.

## Setup
- [ ] Extract `rust\dist\BongoPlayer-v*.zip` to a fresh folder
- [ ] Edit `run.bat`: set `XTREAM_CREDS=user:pass@host:port`
- [ ] Double-click `run.bat`

## Smoke
- [ ] Window opens within 3 seconds
- [ ] No error toasts appear during startup
- [ ] Catalog loads (debug HUD with `d` shows "catalog: loaded" after ~2s)

## Zap
- [ ] Press `1` → tunes to NPO 1 within 1 second, toast appears
- [ ] Press `2`, `3` → tunes to NPO 2, NPO 3
- [ ] Mouse wheel up/down: cycles channels
- [ ] Arrow up/down: cycles channels
- [ ] Each zap shows a toast with channel name immediately
- [ ] Hold arrow down ~2 s → names flip in the toast, the picture stays on the old channel; on release it tunes once to the channel shown (log: one `zap: ->` for the first press, one for the release, no `HTTP 503`)
- [ ] Tap arrow down 3× quickly → tunes once, to the third channel

## News
- [ ] `n` → tunes to an NPO channel
- [ ] `r` → tunes to an RTL channel

## Search
- [ ] Press `f` → search box appears top-center
- [ ] Type "matrix" → at least one [FILM] result
- [ ] Press Enter on a [LIVE] result → tunes to that channel
- [ ] Esc dismisses search

## EPG
- [ ] After tuning, press `e` → current programme strip appears bottom
- [ ] Strip shows current title with HH:MM-HH:MM range
- [ ] Press `e` again → strip disappears
- [ ] Shift+E → EPG grid shows full schedule

## Favorites
- [ ] On a channel, press `*` → toast "added: <name>"
- [ ] Press Shift+F → panel shows favorite
- [ ] Click the favorite → tunes to it
- [ ] Press `*` again on that channel → toast "removed"
- [ ] Restart app → favorites persist (still in Shift+F list)

## Catch-up
- [ ] `g` → pick a finished programme on an archive channel → picture within ~4 s (log: `relay: catch-up playlist in <n> ms`, loadfile `http://127.0.0.1:<port>/vod/<n>.m3u8`)
- [ ] Left/right arrow seek ±30 s within it
- [ ] Then zap back to live → live plays, no error toast (a `HTTP 407` in the log may only delay it)

## Subtitles / audio
- [ ] On a VOD with multiple audio tracks, press `a` → cycles
- [ ] On a stream with subtitles, press `s` → cycles / disables

## VOD seek
- [ ] On a movie (search "matrix" → enter), play
- [ ] Left arrow → seeks -30s
- [ ] Right arrow → seeks +30s

## Window
- [ ] F11 → fullscreen toggles
- [ ] Drag bottom-right to resize → video scales, no crash
- [ ] Alt-F4 → clean exit, no crash report

## Sustained playback
- [ ] Leave a live stream running for 30 minutes
- [ ] No audio crackle, no AV drift, no buffer-exhausted toasts
- [ ] No freeze and no "herverbinden" toast; the log shows live loads as `loadfile http://127.0.0.1:<port>/live/<n>.ts` and at most `relay: ... recovered` lines
- [ ] CPU stays < 10% (Task Manager check)
- [ ] Memory steady (no growth over 100 MB after 30 min)

## Upgrade from tvplayer
- [ ] With old data in `%APPDATA%\tvplayer\config` and no `%APPDATA%\BongoPlayer`: first start → Shift+F shows the old favorites, resume/presets work
- [ ] `tvplayer.ini` next to `BongoPlayer.exe` → renamed to `BongoPlayer.ini`, portal connects without the setup prompt
- [ ] Log (`%APPDATA%\BongoPlayer\config\log\BongoPlayer.log.*`) says "migrated N files from the tvplayer config dir"

## Crash safety
- [ ] Bad Xtream cred (`--xtream nope:bad@127.0.0.1:1`) → boots to black window with error toasts, NOT a crash
- [ ] Tune to a non-existent channel ID → toast surfaces error, app stays alive
