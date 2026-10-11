<#
Runs Bongo Player against the mock portal in an isolated config folder, sends
keys to its window and checks its log. Example:

  .\run-scenario.ps1 -Resume 10 -Keys 'UP' -Expect 'zap: -> TEST 09'

-Keys: steps separated by ';' - a key (UP DOWN LEFT RIGHT ENTER ESC TAB
BACKSPACE SPACE, or one character such as o f n 1), TEXT:<text>, SLEEP:<ms>,
MARK:<label>. Keys are posted to the app's window, so nothing reaches other
windows and no focus is needed. Exit code 1 when an -Expect is missing or a
-Forbid matches.

-Settings replaces settings.json; without it a folder that has none gets
{"language":"nl"}, so the first-start language screen stays away unless
-FirstStart asks for it. -ConfigDir reuses a folder across runs (restarts).
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
$mock = $null; $app = $null
$saved = @{}
try {
    foreach ($k in $MockEnv.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, [string]$MockEnv[$k]) }
    $mock = Start-Process py -ArgumentList '-3', (Join-Path $PSScriptRoot 'mock_portal.py'), $Port, $segs, $reqLog, $MockMode -PassThru -WindowStyle Hidden
    Start-Sleep -Milliseconds 800
    $env:BONGO_CONFIG_DIR = $ConfigDir
    $env:BONGO_INI = Join-Path $ConfigDir 'BongoPlayer.ini'
    if ($MpvVerbose) { $env:MPV_LOG_LEVEL = 'v' }
    $appArgs = @('--xtream', "tester:secret@127.0.0.1:$Port")
    if ($Url) { $appArgs += $Url }
    $app = Start-Process $Exe -ArgumentList $appArgs -PassThru
    if ($Keys) {
        Start-Sleep -Seconds $KeysAfter
        $app.Refresh()
        $hwnd = $app.MainWindowHandle
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
    } else {
        Start-Sleep -Seconds $Seconds
    }
} finally {
    if ($app -and -not $app.HasExited) { $app.Kill(); $app.WaitForExit(3000) | Out-Null }
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
foreach ($e in $Expect) { if ($lines -match $e) { "PASS  expected: $e" } else { "FAIL  missing:  $e"; $fail++ } }
foreach ($f in $Forbid) { if ($lines -match $f) { "FAIL  forbidden: $f"; $fail++ } else { "PASS  absent:   $f" } }
if ($fail) { exit 1 }
