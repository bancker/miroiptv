<#
The regression set of 0.6.0 part 1: the keyboard-only end-to-end scenarios that passed when part 1
was released (0.5.2). Each run opens the app's window once; run it when nobody minds windows
coming and going.

  powershell -NoProfile -ExecutionPolicy Bypass -File e2e\regression.ps1 -Group R      # one group
  powershell -NoProfile -ExecutionPolicy Bypass -File e2e\regression.ps1 -Group R,B,C,N,X
  powershell -NoProfile -ExecutionPolicy Bypass -File e2e\regression.ps1 -Summary

Groups
  R  the R49 re-runs: first-focus scenarios with commands that pass on a normal keyboard after item I.1
  B  the brief scenarios of Tasks 12-17 (Task 18 report, section 3.3), Tab counts / PIN dialogs updated
  C  scenarios whose commands changed with this wave (the PIN dialogs stay open: T16-D, R36, L; R41ab)
  N  new scenarios for the fixes (A, C, D, F, G, H, J, L, M)
  X  the other extras of the Task 12-17 reports that run through run-scenario.ps1 alone

Everything goes through rust\e2e\run-scenario.ps1 (mock portal, isolated config folder). No mouse, no
SendInput: keys are posted to the app's window by the harness. The script refuses to start while a key
is held (the harness does too). Results: one JSON line per scenario in $Results, the harness output of a
failing scenario in $Results\<ID>.txt.
#>
param(
    [string[]]$Group = @('R', 'B', 'C', 'N'),
    [switch]$DryRun,    # print the command lines (and check the parameter names), run nothing
    [string[]]$Only,    # run only the scenarios whose id matches one of these -like patterns (keep a
                        # scenario that depends on an earlier one, e.g. T12-2 and T12-2b, in one batch)
    [switch]$Summary,   # run nothing: summarise results.jsonl (the latest row per id) and exit
    [string]$Results = (Join-Path $env:TEMP 'bongo-regression')
)
$ErrorActionPreference = 'Stop'
# powershell -File passes -Group R,B and -Only 'a*','b' as ONE string each: split them here.
$Group = @($Group | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
if ($Only) { $Only = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ }) }
Set-Location (Join-Path $PSScriptRoot '..')
New-Item -ItemType Directory -Force $Results | Out-Null
$jsonl = Join-Path $Results 'results.jsonl'

# ---- the variables of the Task 16 / 17 reports -------------------------------------------------
$S   = '{"language":"nl","pin":{"salt":"07070707070707070707070707070707","hash":"e1a48ec3f020a4ce7f6374c7e656ef713d164ebea476defc93842f97b85194c4"}}'   # PIN 1234
$A   = @{ MOCK_ADULT = '1' }
$AL  = @{ MOCK_ADULT = '1'; MOCK_LATENCY = '1' }
$N   = @{ MOCK_NEWS = '1' }
$unlock = 'o;DOWN;DOWN;DOWN;ENTER;ENTER;TEXT:1234;ENTER;SLEEP:500;ESC;ESC'
$lock   = 'o;DOWN;DOWN;DOWN;ENTER;ENTER;SLEEP:500;ESC;ESC'
$save   = 'o;DOWN;ENTER;TAB;TAB;TAB;TAB;ENTER;SLEEP:2000'     # Portal > Save with the prefilled mock credentials
$ui = (Get-UICulture).TwoLetterISOLanguageName
$want = if ($ui -in 'nl', 'de') { $ui } else { 'en' }            # the language the first-start screen preselects

function Fresh([string]$name) {
    $d = Join-Path $env:TEMP "bongo-regression-$name"
    if (Test-Path -LiteralPath $d) { Remove-Item -LiteralPath $d -Recurse -Force }
    $d
}

function Show-Value($v) {
    if ($v -is [hashtable]) { '@{ ' + (($v.psbase.Keys | Sort-Object | ForEach-Object { "$_ = '$($v[$_])'" }) -join '; ') + ' }' }
    elseif ($v -is [array]) { ($v | ForEach-Object { "'" + ("$_" -replace "'", "''") + "'" }) -join ',' }
    elseif ($v -is [bool]) { '' }
    elseif ($v -is [int]) { "$v" }
    else { "'" + ("$v" -replace "'", "''") + "'" }
}

$runScenario = Join-Path $PSScriptRoot 'run-scenario.ps1'
$script:grp = ''
$script:thisRun = @()

# One harness run. *>&1 so that the guard's Write-Host message is captured too.
function Invoke-Harness([hashtable]$P) {
    $out = ''; $code = 1
    try {
        $out = & $runScenario @P *>&1 | Out-String
        $code = $LASTEXITCODE
    } catch {
        $out = "EXCEPTION: $($_.Exception.Message)`n$out"; $code = 1
    }
    [pscustomobject]@{ Out = $out; Code = $code; Refused = [bool]($out -match 'refusing to start - ') }
}

