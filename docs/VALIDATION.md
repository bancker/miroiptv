# tvplayer manual validation checklist

Run after `build-zip.ps1` + `verify.ps1` pass. Use a known-good Xtream portal.

## Setup
- [ ] Extract `dist\tvplayer-v*.zip` to a fresh folder
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
- [ ] CPU stays < 10% (Task Manager check)
- [ ] Memory steady (no growth over 100 MB after 30 min)

## Crash safety
- [ ] Bad Xtream cred (`--xtream nope:bad@127.0.0.1:1`) → boots to black window with error toasts, NOT a crash
- [ ] Tune to a non-existent channel ID → toast surfaces error, app stays alive
