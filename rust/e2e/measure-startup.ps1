# Time from the app's first log line ("Bongo Player ... starting") to the first
# picture ("playback restart complete") when resuming a live channel against the
# mock portal with portal-like latency. The clock starts at that log line, not at
# process creation: loading the exe and its DLLs before it is not counted.
# Prints every run and the min / median / max; compare before and after a change.
#
# One run varies by about 1 s on a typical machine (the audio device opens slowly
# or not), so -Runs defaults to 10. -Baseline <exe> alternates the runs between
# that exe and -Exe (baseline, new, baseline, new, ...), so both see the same
# machine state, and prints one summary line per exe; -Runs counts runs per exe.
# Like run-scenario.ps1 it refuses to start while Tab, Shift, Ctrl, Alt, Enter,
# Esc, Space, an arrow key, PgUp, PgDn, Home or End is held down (see
# held-keys.ps1).
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\BongoPlayer.exe'),
    [int]$Runs = 10,
    [string]$Baseline = ''
)
. (Join-Path $PSScriptRoot 'held-keys.ps1')
Assert-KeysUp
# Numbers are printed with the invariant culture (a Dutch machine would say 2,85).
$inv = [Globalization.CultureInfo]::InvariantCulture
function Num($x) { ([double]$x).ToString('F2', $inv) }
function Secs($line) {
    if ($line -match 'T(\d\d):(\d\d):(\d\d\.\d+)Z') { [int]$Matches[1] * 3600 + [int]$Matches[2] * 60 + [double]$Matches[3] }
}
# One run of the exe; the seconds from "starting" to the first picture, or nothing.
function Measure-Run([string]$ExePath) {
    $out = & (Join-Path $PSScriptRoot 'run-scenario.ps1') -Exe $ExePath -Resume 3 -Seconds 8 -MpvVerbose `
        -MockEnv @{ MOCK_LATENCY = '1' } -Settings '{"language":"nl"}'
    $dir = (($out | Select-String '^config folder: (.+)$').Matches[0].Groups[1].Value).Trim()
    $log = Get-ChildItem (Join-Path $dir 'log') -Filter 'BongoPlayer.log.*' -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $log) { return }
    $lines = Get-Content $log.FullName
    $start = Secs (($lines | Select-String ' starting' | Select-Object -First 1).Line)
    $pic = Secs (($lines | Select-String 'playback restart complete' | Select-Object -First 1).Line)
    if ($null -ne $start -and $null -ne $pic) { $pic - $start }
}
function Summary([string]$label, $times) {
    $s = @($times | Sort-Object)
    $n = $s.Count
    $mid = [int][Math]::Floor($n / 2)
    $median = if ($n % 2) { $s[$mid] } else { ($s[$mid - 1] + $s[$mid]) / 2 }
    '{0}median: {1} s (min {2}, max {3}) over {4} runs' -f $label, (Num $median), (Num $s[0]), (Num $s[$n - 1]), $n
}

$targets = @(@{ Label = ''; Exe = $Exe; Times = @() })
if ($Baseline) {
    if (-not (Test-Path $Baseline)) { throw "baseline exe not found: $Baseline" }
    $targets = @(@{ Label = 'baseline'; Exe = $Baseline; Times = @() }, @{ Label = 'new'; Exe = $Exe; Times = @() })
}
$n = 0
for ($i = 1; $i -le $Runs; $i++) {
    foreach ($t in $targets) {
        $n++
        $name = if ($t.Label) { "run $n ($($t.Label))" } else { "run $n" }
        $secs = Measure-Run $t.Exe
        if ($null -ne $secs) { $t.Times += $secs; '{0}: {1} s' -f $name, (Num $secs) }
        else { '{0}: no picture' -f $name }
    }
}
foreach ($t in $targets) {
    $label = if ($t.Label) { '{0,-9}' -f $t.Label } else { '' }
    if ($t.Times.Count) { Summary $label $t.Times } else { '{0}median: no picture in any run' -f $label }
}