# A scenario: PASS when the harness exits 0 and $Check (if any) holds.
#  - the held-key guard refused (the user may be typing): wait 10 s and retry, up to 3 times; those are
#    counted as "refused", not as fails;
#  - a fail is re-run once; it passes (on re-run) or fails for good, with both outputs kept.
function Scn([string]$Id, [hashtable]$P, [scriptblock]$Check = $null) {
    if ($DryRun) {
        # (not .Keys: run-scenario.ps1 has a parameter called Keys)
        $known = (Get-Command $runScenario).Parameters.GetEnumerator() | ForEach-Object { $_.Key }
        $bad = @($P.psbase.Keys | Where-Object { $_ -notin $known })
        if ($bad) { throw "$Id : run-scenario.ps1 has no parameter $($bad -join ', ')" }
        $line = '.\e2e\run-scenario.ps1'
        foreach ($k in ($P.psbase.Keys | Sort-Object)) { $line += " -$k " + (Show-Value $P[$k]) }
        "# $Id" + $(if ($Check) { '   (+ a check of the files afterwards)' } else { '' })
        $line.TrimEnd()
        return
    }
    if ($Only -and -not ($Only | Where-Object { $Id -like $_ })) { return }
    $t0 = Get-Date
    $refused = 0; $attempts = 0; $pass = $false; $exit = $null; $checked = $true; $why = ''; $ran = $false
    while ($true) {
        $r = Invoke-Harness $P
        if ($r.Refused) {
            $refused++
            if ($refused -le 3) { "refused  $Id (a key is held; waiting 10 s, retry $refused of 3)"; Start-Sleep -Seconds 10; continue }
            $why = 'the held-key guard refused 4 times'
            break
        }
        $ran = $true
        $attempts++
        $exit = $r.Code; $checked = $true; $why = ''
        if ($exit -eq 0 -and $Check) {
            try { $checked = [bool](& $Check) } catch { $checked = $false; $why = $_.Exception.Message }
        }
        $pass = ($exit -eq 0 -and $checked)
        $r.Out | Set-Content -Path (Join-Path $Results "$Id.attempt$attempts.txt") -Encoding UTF8
        if (-not $pass) {
            # keep the app's own log of a failed attempt: a re-run in the same folder clears it
            $m = [regex]::Match($r.Out, 'config folder: (.+?)\s*(\r?\n|$)')
            if ($m.Success) {
                $log = Get-ChildItem (Join-Path $m.Groups[1].Value 'log') -Filter 'BongoPlayer.log.*' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
                if ($log) { Copy-Item $log.FullName (Join-Path $Results "$Id.attempt$attempts.applog.txt") -Force }
            }
            # and what the mock portal saw (its request log is rewritten by the next run)
            $req = Join-Path $PSScriptRoot 'requests-18080.log'
            if (Test-Path $req) { Copy-Item $req (Join-Path $Results "$Id.attempt$attempts.requests.txt") -Force }
        }
        if ($pass -or $attempts -ge 2) { break }
        "again    $Id (failed once: exit $exit, check $checked $why)"
    }
    $status = if (-not $ran) { 'refused' } elseif ($pass -and $attempts -eq 1) { 'pass' } elseif ($pass) { 'pass-on-rerun' } else { 'fail' }
    $row = [ordered]@{ id = $Id; group = $script:grp; status = $status; attempts = $attempts; refused = $refused; exit = $exit; check = $checked; why = $why; seconds = [int]((Get-Date) - $t0).TotalSeconds; at = (Get-Date).ToString('s') }
    ($row | ConvertTo-Json -Compress) | Add-Content -Path $jsonl
    $script:thisRun += [pscustomobject]$row
    $tail = (@(if ($refused) { "refused $refused x, retried" }) + @(if ($status -eq 'pass-on-rerun') { 'on re-run' })) -join '; '
    switch ($status) {
        'pass' { "PASS  $Id" + $(if ($tail) { " ($tail)" } else { '' }) }
        'pass-on-rerun' { "PASS  $Id ($tail)" }
        'refused' { "REFUSED  $Id ($why)" }
        default {
            "FAIL  $Id (exit $exit, check $checked $why)" + $(if ($tail) { " ($tail)" } else { '' })
            ($r.Out -split "`n" | Select-Object -Last 40) -join "`n"
        }
    }
}

function LastWatched([string]$dir) { (Get-Content -Raw -LiteralPath (Join-Path $dir 'last_watched.json')) }
function RequestsMatching([string]$pattern) { @(Get-Content 'e2e\requests-18080.log' -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern }).Count }

