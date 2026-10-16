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
- [ ] Digits `0`-`9` are car-radio presets and start empty: tapping an empty one says "sneltoets N is leeg - houd N ingedrukt om op te slaan"
- [ ] Hold a digit (about half a second) on a channel → toast "sneltoets N opgeslagen: <zender>"; tap it later → tunes to that channel within 1 second, toast appears; it survives a restart
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
- [ ] Bronnen: Test with a wrong password → "Gebruiker of wachtwoord onjuist"; Save with the right one → "Verbonden: N zenders", or just "Verbonden" when the channel list is slow to arrive; `BongoPlayer.ini` then holds the line `portal=<user>:<password>@<server>:<port>`
- [ ] Bronnen: a pasted server such as `http://host:8080/` (with a line break after it) connects like `host:8080`; a busy portal (HTTP 503 or 509) reads "Server niet bereikbaar", not "Geen geldige portal"
- [ ] Started with `--xtream user:pass@host:port`: Settings → Bronnen shows a note under the fields that the portal comes from the command line and that what is saved there only counts without that option; after a Save the note is gone
- [ ] Controls: reverse zapping → arrow up and wheel up go to the next channel; the help shows it
- [ ] `?` opens the help; its `o` row is underlined and a click on it opens the settings; in a small window the help scrolls with the wheel, a thin marker shows the position, and Tab stops only on the `o` row

