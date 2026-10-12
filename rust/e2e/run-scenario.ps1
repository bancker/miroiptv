<#
Runs Bongo Player against the mock portal in an isolated config folder, sends
keys to its window and checks its log. Example:

  .\run-scenario.ps1 -Resume 10 -Keys 'UP' -Expect 'zap: -> TEST 09'

-Keys: steps separated by ';' - a key (UP DOWN LEFT RIGHT ENTER ESC TAB
BACKSPACE SPACE, or one character such as o f n 1), TEXT:<text>, SLEEP:<ms>,
MARK:<label>. Keys are posted to the app's window, so nothing reaches other
windows and no focus is needed. Exit code 1 when an -Expect is missing or a
-Forbid matches, and also when the run proves nothing: no log (or no "starting"
line), no window to post the keys to (waits up to 10 s for it), the app exited
by itself before the run ended, or it left a panic-*.log. Exit code 0 otherwise.

It refuses to start (exit code 1, naming the key) while Tab, Shift, Ctrl, Alt,
Enter, Esc, Space or an arrow key is held down: winit turns a key Windows reports
as down into a key press when the window gets the focus, so the result would
depend on the keyboard's state (see held-keys.ps1).

-Settings replaces settings.json; without it a folder that has none gets
{"language":"nl"}, so the first-start language screen stays away unless
-FirstStart asks for it. -ConfigDir reuses a folder across runs (restarts); its
old BongoPlayer.log.* and panic-*.log are removed first, so a run is judged on
its own log lines only.

-NoPortal leaves out the --xtream arguments. A fresh isolated config folder has
no BongoPlayer.ini either, so the app starts on the first-run setup prompt
("portal: no credentials" in its log), e.g.
  .\run-scenario.ps1 -NoPortal -Keys 'TEXT:onno' -Expect 'portal: no credentials'
#>
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\BongoPlayer.exe'),
    [int]$Port = 18080,
    [int]$Resume = 0,
    [string]$ResumeUrl = '',
    [string]$Url = '',
    [string]$Settings = '',
    [switch]$FirstStart,    # no settings.json: the language screen shows
    [switch]$OldResumeFile, # last_watched.json as 0.5 wrote it (no adult flag)
    [switch]$NoPortal,      # no --xtream arguments: the setup prompt shows
    [string]$ConfigDir = '',
    [string]$Keys = '',
    [int]$KeysAfter = 7,
    [int]$Seconds = 12,
    [string[]]$Expect = @(),
    [string[]]$Forbid = @(),
    [hashtable]$MockEnv = @{},
    [string]$MockMode = 'none',
    [switch]$MpvVerbose
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'held-keys.ps1')
Assert-KeysUp
Add-Type -Namespace E2E -Name Win -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
[DllImport("user32.dll")] public static extern short VkKeyScanW(char ch);
[DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint code, uint mapType);
public static void Key(IntPtr hwnd, int vk, bool extended) {
    uint scan = MapVirtualKeyW((uint)vk, 0);
    uint down = 1u | (scan << 16) | (extended ? (1u << 24) : 0u);
    uint up = down | (1u << 30) | (1u << 31);
    PostMessage(hwnd, 0x0100, (IntPtr)vk, (IntPtr)unchecked((int)down));
    PostMessage(hwnd, 0x0101, (IntPtr)vk, (IntPtr)unchecked((int)up));
}
// A character key: down, the character itself (what TranslateMessage makes of
// a real key press - winit needs it), up.
public static void Char(IntPtr hwnd, char c) {
    int vk = VkKeyScanW(c) & 0xFF;
    uint scan = MapVirtualKeyW((uint)vk, 0);
    uint down = 1u | (scan << 16);
    uint up = down | (1u << 30) | (1u << 31);
    PostMessage(hwnd, 0x0100, (IntPtr)vk, (IntPtr)unchecked((int)down));
    PostMessage(hwnd, 0x0102, (IntPtr)c, (IntPtr)unchecked((int)down));
    PostMessage(hwnd, 0x0101, (IntPtr)vk, (IntPtr)unchecked((int)up));
}
'@
$named = @{ UP = @(0x26, $true); DOWN = @(0x28, $true); LEFT = @(0x25, $true); RIGHT = @(0x27, $true)
            ENTER = @(0x0D, $false); ESC = @(0x1B, $false); TAB = @(0x09, $false); BACKSPACE = @(0x08, $false) }