# ====================================================================================================
function Group-R {
    'R: the R49 re-runs'
    Scn 'R-T12-1' @{ FirstStart = $true; Resume = 3; Keys = 'ENTER'; KeysAfter = 4; Seconds = 10; Expect = "settings: language set to $want", 'relay: first bytes' }
    Scn 'R-T12-6' @{ FirstStart = $true; Resume = 3; Keys = 'TAB;TAB;TAB;ENTER'; KeysAfter = 4; Seconds = 10; Expect = 'settings: language set to de', 'relay: first bytes'; Forbid = 'language set to en' }
    # The setup prompt: its frame is no Tab stop any more, so the FIRST Tab lands in the Server field.
    Scn 'R-T13-x15' @{ NoPortal = $true; Settings = '{"language":"en"}'; Keys = 'TAB;TEXT:127.0.0.1:18080;TAB;TEXT:tester;TAB;TEXT:secret;ENTER'; Seconds = 14; Expect = 'toast: connecting to portal...' }
    $d4 = Fresh 'prompt-bad'
    Scn 'R-T15-4' @{ NoPortal = $true; ConfigDir = $d4; Keys = 'TAB;TEXT:127.0.0.1:18080;TAB;TEXT:tester;TAB;TEXT:wrong;ENTER'; Seconds = 14; Expect = 'portal: no credentials', 'settings: connection check BadLogin'; Forbid = 'portal saved', 'settings: opened' } { -not (Test-Path (Join-Path $d4 'BongoPlayer.ini')) }
    $d5 = Fresh 'prompt-ok'
    Scn 'R-T15-5' @{ NoPortal = $true; ConfigDir = $d5; Keys = 'TAB;TEXT:127.0.0.1:18080;TAB;TEXT:tester;TAB;TEXT:secret;ENTER'; Seconds = 14; Expect = 'portal: no credentials', 'settings: connection check Ok', 'settings: portal saved'; Forbid = 'settings: opened' } { (Get-Content -Raw (Join-Path $d5 'BongoPlayer.ini')).Trim() -eq 'portal=tester:secret@127.0.0.1:18080' }
}

# ====================================================================================================
function Group-B {
    'B: the brief scenarios of Tasks 12-17'
    # Task 12 brief, step 1
    $dsw = Fresh 'swap'
    Scn 'T12-2' @{ ConfigDir = $dsw; Resume = 10; Keys = 'o;DOWN;DOWN;ENTER;SPACE;ESC;ESC;DOWN'; Expect = 'settings: zap keys reversed', 'zap: -> TEST 09' }
    Scn 'T12-2b' @{ ConfigDir = $dsw; Resume = 10; Keys = 'DOWN'; Expect = 'zap: -> TEST 09' }
    Scn 'T12-3' @{ Resume = 10; Keys = 'o;DOWN;DOWN;1;f;SPACE;ESC;DOWN'; Expect = 'settings: opened', 'settings: closed', 'zap: -> TEST 11'; Forbid = 'zap: -> TEST 12' }
    Scn 'T12-4' @{ Resume = 10; Keys = 'o;SLEEP:300;o;SLEEP:300;DOWN'; Expect = 'settings: closed', 'zap: -> TEST 11' }
    # Task 13 brief
    Scn 'T13-1' @{ Settings = '{"language":"de"}'; Resume = 3; Keys = 'a;5'; Expect = 'toast: n.chste Tonspur', 'toast: Taste 5 ist leer' }
    Scn 'T13-2' @{ Settings = '{"language":"en"}'; Resume = 3; Keys = 'a;5'; Expect = 'toast: next audio track', 'toast: preset 5 is empty' }
    Scn 'T13-3' @{ Resume = 10; Keys = 'o;1;SPACE;ESC'; Expect = 'settings: closed'; Forbid = 'toast: sneltoets', 'toast: pauze' }
    # Task 14 brief
    foreach ($lg in 'nl', 'en', 'de') {
        Scn "T14-help-$lg" @{ Settings = "{`"language`":`"$lg`"}"; Resume = 3; Keys = '?'; Seconds = 15 }
        Scn "T14-guide-$lg" @{ Settings = "{`"language`":`"$lg`"}"; Resume = 3; Keys = 'g'; KeysAfter = 9; Seconds = 18 }
    }
    # Task 15 brief
    Scn 'T15-1' @{ Resume = 3; Keys = 'o;DOWN;ENTER;TAB;TAB;BACKSPACE;BACKSPACE;BACKSPACE;BACKSPACE;BACKSPACE;BACKSPACE;TEXT:wrong;TAB;ENTER'; Seconds = 14; Expect = 'settings: connection check BadLogin'; Forbid = 'portal saved' }
    $dp = Fresh 'portal'
    Scn 'T15-2' @{ ConfigDir = $dp; Resume = 3; Keys = 'o;DOWN;ENTER;TAB;TAB;TAB;TAB;ENTER'; Seconds = 14; Expect = 'connection check Ok', 'portal saved' } { (Get-Content -Raw (Join-Path $dp 'BongoPlayer.ini')).Trim() -eq 'portal=tester:secret@127.0.0.1:18080' }
    Scn 'T15-3' @{ NoPortal = $true; ConfigDir = $dp; Seconds = 10; Expect = 'portal: xtream <user>@127.0.0.1:18080', 'resuming last watched'; Forbid = 'no credentials', 'unreadable credentials', 'placeholder' }
    # Task 16 brief, step 1 (A-E)
    Scn 'T16-A' @{ MockEnv = $A; Settings = $S; Resume = 2; Keys = 'DOWN;SLEEP:1500;f;SLEEP:300;TEXT:adult;SLEEP:300;ENTER'; Expect = 'zap: -> TEST 03'; Forbid = 'zap: -> 24/7', 'zap: -> XXX', '/movie/' }
    Scn 'T16-B' @{ MockEnv = $A; Settings = $S; Resume = 2; Keys = 'o;DOWN;DOWN;DOWN;ENTER;ENTER;TEXT:1234;ENTER;SLEEP:500;ESC;ESC;DOWN'; Expect = 'adult: unlocked', 'zap: -> 24/7 \| ADULT A'; Forbid = 'zap: -> TEST 01', 'zap: -> TEST 03' }
    $dad = Fresh 'adult'
    Scn 'T16-C1' @{ ConfigDir = $dad; MockEnv = $A; Settings = $S; Resume = 2; Seconds = 22; Keys = 'o;DOWN;DOWN;DOWN;ENTER;ENTER;TEXT:1234;ENTER;SLEEP:500;ESC;ESC;DOWN;SLEEP:2500;o;DOWN;DOWN;DOWN;ENTER;ENTER;SLEEP:500;ESC;ESC'; Expect = 'adult: locked', 'player: stop' }
    Scn 'T16-C2' @{ ConfigDir = $dad; MockEnv = $A; Seconds = 8; Expect = 'not resumed'; Forbid = 'loadfile' }
    Scn 'T16-C3' @{ ConfigDir = $dad; MockEnv = $A; Resume = 2; Keys = 'DOWN'; Expect = 'zap: -> TEST 03'; Forbid = 'zap: -> 24/7' }
    # (T16-D, the three wrong PINs, is in group C: its command changed.)
    Scn 'T16-E1' @{ MockEnv = $A; Resume = 901; OldResumeFile = $true; Seconds = 8; Expect = 'not resumed'; Forbid = 'loadfile' }
    Scn 'T16-E2' @{ MockEnv = $A; Resume = 3; OldResumeFile = $true; Expect = 'relay: first bytes' }
    # Task 17 brief, step 1
    Scn 'T17-1' @{ MockEnv = $N; Resume = 3; Keys = 'n'; Expect = 'news picker 1 items', 'NOS NOS Journaal'; Forbid = 'Jeugdjournaal' }
    Scn 'T17-2' @{ MockEnv = $N; Settings = '{"language":"nl","news_rules":[{"channel":"NPO 1","patterns":"*","label":"NPO","skip_secs":0}]}'; Resume = 3; Keys = 'n'; Expect = 'news picker 3 items' }
    Scn 'T17-3' @{ MockEnv = $N; Settings = '{"language":"nl","news_rules":[]}'; Resume = 3; Keys = 'n'; Expect = 'news picker: no rules' }
    Scn 'T17-4' @{ MockEnv = $N; Settings = '{"language":"nl","news_rules":[]}'; Resume = 3; Keys = 'o;DOWN;DOWN;DOWN;DOWN;ENTER;TAB;ENTER;SLEEP:300;ESC;ESC;n'; Expect = 'settings: closed', 'news picker 1 items' }
}