## Bronnen (M3U)
- [ ] `o` → Bronnen with one portal: the form shows the portal (Name, Server, User, Password, Test, Save) and the list below it marks it "in gebruik"; Test and Save work as before
- [ ] Nieuwe bron → M3U → `https://iptv-org.github.io/iptv/index.m3u` → Opslaan: "bron opgeslagen"; the portal stays in use; Actief maken: the list loads (about 11,000 channels), up/down zaps in the list's own order, `f` finds a channel, Shift+F and `*` keep favorites of this source only
- [ ] Quit and start again: the list shows at once (the saved one), the last channel resumes, the debug log says `m3u: saved list of source <id>` and then `m3u: list of source <id> read` or `… unchanged`
- [ ] A channel of the list with an `#EXTVLCOPT` User-Agent plays; a dead one says "streamfout op <naam>" and the app stays on it; `g` opens the channel list (see Zenderlijst)
- [ ] Zap from a playing channel to a dead one: the picture goes black under the error toast, not the picture of the channel before it frozen under the new name; a channel that played and then dies keeps its own last frame
- [ ] Back to the portal with Actief maken: the portal's favorites, presets and last channel are back; the guide works
- [ ] An M3U address with a password (`https://user:pass@…`): nothing of it in the log (`%APPDATA%\BongoPlayer\config\log\`), only `source: m3u <id> (<host>)`
- [ ] A `http://…/get.php?username=…&password=…` link typed as an M3U address, then Opslaan: the question offers an Xtream source; "Als Xtream-bron" fills Server, User and Password. The same link with `https://` stays an M3U list, with no question
- [ ] A list in a file (`C:\…\lijst.m3u`) loads; remove the file and start again: the empty state says "zenderlijst ophalen mislukt" with its path
- [ ] A video file (over 128 MB, or any with a NUL byte in its first KB) saved as an M3U address: "dit is geen zenderlijst", at once; the next start is as quick as ever
- [ ] Verwijderen is disabled for the source in use; for another source it is one button that needs a second press (the first turns it into "Echt verwijderen (met favorieten)?") and takes that source's favorites, presets, last channel and saved list along
- [ ] First start without any source: the setup prompt's last line is a link to Bronnen with a new M3U list ready; typing there types, it fires no hotkey. With a 0.6.1 `favorites.json` in the config folder, a list saved there is source 2 (`source=2;m3u;…` in the ini) and shows none of those favorites
- [ ] Downgrade check: with a portal and an M3U list saved, BongoPlayer 0.6.1 still starts on the portal

## Zenderlijst (M3U, g)
- [ ] On an M3U source `g` opens the channel list over the picture, which keeps playing: groups on the left (Alle zenders, Favorieten when the source has favorites in the list, the list's groups A–Z, Zonder groep when a channel has none, Defecte zenders when channels are marked), each with its count; the group's channels A–Z on the right ("Sport 2" before "Sport 10"); "Één" sorts among the E's, not after Z; a ▶ on the channel that plays; long names end in "…"; one line of keys at the bottom
- [ ] The list opens in the group used last this session (the first time: Alle zenders), on the channel that plays when that group holds it; opening starts nothing
- [ ] Up/Down: one press starts the channel behind the list at once; quick presses or a held key only move the selection, and the channel starts where you stop; at the first and the last row the selection stops; PgUp/PgDn a page, Home/End the ends, at the same tempo
- [ ] Left/Right: the group before or after, nothing starts. Enter: the list closes and the chosen channel plays (at once, also when it waited). Esc or g: the list closes, what plays goes on, a waiting start is dropped
- [ ] Mouse: a click on a group opens it; a click on a channel starts it; a double click starts it and closes the list; the wheel scrolls without starting anything
- [ ] While the list is open no "[TV] …" toast shows, but "streamfout op …" does; f, o, ?, n and * do nothing
- [ ] After closing, Up/Down zap on from the channel that plays, in the M3U file's order
- [ ] On an Xtream portal `g` is still the guide (see Guide (g))
- [ ] A dead channel, then within 2 minutes one that plays from another server: after the check of its address (up to 5 s) the dead one shows in "Defecte zenders" with how long ago it failed, zapping skips it, `f` shows it with "defect", and Shift+F does too when it is a favorite; the log says `m3u: <naam> (stream_id <id>) failed before its first picture` and `… marked defect (http 404)` (or another status of 400 or more except the busy 429, 503 and 509, which mark nothing; or `connect failed`, `timeout`), never an address; `bad_streams-<id>.json` (or `bad_streams.json` for a list that is source 1) in `%APPDATA%\BongoPlayer\config\` holds its id and time, no name and no address
- [ ] A dead channel, then within 2 minutes one that plays from the same server (a paid list: one account for every channel): the dead one is not marked and its server is not asked; the log says `… not checked (the server of what plays), not marked`
- [ ] A dead channel whose server answers the check busy (429, 503 or 509), then within 2 minutes one that plays from another server: the dead one is not marked; the log says `… not checked (busy, http 503), not marked` (with its own status)
- [ ] Pull the network cable, zap through a few channels (they fail), wait more than 2 minutes, plug it back and zap: nothing is marked
- [ ] Pull the cable for less than 2 minutes, zap through a few good channels (they fail), plug it back and zap to one that plays: none of them is marked; the log says `… answers again, not marked` for each (one on the server of the channel that plays says `… not checked (the server of what plays), not marked` instead)
- [ ] A marked channel played by a preset, the search, a favorite or the list plays; once it shows a picture its mark is gone (`… plays again, mark cleared`); while it plays, Up/Down go on from its place in the file
- [ ] Quit and start again: the marks are still there; a mark older than 7 days is gone after the start (`m3u: <n> defect marks expired`)
- [ ] A channel that never shows a picture and never errors: after about 25 s "streamfout op …", no "herverbinden"
- [ ] Delete the source in Bronnen: its bad_streams file goes too
- [ ] On iptv-org's list (about 11,000 channels) the list opens without a visible pause, and zapping is as fast as before

## Adult content
- [ ] Locked (after every start): zapping, search, guide, favorites and presets show no adult channels or films
- [ ] Only the portal's adult flag counts, never a name: while locked a film such as xXx (2002), or a channel whose name starts with XXX but carries no flag, still shows in search and zapping
- [ ] Unlock: the first time asks to choose a PIN (twice); later it asks for the PIN; then adult content is visible until the app closes
- [ ] 3 wrong PINs → wait 30 s
- [ ] A wrong PIN, or one that is not 4 digits, or two new PINs that differ: the dialog stays open with the note, its fields empty and the cursor in the first; the right PIN closes it; Esc cancels. After the 3rd wrong PIN the dialog stays, with OK disabled, until the 30 s are over
- [ ] Lock while adult content plays → playback stops, the picture goes black, "Vergrendeld"
- [ ] Close the app while adult content plays → the next start does not resume it
- [ ] A channel the portal flagged adult since it was last watched (its `last_watched.json` still says not adult): the start resumes it, and when the catalog has loaded it stops with "Vergrendeld"; the next start does not resume it
- [ ] Forgot PIN: the portal password clears it
- [ ] Saving another portal (Settings → Bronnen) while adult content plays stops playback
- [ ] In the PIN dialogs Enter moves to the next field and Esc cancels the open dialog first
- [ ] After 3 wrong PINs and Forgot PIN the 30 s wait is gone (a new PIN can be chosen at once)
- [ ] A bare adult stream URL on the command line (`BongoPlayer.exe <url>`) → the next plain start does not resume it

## News rules
- [ ] Settings → News lists the 7 default rules, each with "gevonden: …" or "niet gevonden"
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

## Guide (g)
- [ ] `g` opens the guide with only the channels that have programme information: no empty columns; each column shows its logo, or its badge and name
- [ ] g while a guide channel plays: the guide opens on its column, near the middle (for the first and the last channels as near as the edges allow)
- [ ] g while a channel without catch-up plays: the guide opens where it was, with the toast "<naam> heeft geen gids"
- [ ] A held g opens the guide once: it does not flicker open and shut
- [ ] Every column lines up the programme on now on one line, with a progress bar; one LIVE programme per column, no doubles
- [ ] The wheel over a column scrolls only that column; Up/Down move the cursor in the chosen column; Left/Right change column; PgUp/PgDn (or ‹ ›) change page
- [ ] While you browse a column, new EPG arriving for it keeps your programme; a column you did not touch jumps to now
- [ ] `d` (or the date button) opens the date list; another day jumps to 19:00; "Nu & straks" (`n`) goes back to now; `p` = 19:00 on the chosen day
- [ ] `/` (or a click) puts the keyboard in the filter box; n, p and g typed there switch nothing; a channel name or a programme title filters; Esc leaves the box, Esc again clears the filter; a filter without matches says "niets gevonden"
- [ ] `f` in the guide opens the search over it (Shift+F does nothing there); Esc returns to the guide as it was; a film or channel pick plays and closes the guide; a series pick shows the toast and leaves the guide open
- [ ] Enter on now plays live; on a past programme the wait screen shows, then the programme from its start; Esc on the wait screen goes back to the guide; a programme still to come says when it starts; one too old is dimmed and says "niet terug te kijken"
- [ ] Close the app with the guide loaded: the next start opens the guide complete at once (`%APPDATA%\BongoPlayer\config\cache\epg-<host>_<port>.json`)
- [ ] Locked: no adult columns; unlocked: they show; lock, then `g`: they are gone
- [ ] After F5 (catalog reload) the guide shows the new channel list without a restart

## Catch-up
- [ ] `g` → pick a finished programme on an archive channel → picture within ~4 s (log: `relay: catch-up playlist in <n> ms`, loadfile `http://127.0.0.1:<port>/vod/<n>.m3u8`)
- [ ] Left/right arrow seek ±30 s within it
- [ ] Then zap back to live → live plays, no error toast
- [ ] A stream the portal refuses (RTL 4 UHD, `HTTP 407`) → the channel's FHD plays within ~1 s with toast "[TV] … (UHD niet beschikbaar)"; a preset on it goes straight to FHD from then on
- [ ] Restart (resumes the last channel) → arrow up/down goes to its neighbour, not to the end of the list

## Subtitles / audio
- [ ] On a live channel with one audio track (Bloomberg on an M3U list), press `a` → "één audiospoor", the sound stays; zap on → the next channel has sound
- [ ] On a VOD with several audio tracks, press `a` → "audiospoor 2/3: deu", again → the next, after the last the first again, never silent; zap to a live channel → it has sound (a track choice is per stream, log `audio and subtitles back to auto for another stream`); a stream without audio says "geen audiospoor"
- [ ] On a stream with subtitles, press `s` → cycles / disables; the next channel starts with its own default

## VOD seek
- [ ] On a movie (search "matrix" → enter), play
- [ ] Left arrow → seeks -30s
- [ ] Right arrow → seeks +30s
- [ ] Space → pauses (toast "|| pauze"); space again → plays on. Same for catch-up
- [ ] Paused, zap to a live channel → it plays (not paused); space on live → toast only, keeps playing

## Window
- [ ] F11 → fullscreen toggles
- [ ] `t` (borderless): hold the right mouse button and drag: the window follows; release stops it; nothing moves in full screen, maximized (Win+Up) or with a border; a left or middle click does nothing to the window. The debug log has a `window drag: begins` line and a `window drag: ended (reason)` line per drag
- [ ] Borderless drag with Ctrl + / Ctrl - zoom: the window still follows the cursor one to one
- [ ] Borderless drag across a second monitor with another scale: the window follows; at most one hop at the crossing
- [ ] Borderless: F11 while holding the right button goes full screen and the window does not also move; Alt+Tab while holding it stops the drag; flick the cursor out of the window and release outside it: the drag stops
- [ ] Swapped (left-handed) mouse buttons: the drag is held by the physical left button, and letting go of that one stops it
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