$segs = Join-Path $PSScriptRoot 'segs'
if (-not (Test-Path (Join-Path $segs 'seg0.ts'))) { throw "no segments: run make-segments.ps1 first" }
$reqLog = Join-Path $PSScriptRoot "requests-$Port.log"
if (Test-Path $reqLog) { Remove-Item $reqLog }
if (-not $ConfigDir) {
    $ConfigDir = Join-Path ([IO.Path]::GetTempPath()) ("bongo-e2e-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
}
New-Item -ItemType Directory -Force $ConfigDir | Out-Null
# WriteAllText: UTF-8 without a BOM (Set-Content -Encoding UTF8 adds one).
$settingsFile = Join-Path $ConfigDir 'settings.json'
if ($FirstStart) { Remove-Item $settingsFile -ErrorAction SilentlyContinue }
elseif ($Settings) { [IO.File]::WriteAllText($settingsFile, $Settings) }
elseif (-not (Test-Path $settingsFile)) { [IO.File]::WriteAllText($settingsFile, '{"language":"nl"}') }
$lastUrl = if ($ResumeUrl) { $ResumeUrl } elseif ($Resume) { "http://127.0.0.1:$Port/live/tester/secret/$Resume.m3u8" } else { '' }
if ($lastUrl) {
    $live = if ($lastUrl -match '/live/') { 'true' } else { 'false' }
    # 0.6 adds an adult flag (Task 16); older versions ignore it.
    $adult = if ($OldResumeFile) { '' } else { ',"adult":false' }
    Set-Content (Join-Path $ConfigDir 'last_watched.json') ('{"url":"' + $lastUrl + '","live":' + $live + $adult + '}') -Encoding ASCII -NoNewline
}
# A reused -ConfigDir still holds the previous run's log (the app appends to one
# file per day) and panic files: clear them, so a run is judged on its own lines.
$logDir = Join-Path $ConfigDir 'log'
foreach ($old in 'BongoPlayer.log.*', 'panic-*.log') {
    Get-ChildItem $logDir -Filter $old -ErrorAction SilentlyContinue | Remove-Item -Force
}
$mock = $null; $app = $null
$saved = @{}
$problems = @()   # reasons this run cannot pass, whatever the log says
$exitedEarly = $false; $exitCode = $null
try {
    foreach ($k in $MockEnv.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, [string]$MockEnv[$k]) }
    $mock = Start-Process py -ArgumentList '-3', (Join-Path $PSScriptRoot 'mock_portal.py'), $Port, $segs, $reqLog, $MockMode -PassThru -WindowStyle Hidden
    Start-Sleep -Milliseconds 800
    $env:BONGO_CONFIG_DIR = $ConfigDir
    $env:BONGO_INI = Join-Path $ConfigDir 'BongoPlayer.ini'
    if ($MpvVerbose) { $env:MPV_LOG_LEVEL = 'v' }
    $appArgs = @()
    if (-not $NoPortal) { $appArgs += '--xtream', "tester:secret@127.0.0.1:$Port" }
    if ($Url) { $appArgs += $Url }
    # Start-Process rejects an empty -ArgumentList, which -NoPortal without -Url gives.
    $start = @{ FilePath = $Exe; PassThru = $true }
    if ($appArgs.Count) { $start.ArgumentList = $appArgs }
    $app = Start-Process @start
    try { $null = $app.Handle } catch { }   # Start-Process -PassThru: keeps ExitCode readable
    if ($Keys) {
        Start-Sleep -Seconds $KeysAfter
        # Keys are posted to the window by its handle; handle 0 would post to this
        # script's own thread and silently do nothing. Wait up to 10 s for the window.
        $hwnd = [IntPtr]::Zero
        $until = (Get-Date).AddSeconds(10)
        do {
            if ($app.HasExited) { break }
            $app.Refresh()
            $hwnd = $app.MainWindowHandle
            if ($hwnd -ne [IntPtr]::Zero) { break }
            Start-Sleep -Milliseconds 200
        } while ((Get-Date) -lt $until)
        if ($hwnd -eq [IntPtr]::Zero) {
            $problems += "no window: the app has no main window to post the keys to (waited $KeysAfter s plus up to 10 s), so no key was sent"
        } else {
            foreach ($step in $Keys -split ';') {
                $kind, $arg = $step -split ':', 2
                if ($null -eq $arg) { $arg = '' }
                switch ($kind) {
                    'TEXT' { foreach ($c in $arg.ToCharArray()) { [E2E.Win]::Char($hwnd, $c); Start-Sleep -Milliseconds 40 } }
                    'SLEEP' { Start-Sleep -Milliseconds ([int]$arg) }
                    'MARK' { "  --- $arg at {0:HH:mm:ss.fff}" -f (Get-Date).ToUniversalTime() }
                    default {
                        if ($named.ContainsKey($kind)) { [E2E.Win]::Key($hwnd, $named[$kind][0], $named[$kind][1]) }
                        elseif ($kind -eq 'SPACE') { [E2E.Win]::Char($hwnd, [char]' ') }
                        elseif ($kind.Length -eq 1) { [E2E.Win]::Char($hwnd, [char]$kind) }
                        else { throw "unknown key step '$step'" }
                        Start-Sleep -Milliseconds 150
                    }
                }
            }
            Start-Sleep -Seconds ([Math]::Max(1, $Seconds - $KeysAfter))
        }
    } else {
        Start-Sleep -Seconds $Seconds
    }
} finally {
    if ($app) {
        # An app that is already gone before we kill it quit or crashed by itself.
        $exitedEarly = $app.HasExited
        if ($exitedEarly) { try { $exitCode = $app.ExitCode } catch { } }
        else { $app.Kill(); $app.WaitForExit(3000) | Out-Null }
    }
    if ($mock -and -not $mock.HasExited) { $mock.Kill() }
    $env:BONGO_CONFIG_DIR = $null; $env:BONGO_INI = $null; $env:MPV_LOG_LEVEL = $null
    foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
}
$log = Get-ChildItem (Join-Path $ConfigDir 'log') -Filter 'BongoPlayer.log.*' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
# The log is UTF-8 without a BOM: say so, or "nächste" arrives as "nÃ¤chste".
$lines = if ($log) { Get-Content $log.FullName -Encoding UTF8 } else { @() }
"config folder: $ConfigDir"
'=== app log (zaps, loads, settings, adult, news, pause, errors) ==='
$lines | Where-Object { $_ -match 'zap: |loadfile|resuming|resume:|settings:|adult:|news picker|pause:|player: stop|toast:|relay: (first bytes|the portal)|end-file \(reason=(eof|error)\)|ERROR' } |
    ForEach-Object { '  ' + (($_ -replace '^\d{4}-\d\d-\d\dT(\d\d:\d\d:\d\d\.\d{3})\d*Z', '$1') -replace '\s+(main|mpv-player|tokio-rt-worker)\s+\S+: ', ' ') }