# ====================================================================================================
function Group-C {
    'C: scenarios whose commands changed with this wave'
    # T16-D: the dialog stays open after a wrong PIN, so the next try is typed straight into it.
    Scn 'T16-D' @{ Settings = $S; Resume = 3; Seconds = 16; Keys = 'o;DOWN;DOWN;DOWN;ENTER;ENTER;TEXT:0000;ENTER;SLEEP:300;TEXT:0000;ENTER;SLEEP:300;TEXT:0000;ENTER'; Expect = 'adult: PIN input blocked for 30 s' }
    # R36: three wrong PINs, Esc out of the blocked dialog (the keyboard lands on Forgot PIN), the portal
    # password clears the PIN, then a new PIN is chosen at once.
    Scn 'T16-R36' @{ MockEnv = $A; Settings = $S; Resume = 2; Seconds = 22; Keys = 'o;DOWN;DOWN;DOWN;ENTER;ENTER;TEXT:0000;ENTER;SLEEP:300;TEXT:0000;ENTER;SLEEP:300;TEXT:0000;ENTER;SLEEP:300;ESC;ENTER;TEXT:secret;ENTER;SLEEP:500;ENTER;SLEEP:500;TEXT:2468;TAB;TEXT:2468;ENTER;SLEEP:500'; Expect = 'adult: PIN input blocked for 30 s', 'adult: PIN cleared with the portal password', 'adult: unlocked' }
    # L: blocked while unlocked (3 wrong current PINs in Change PIN), Esc, Lock, then Forgot PIN.
    $wc = 'TEXT:0000;ENTER;TEXT:5555;ENTER;TEXT:5555;ENTER;SLEEP:300'
    Scn 'T16-L' @{ MockEnv = $A; Settings = $S; Resume = 2; Seconds = 28; Keys = "$unlock;o;DOWN;DOWN;DOWN;ENTER;TAB;ENTER;$wc;$wc;$wc;ESC;ENTER;SLEEP:500;ENTER;SLEEP:500;TEXT:secret;ENTER"; Expect = 'adult: PIN input blocked for 30 s', 'adult: locked', 'adult: PIN cleared with the portal password' }
    Scn 'T16-K' @{ Settings = $S; Resume = 2; Keys = 'o;DOWN;DOWN;DOWN;ENTER;ENTER;TEXT:1o2 f3e4n5g6t7a8s9d0;ENTER;SLEEP:500'; Expect = 'adult: unlocked'; Forbid = 'settings: closed', 'news picker', 'pause:', 'randloos', 'ondertiteling', 'audiospoor', 'pauze', 'sneltoets', 'zap: ->' }
    # R41a/b: a bare adult URL on the command line keeps playing, but the file now learns it is adult.
    $du = Fresh 'url'; New-Item -ItemType Directory $du | Out-Null
    [IO.File]::WriteAllText((Join-Path $du 'BongoPlayer.ini'), "portal=tester:secret@127.0.0.1:18080`n")
    Scn 'T16-R41a' @{ ConfigDir = $du; NoPortal = $true; Url = 'http://127.0.0.1:18080/live/tester/secret/901.m3u8'; MockEnv = $A; Seconds = 9; Expect = 'loadfile'; Forbid = 'player: stop' } { (LastWatched $du) -match '"adult":true' }
    Scn 'T16-R41b' @{ ConfigDir = $du; NoPortal = $true; MockEnv = $A; Seconds = 8; Expect = 'not resumed'; Forbid = 'loadfile' }
    # R41c/d: a normal resume still plays at once, and the file keeps saying "not adult".
    $dr = Fresh 'resume'; New-Item -ItemType Directory $dr | Out-Null
    Scn 'T16-R41c' @{ ConfigDir = $dr; MockEnv = $AL; Resume = 3; Seconds = 8; Expect = 'resuming last watched', 'relay: first bytes'; Forbid = 'player: stop' } { (LastWatched $dr) -match '"adult":false' }
    Scn 'T16-R41d' @{ ConfigDir = $dr; MockEnv = $AL; Seconds = 8; Expect = 'resuming last watched', 'relay: first bytes' }
    # Presets seeded into a throw-away folder: 1 and 4 flagged, 2 flagged (XXX in its name), 3 normal.
    $dpr = Fresh 'presets'; New-Item -ItemType Directory $dpr | Out-Null
    [IO.File]::WriteAllText((Join-Path $dpr 'presets.json'), '{"slots":[null,{"stream_id":901,"name":"24/7 | ADULT A"},{"stream_id":902,"name":"XXX | ADULT B"},{"stream_id":3,"name":"TEST 03"},{"stream_id":901,"name":"Renamed"},null,null,null,null,null]}')
    Scn 'T16-Plock' @{ ConfigDir = $dpr; MockEnv = $A; Settings = $S; Resume = 2; Keys = '1;SLEEP:400;2;SLEEP:400;4;SLEEP:400;3'; Expect = 'sneltoets 1 is leeg', 'sneltoets 2 is leeg', 'sneltoets 4 is leeg', 'zap: -> TEST 03'; Forbid = 'zap: -> 24/7', 'zap: -> XXX' }
    Scn 'T16-Punlock' @{ ConfigDir = $dpr; MockEnv = $A; Settings = $S; Resume = 2; Keys = "$unlock;1;SLEEP:800;2;SLEEP:800;4"; Expect = 'adult: unlocked', 'zap: -> 24/7 \| ADULT A', 'zap: -> XXX \| ADULT B'; Forbid = 'is leeg' }
    Scn 'T16-Z1' @{ MockEnv = $A; Settings = $S; Resume = 2; Seconds = 22; Keys = "DOWN;SLEEP:1500;$unlock;SLEEP:300;UP"; Expect = 'zap: -> TEST 03', 'adult: unlocked', 'zap: -> XXX \| ADULT B'; Forbid = 'zap: -> TEST 02' }
    Scn 'T16-Z2' @{ MockEnv = $A; Settings = $S; Resume = 2; Seconds = 30; Keys = "$unlock;DOWN;SLEEP:700;DOWN;SLEEP:700;DOWN;SLEEP:1500;$lock;SLEEP:300;UP"; Expect = 'adult: unlocked', 'zap: -> TEST 03', 'adult: locked', 'zap: -> TEST 02'; Forbid = 'zap: -> TEST 04', 'player: stop' }
    $d1 = Fresh 'save-adult'
    Scn 'T16-R34a' @{ ConfigDir = $d1; MockEnv = $A; Settings = $S; Resume = 2; Seconds = 24; Keys = "$unlock;DOWN;SLEEP:1500;$save"; Expect = 'adult: unlocked', 'zap: -> 24/7 \| ADULT A', 'settings: portal saved', 'player: stop' }
    Scn 'T16-R34b' @{ MockEnv = $A; Settings = $S; Resume = 2; Seconds = 26; Keys = "$unlock;DOWN;SLEEP:700;DOWN;SLEEP:700;DOWN;SLEEP:1500;$save"; Expect = 'zap: -> TEST 03', 'settings: portal saved'; Forbid = 'player: stop' }
}

