# End-to-end tests

Runs the real Bongo Player against a mock Xtream portal, in a throw-away
config folder (`BONGO_CONFIG_DIR`, `BONGO_INI`), so your own settings, PIN and
credentials are never touched.

Once: `.\make-segments.ps1` (needs ffmpeg). Then build the release exe
(`cargo build --release` in `rust/`) and run scenarios, e.g.:

    .\run-scenario.ps1 -Resume 3 -Expect 'relay: first bytes'

See the parameter block in `run-scenario.ps1` and the docstring in
`mock_portal.py` for what the mock can do.

`run-scenario.ps1` and `measure-startup.ps1` refuse to start while Tab, Shift,
Ctrl, Alt, Enter, Esc, Space or an arrow key is held down (`held-keys.ps1`):
winit turns a key that Windows reports as down into a key press when the window
gets the focus, which would make a run depend on the keyboard's state. Press and
release the key once and run again. `test-held-keys.ps1` checks that guard against
a fake keyboard (nothing is pressed or sent).

Keys are posted to the window as messages (key down, the character, key up),
never injected, and winit takes them like real ones. Two things follow. A `TEXT:`
step ends with a 300 ms pause, because egui handles the events of one frame
together and a Tab moves the focus first: a character typed in the frame of the
next Tab lands in the next field. Real keys obey that rule too, but a person's
gap before a Tab is far longer than a frame, while the harness types about one
character per frame. And about one posted character in a hundred gains a second
one: winit's message loop calls `TranslateMessage` on the posted key
down, which queues a `WM_CHAR` of its own, and when winit takes the down before
the posted up is queued it adds both characters to that key's text
(`text: Some("ss")` in a log made with `RUST_LOG=eframe=trace`). It only hurts
scenarios that type into a field, the setup prompt's login above all (an extra
character is a `BadLogin` or an unreachable host), so run such a scenario once
more before looking for a bug in the app.

Before committing: `bash check-staged.sh` (fails if the staged diff holds the
real portal credentials).

## Regression set

`regression.ps1` runs the keyboard-only scenarios of 0.6.0 part 1 (and, from part 2 on, the guide's
group `G`) through `run-scenario.ps1`, one JSON line per scenario in `%TEMP%\bongo-regression`.
`-Group` picks groups, `-Only` scenario ids (wildcards), `-DryRun` prints the commands, `-Summary`
sums up the last results. A scenario that fails is run once more; one that the held-key guard
refuses waits 10 s and tries again (up to 3 times). Typing scenarios can fail once on a doubled
character (see above), so a pass on the re-run counts.