'=== mock portal: live requests ==='
if (Test-Path $reqLog) { Get-Content $reqLog | Where-Object { $_ -match '/live/' } | ForEach-Object { '  ' + $_ } }
$fail = 0
# A pass has to mean the app ran: with no log at all every -Forbid would pass.
if (-not $log) { "FAIL  no log: the app wrote no BongoPlayer.log.* in $logDir"; $fail++ }
elseif (-not ($lines -match ' starting')) { "FAIL  no ' starting' line in the log: the app did not start"; $fail++ }
foreach ($problem in $problems) { "FAIL  $problem"; $fail++ }
if ($exitedEarly) {
    "FAIL  the app exited by itself before the run ended (exit code $(if ($null -ne $exitCode) { $exitCode } else { 'unknown' }))"; $fail++
}
foreach ($panic in @(Get-ChildItem $logDir -Filter 'panic-*.log' -ErrorAction SilentlyContinue)) {
    "FAIL  the app panicked ($($panic.Name)):"
    Get-Content $panic.FullName -TotalCount 2 | ForEach-Object { "        $_" }
    $fail++
}
foreach ($e in $Expect) { if ($lines -match $e) { "PASS  expected: $e" } else { "FAIL  missing:  $e"; $fail++ } }
foreach ($f in $Forbid) { if ($lines -match $f) { "FAIL  forbidden: $f"; $fail++ } else { "PASS  absent:   $f" } }
if ($fail) { exit 1 }
exit 0