# ====================================================================================================
function Group-N {
    'N: new scenarios for the fixes'
    # A: the guide is closed by the help's settings link, so nothing of it lies under the overlay. With the
    # guide gone, DOWN zaps once the overlay is closed (an open guide would take the key).
    Scn 'N-A1' @{ Resume = 10; Seconds = 16; Keys = 'g;SLEEP:2500;?;SLEEP:400;TAB;ENTER;SLEEP:600;ESC;SLEEP:400;DOWN'; Expect = 'settings: opened', 'settings: closed', 'zap: -> TEST 11'; Forbid = 'guide: \[' }
    Scn 'N-A2' @{ Resume = 10; Seconds = 16; Keys = 'g;SLEEP:2500;?;SLEEP:400;TAB;ENTER;SLEEP:600;TAB;TAB;ENTER;SLEEP:600;ESC;ESC'; Expect = 'settings: opened'; Forbid = 'zap: ->', 'guide: \[' }
    # L: Enter on the help's settings row opens the settings in their list (o closes them; in the Language pane it would be a letter).
    Scn 'N-L' @{ Resume = 10; Seconds = 14; Keys = '?;SLEEP:500;TAB;ENTER;SLEEP:500;o;SLEEP:300;DOWN'; Expect = 'settings: opened', 'settings: closed', 'zap: -> TEST 11' }
    # M: only the portal's flag marks adult content.
    Scn 'N-M1' @{ MockEnv = @{ MOCK_XXX = '1' }; Resume = 3; Keys = 'f;SLEEP:400;TEXT:xxx some;SLEEP:800;ENTER'; Expect = 'zap: -> XXX Something' }
    Scn 'N-M2' @{ MockEnv = @{ MOCK_XXX = '1'; MOCK_ADULT = '1' }; Resume = 3; Keys = 'f;SLEEP:400;TEXT:state of the union;SLEEP:800;ENTER'; Expect = 'toast: \[FILM\] xXx: State of the Union' }
    Scn 'N-M3' @{ MockEnv = @{ MOCK_XXX = '1'; MOCK_ADULT = '1' }; Resume = 3; Keys = 'f;SLEEP:400;TEXT:adult b;SLEEP:800;ENTER'; Forbid = 'zap: -> XXX', 'zap: -> 24/7' }
    # C: a resumed channel the portal flagged since (file says "not adult") stops once the catalog is in; the next start skips it.
    $dC = Fresh 'stale-false'
    Scn 'N-C1' @{ ConfigDir = $dC; MockEnv = $AL; Resume = 901; Seconds = 14; Expect = 'resuming last watched', 'player: stop', 'toast: Vergrendeld' } { (LastWatched $dC) -match '"adult":true' }
    Scn 'N-C2' @{ ConfigDir = $dC; MockEnv = $AL; Seconds = 8; Expect = 'not resumed'; Forbid = 'loadfile' }
    # C: a file without the flag on an adult channel waits for the catalog, skips, and learns it.
    $dC3 = Fresh 'oldfile'
    Scn 'N-C3' @{ ConfigDir = $dC3; MockEnv = $A; Resume = 901; OldResumeFile = $true; Seconds = 8; Expect = 'not resumed'; Forbid = 'loadfile' } { (LastWatched $dC3) -match '"adult":true' }
    # D: a held n starts one news fetch (the mock takes 2.5 s per day EPG, the keys come every 150 ms).
    Scn 'N-D1' @{ MockEnv = @{ MOCK_NEWS = '1'; MOCK_EPG_DELAY = '2.5' }; Resume = 3; Seconds = 16; Keys = 'n;n;n;n;n;n;n;n'; Expect = 'news picker 1 items' } { (RequestsMatching 'get_simple_data_table') -eq 1 }
    # D: a Lock while the fetch is in flight: the list does not pop up.
    Scn 'N-D2' @{ MockEnv = @{ MOCK_ADULT = '1'; MOCK_NEWS = '1'; MOCK_EPG_DELAY = '5' }; Settings = $S; Resume = 3; Seconds = 26; Keys = "$unlock;n;SLEEP:300;$lock"; Expect = 'adult: unlocked', 'adult: locked', 'list dropped, the catalog or the lock changed since' }
    # D: a portal Save while the fetch is in flight (the overlay closed again by the time it lands).
    $dD3 = Fresh 'news-save'
    Scn 'N-D3' @{ ConfigDir = $dD3; MockEnv = @{ MOCK_NEWS = '1'; MOCK_EPG_DELAY = '9' }; Resume = 3; Seconds = 30; Keys = "n;SLEEP:300;$save;ESC;ESC"; Expect = 'settings: portal saved', 'list dropped, the catalog or the lock changed since' }
    # F: a typed address with a scheme and a trailing slash is cleaned.
    $dF = Fresh 'scheme'
    Scn 'N-F' @{ NoPortal = $true; ConfigDir = $dF; Keys = 'TAB;TEXT:http://127.0.0.1:18080/;TAB;TEXT:tester;TAB;TEXT:secret;ENTER'; Seconds = 14; Expect = 'portal: no credentials', 'settings: connection check Ok', 'settings: portal saved' } { (Get-Content -Raw (Join-Path $dF 'BongoPlayer.ini')).Trim() -eq 'portal=tester:secret@127.0.0.1:18080' }
    # G: a busy portal at login is "unreachable", not "not a valid portal".
    foreach ($status in '503', '509') {
        $dG = Fresh "busy-$status"
        Scn "N-G-$status" @{ NoPortal = $true; ConfigDir = $dG; MockEnv = @{ MOCK_LOGIN_STATUS = $status }; Keys = 'TAB;TEXT:127.0.0.1:18080;TAB;TEXT:tester;TAB;TEXT:secret;ENTER'; Seconds = 14; Expect = 'portal: no credentials', 'settings: connection check Unreachable'; Forbid = 'connection check Invalid', 'portal saved' } { -not (Test-Path (Join-Path $dG 'BongoPlayer.ini')) }
    }
    # H: the PIN dialogs stay open on a wrong or malformed PIN.
    Scn 'N-H1' @{ MockEnv = $A; Settings = $S; Resume = 2; Keys = 'o;DOWN;DOWN;DOWN;ENTER;ENTER;TEXT:0000;ENTER;SLEEP:400;TEXT:1234;ENTER;SLEEP:500;ESC;ESC'; Expect = 'adult: wrong PIN', 'adult: unlocked' }
    Scn 'N-H2' @{ MockEnv = $A; Settings = $S; Resume = 2; Keys = 'o;DOWN;DOWN;DOWN;ENTER;ENTER;TEXT:12;ENTER;SLEEP:300;TEXT:1234;ENTER;SLEEP:500;ESC;ESC'; Expect = 'adult: unlocked'; Forbid = 'adult: wrong PIN' }
    # J: the log says the portal comes from --xtream (the Portal settings show their note then); not so for the ini.
    Scn 'N-J1' @{ Resume = 3; Expect = 'portal: from --xtream, which beats BongoPlayer.ini on every start' }
    $dJ = Fresh 'ini-portal'; New-Item -ItemType Directory $dJ | Out-Null
    [IO.File]::WriteAllText((Join-Path $dJ 'BongoPlayer.ini'), "portal=tester:secret@127.0.0.1:18080`n")
    Scn 'N-J2' @{ NoPortal = $true; ConfigDir = $dJ; Seconds = 8; Expect = 'portal: xtream <user>@127.0.0.1:18080'; Forbid = 'portal: from --xtream' }
}

