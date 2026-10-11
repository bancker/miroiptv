# End-to-end tests

Runs the real Bongo Player against a mock Xtream portal, in a throw-away
config folder (`BONGO_CONFIG_DIR`, `BONGO_INI`), so your own settings, PIN and
credentials are never touched.

Once: `.\make-segments.ps1` (needs ffmpeg). Then build the release exe
(`cargo build --release` in `rust/`) and run scenarios, e.g.:

    .\run-scenario.ps1 -Resume 3 -Expect 'relay: first bytes'

See the parameter block in `run-scenario.ps1` and the docstring in
`mock_portal.py` for what the mock can do.

Before committing: `bash check-staged.sh` (fails if the staged diff holds the
real portal credentials).
