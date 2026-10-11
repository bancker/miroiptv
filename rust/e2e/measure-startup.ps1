# Time from process start to the first picture ("playback restart complete")
# when resuming a live channel against the mock portal with portal-like
# latency. Prints every run and the median; compare before and after a change.
param([string]$Exe = (Join-Path $PSScriptRoot '..\target\release\BongoPlayer.exe'), [int]$Runs = 5)
function Secs($line) {
    if ($line -match 'T(\d\d):(\d\d):(\d\d\.\d+)Z') { [int]$Matches[1] * 3600 + [int]$Matches[2] * 60 + [double]$Matches[3] }
}
$times = @()
for ($i = 1; $i -le $Runs; $i++) {
    $out = & (Join-Path $PSScriptRoot 'run-scenario.ps1') -Exe $Exe -Resume 3 -Seconds 8 -MpvVerbose `
        -MockEnv @{ MOCK_LATENCY = '1' } -Settings '{"language":"nl"}'
    $dir = (($out | Select-String '^config folder: (.+)$').Matches[0].Groups[1].Value).Trim()
    $log = Get-ChildItem (Join-Path $dir 'log') -Filter 'BongoPlayer.log.*' | Select-Object -First 1
    $lines = Get-Content $log.FullName
    $start = Secs (($lines | Select-String ' starting' | Select-Object -First 1).Line)
    $pic = Secs (($lines | Select-String 'playback restart complete' | Select-Object -First 1).Line)
    if ($null -ne $start -and $null -ne $pic) { $t = $pic - $start; $times += $t; "run {0}: {1:N2} s" -f $i, $t }
    else { "run {0}: no picture" -f $i }
}
if ($times.Count) { "median: {0:N2} s over {1} runs" -f (($times | Sort-Object)[[int][Math]::Floor($times.Count / 2)]), $times.Count }