# ====================================================================================================
function Group-X {
    'X: the other extras of the Task 12-17 reports (run-scenario.ps1 alone)'
    Scn 'T3-1' @{ Resume = 3; Expect = 'relay: first bytes'; Forbid = 'secret' }
    Scn 'T3-2' @{ Resume = 10; Keys = 'UP;SLEEP:1500;DOWN;MARK:after-keys'; Expect = 'zap: -> TEST 09', 'zap: -> TEST 10' }
    Scn 'T4-1' @{ Resume = 3; Expect = 'relay: first bytes', 'portal: xtream <user>@127.0.0.1:18080' }
    Scn 'T5-1' @{ Resume = 3; Expect = 'resuming last watched \(http://127.0.0.1:18080/live/<u>/<p>/3.m3u8\)'; Forbid = 'secret' }
    Scn 'T5-2' @{ ResumeUrl = "http://127.0.0.1:18080/live/tester/it's pw9/3.m3u8"; Expect = 'resuming last watched \(http://127.0.0.1:18080/live/<u>/<p>/3.m3u8\)'; Forbid = 'pw9', "it's", 'tester' }
    Scn 'T12-5' @{ NoPortal = $true; Keys = 'TEXT:onno;o;f;1'; Expect = 'portal: no credentials'; Forbid = 'settings: opened' }
    Scn 'T12-7' @{ Resume = 10; Keys = 'o;DOWN;DOWN;TAB;TAB;TAB;TAB;TAB;TAB;o;DOWN'; Expect = 'settings: opened'; Forbid = 'settings: closed', 'zap: ->' }
    Scn 'T12-8' @{ Resume = 10; Keys = 'o;DOWN;DOWN;TAB;TAB;TAB;TAB;TAB;TAB;ESC;SLEEP:300;DOWN'; Expect = 'settings: opened'; Forbid = 'settings: closed', 'zap: ->' }
    Scn 'T12-9' @{ Resume = 10; Keys = 'o;TAB;TAB;TAB;ENTER;SPACE;ESC;ESC;DOWN'; Expect = 'settings: zap keys reversed', 'settings: closed', 'zap: -> TEST 09' }
    Scn 'T12-10' @{ Resume = 10; Keys = 'DOWN;SLEEP:500;o;TAB;o;SLEEP:300;DOWN'; Expect = 'settings: closed', 'zap: -> TEST 12' }
    Scn 'T12-11' @{ Resume = 10; Keys = 'DOWN;SLEEP:500;o;TAB;TAB;TAB;ENTER;SPACE;ESC;ESC'; Expect = 'settings: zap keys reversed' }
    Scn 'T12-x1' @{ Resume = 3; Keys = 'o;ENTER;DOWN;SPACE'; Expect = 'settings: language set to en' }
    Scn 'T12-x6' @{ Resume = 10; Keys = 'o;ENTER;o;ESC;DOWN'; Expect = 'settings: opened'; Forbid = 'settings: closed', 'zap: ->' }
    Scn 'T13-x1' @{ MockEnv = @{ MOCK_VARIANTS = '1' }; Settings = '{"language":"en"}'; Resume = 5; Seconds = 22; Keys = 'TEXT:+;SLEEP:1500;TEXT:+;SLEEP:1500;TEXT:+;SLEEP:1500;TEXT:-'; Expect = 'toast: quality up: FHD', 'toast: quality up: UHD', 'toast: quality: already highest', 'toast: quality down: FHD' }
    Scn 'T13-x2' @{ Settings = '{"language":"de"}'; Resume = 4; Seconds = 16; Keys = 'DOWN;SLEEP:1500;TEXT:*;SLEEP:800;TEXT:*'; Expect = 'toast: \[\+\] hinzugef.gt: TEST 05', 'toast: \[-\] entfernt: TEST 05' }
    Scn 'T13-x7' @{ Settings = '{"language":"en"}'; Resume = 3; Keys = 'n'; Expect = 'toast: no news channels found' }
    Scn 'T13-x8' @{ MockEnv = $N; Settings = '{"language":"de"}'; Resume = 3; Keys = 'n'; Expect = 'toast: Nachrichten werden geladen...', 'news picker 1 items' }
    Scn 'T13-x10' @{ MockEnv = @{ MOCK_DENY = '3' }; Settings = '{"language":"en"}'; Resume = 3; Expect = 'toast: channel not available on this subscription' }
    Scn 'T13-x14' @{ MockEnv = $A; Settings = '{"language":"de"}'; Resume = 3; Keys = 'f;SLEEP:400;TEXT:gewone;SLEEP:800;ENTER'; Expect = 'toast: \[FILM\] Gewone Film' }
    Scn 'T17-A' @{ MockEnv = $N; Settings = '{"language":"nl","news_rules":[]}'; Resume = 3; Keys = 'o;DOWN;DOWN;DOWN;DOWN;ENTER;TAB;SLEEP:300;ESC;ESC;n'; Expect = 'settings: closed', 'news picker: no rules'; Forbid = 'news picker 1 items' }
    Scn 'T17-G' @{ MockEnv = $N; Resume = 3; Keys = 'o;DOWN;DOWN;DOWN;DOWN;SLEEP:500;ESC;ESC'; Expect = 'settings: news lookup \(7 rules\)' }
    Scn 'T17-I' @{ MockEnv = $N; Resume = 3; Keys = 'o;DOWN;ENTER;TAB;TAB;TAB;TAB;ENTER;SLEEP:2500;ESC;ESC'; Expect = 'settings: portal saved'; Forbid = 'news lookup' }
    Scn 'GUIDE-live' @{ MockEnv = $N; ConfigDir = (Fresh 'guide-live'); Resume = 3; Seconds = 22; Keys = 'g;SLEEP:2500;l;o;SLEEP:3500;ENTER'; Expect = 'guide: \[LIVE\] NL \| NPO 1 \| Tussen Kunst en Kitsch'; Forbid = 'video stalled', '/vod/' }
    Scn 'GUIDE-catchup' @{ MockEnv = $N; ConfigDir = (Fresh 'guide-catchup'); Resume = 3; Seconds = 26; Keys = 'g;SLEEP:2500;t;SLEEP:3500;UP;ENTER'; Expect = 'guide: \[CATCHUP\] NL \| NPO1 HD \| NOS Jeugdjournaal', 'relay: catch-up playlist in' }
}

