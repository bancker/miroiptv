# Bongo Player manual validation checklist

Run after `rust\build-zip.ps1` + `rust\verify.ps1` pass. Use a known-good Xtream portal.

## Setup
- [ ] Extract `rust\dist\BongoPlayer-v*.zip` to a fresh folder
- [ ] Double-click `run.bat`: the first start asks for the language and then for the portal (server, user, password); `BongoPlayer.ini` appears next to the exe

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
- [ ] `n` → a "Laatste nieuws" list of recent bulletins opens (channels and title patterns come from the news rules, see "News rules"); arrows choose, Enter plays one (catch-up; a 24 h news channel plays live), Esc closes
- [ ] Hold `n`: one list comes up and the log shows one `news picker N items` line, not one per key repeat; a list still on its way when Lock, a portal save or the settings come first does not pop up

## Search
- [ ] Press `f` → search box appears top-center
- [ ] Type "matrix" → at least one [FILM] result
- [ ] Press Enter on a [LIVE] result → tunes to that channel
- [ ] Arrow down / up → moves the highlighted result; Enter plays the highlighted one (film or channel)
- [ ] After the arrows, keep typing → the letters land at the end of the query
- [ ] Esc dismisses search

## Settings (o) and language
- [ ] First start (no `settings.json`): a language screen in three languages, the Windows language preselected; Enter picks it and the last channel then plays
- [ ] `o` opens the settings; Up/Down choose a section, Enter opens it, Esc goes back; Esc or `o` in the list closes
- [ ] Switch the language: every text changes at once (help, toasts, guide, settings)
- [ ] Portal: Test with a wrong password → "Gebruiker of wachtwoord onjuist"; Save with the right one → "Verbonden: N zenders", or just "Verbonden" when the channel list is slow to arrive; `BongoPlayer.ini` then holds the line `portal=<user>:<password>@<server>:<port>`
- [ ] Portal: a pasted server such as `http://host:8080/` (with a line break after it) connects like `host:8080`; a busy portal (HTTP 503 or 509) reads "Server niet bereikbaar", not "Geen geldige portal"
- [ ] Started with `--xtream user:pass@host:port`: Settings → Portal shows a note under the fields that the portal comes from the command line and that what is saved there only counts without that option; after a Save the note is gone
- [ ] Controls: reverse zapping → arrow up and wheel up go to the next channel; the help shows it
- [ ] `?` opens the help; its `o` row is underlined and a click on it opens the settings; in a small window the help scrolls with the wheel, a thin marker shows the position, and Tab stops only on the `o` row

## Adult content
- [ ] Locked (after every start): zapping, search, guide, favorites and presets show no adult channels or films
- [ ] Only the portal's adult flag counts, never a name: while locked a film such as xXx (2002), or a channel whose name starts with XXX but carries no flag, still shows in search and zapping
- [ ] Unlock: the first time asks to choose a PIN (twice); later it asks for the PIN; then adult content is visible until the app closes
- [ ] 3 wrong PINs → wait 30 s
- [ ] Lock while adult content plays → playback stops, the picture goes black, "Vergrendeld"
- [ ] Close the app while adult content plays → the next start does not resume it
- [ ] A channel the portal flagged adult since it was last watched (its `last_watched.json` still says not adult): the start resumes it, and when the catalog has loaded it stops with "Vergrendeld"; the next start does not resume it
- [ ] Forgot PIN: the portal password clears it
- [ ] Saving another portal (Settings → Portal) while adult content plays stops playback
- [ ] In the PIN dialogs Enter moves to the next field and Esc cancels the open dialog first
- [ ] After 3 wrong PINs and Forgot PIN the 30 s wait is gone (a new PIN can be chosen at once)
- [ ] A bare adult stream URL on the command line (`BongoPlayer.exe <url>`) → the next plain start does not resume it

## News rules
- [ ] Settings → News lists the 7 default rules, each with "gevonden: …"
- [ ] Change a pattern; N follows the new rule; "Standaard herstellen" brings the defaults back
- [ ] With many rules the list scrolls and Tab brings each rule into view
- [ ] Removing a rule with the mouse leaves the keyboard where it was (typing afterwards edits no rule)

## EPG
- [ ] After tuning, press `e` → current programme strip appears bottom
- [ ] Strip shows current title with HH:MM-HH:MM range
- [ ] Press `e` again → strip disappears
- [ ] Shift+E → EPG grid shows full schedule

## Favorites
- [ ] On a channel, press `*` → toast "added: <name>"
- [ ] Press Shift+F → panel shows favorite
- [ ] Shift+F right after the start, before the catalog has loaded → "catalogus laden...", no favorite names (adult ones included); once loaded, the favorites show
- [ ] Click the favorite → tunes to it
- [ ] Press `*` again on that channel → toast "removed"
- [ ] Restart app → favorites persist (still in Shift+F list)

## Catch-up
- [ ] `g` → pick a finished programme on an archive channel → picture within ~4 s (log: `relay: catch-up playlist in <n> ms`, loadfile `http://127.0.0.1:<port>/vod/<n>.m3u8`)
- [ ] Left/right arrow seek ±30 s within it
- [ ] Then zap back to live → live plays, no error toast
- [ ] A stream the portal refuses (RTL 4 UHD, `HTTP 407`) → the channel's FHD plays within ~1 s with toast "[TV] … (UHD niet beschikbaar)"; a preset on it goes straight to FHD from then on
- [ ] Restart (resumes the last channel) → arrow up/down goes to its neighbour, not to the end of the list

## Subtitles / audio
- [ ] On a VOD with multiple audio tracks, press `a` → cycles
- [ ] On a stream with subtitles, press `s` → cycles / disables

## VOD seek
- [ ] On a movie (search "matrix" → enter), play
- [ ] Left arrow → seeks -30s
- [ ] Right arrow → seeks +30s
- [ ] Space → pauses (toast "|| pauze"); space again → plays on. Same for catch-up
- [ ] Paused, zap to a live channel → it plays (not paused); space on live → toast only, keeps playing

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