# ====================================================================================================
if ($Summary) {
    $latest = [ordered]@{}
    foreach ($line in Get-Content $jsonl -ErrorAction SilentlyContinue) { $o = $line | ConvertFrom-Json; $latest[$o.id] = $o }
    $rows = @($latest.Values)
    foreach ($g in 'R', 'N', 'C', 'B', 'X') {
        $m = @($rows | Where-Object { $_.group -eq $g })
        if (-not $m.Count) { continue }
        "{0}: {1} scenario(s): {2} PASS, {3} PASS on re-run, {4} FAIL, {5} refused-for-good; {6} refusal(s) retried" -f $g, $m.Count,
            @($m | Where-Object status -eq 'pass').Count, @($m | Where-Object status -eq 'pass-on-rerun').Count,
            @($m | Where-Object status -eq 'fail').Count, @($m | Where-Object status -eq 'refused').Count, ($m | Measure-Object refused -Sum).Sum
        foreach ($x in ($m | Where-Object { $_.status -ne 'pass' -or $_.refused })) { "    {0}  {1}  attempts {2}  refused {3}  {4}" -f $x.id, $x.status, $x.attempts, $x.refused, $x.why }
    }
    return
}
foreach ($g in $Group) {
    $script:grp = $g
    switch ($g) {
        'R' { Group-R }
        'B' { Group-B }
        'C' { Group-C }
        'N' { Group-N }
        'X' { Group-X }
        default { Write-Warning "unknown group $g" }
    }
}
if ($DryRun) { return }
$t = $script:thisRun
"{0} scenario(s) this run: {1} PASS, {2} PASS on re-run, {3} FAIL, {4} refused for good (results: {5})" -f $t.Count,
    @($t | Where-Object status -eq 'pass').Count, @($t | Where-Object status -eq 'pass-on-rerun').Count,
    @($t | Where-Object status -eq 'fail').Count, @($t | Where-Object status -eq 'refused').Count, $jsonl
