# Checks held-keys.ps1 against a fake keyboard: no key is pressed, read for real or sent.
# It names exactly the keys that are down, covers every watched key, lets a clean
# keyboard through and ends the calling script with exit code 1 and a message that
# names the key. Exit code 0 when all of that holds.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\e2e\test-held-keys.ps1
. (Join-Path $PSScriptRoot 'held-keys.ps1')
$failures = 0
function Check([string]$what, [bool]$ok) {
    if ($ok) { "PASS  $what" } else { "FAIL  $what"; $script:failures++ }
}

Check 'nothing held: nothing named' ((Get-HeldKeys { param($vk) $false }).Count -eq 0)
foreach ($name in $script:WatchedKeys.Keys) {
    $code = $script:WatchedKeys[$name]
    $held = @(Get-HeldKeys ([scriptblock]::Create("param(`$vk) `$vk -eq $code")))
    Check "only $name held: exactly $name is named" ($held.Count -eq 1 -and $held[0] -eq $name)
}
# Group G of the regression set pages and jumps with these: they are watched too.
foreach ($code in 0x21, 0x22, 0x23, 0x24) {
    $held = @(Get-HeldKeys ([scriptblock]::Create("param(`$vk) `$vk -eq $code")))
    Check ("virtual key 0x{0:X2} (PgUp, PgDn, End or Home) held: it is named" -f $code) ($held.Count -eq 1)
}
$many = @(Get-HeldKeys { param($vk) $vk -in 0x11, 0x10, 0x28 })
Check 'Ctrl, Shift and Down held: all three named' (($many -join ',') -eq 'Shift,Ctrl,Down arrow')
Check 'a key that is not watched is ignored' ((Get-HeldKeys { param($vk) $vk -eq 0x41 }).Count -eq 0)

# The refusal ends the calling script: run a probe script in a child process.
$probe = Join-Path $env:TEMP ('bongo-heldkeys-probe-' + [guid]::NewGuid().ToString('N').Substring(0, 8) + '.ps1')
try {
    $env:BONGO_PROBE_DIR = $PSScriptRoot
    foreach ($case in @(@{ Down = '0x09'; Code = 1; Text = 'Tab is held down'; Reaches = $false },
                        @{ Down = '-1'; Code = 0; Text = ''; Reaches = $true })) {
        @"
. (Join-Path `$env:BONGO_PROBE_DIR 'held-keys.ps1')
Assert-KeysUp { param(`$vk) `$vk -eq $($case.Down) }
Write-Output 'PROBE REACHED THE END'
"@ | Set-Content -Path $probe -Encoding ASCII
        $out = (powershell -NoProfile -ExecutionPolicy Bypass -File $probe) -join "`n"
        $what = if ($case.Code) { 'a held Tab' } else { 'a clean keyboard' }
        Check "${what}: the script ends with exit code $($case.Code)" ($LASTEXITCODE -eq $case.Code)
        if ($case.Code) {
            Check "${what}: the message names the key" ($out -match [regex]::Escape($case.Text))
        } else {
            Check "${what}: no message" ($out -notmatch 'refusing to start')
        }
        Check "${what}: the script $(if ($case.Reaches) { 'goes on' } else { 'does not go on' })" (($out -match 'PROBE REACHED THE END') -eq $case.Reaches)
    }
} finally {
    Remove-Item $probe -Force -ErrorAction SilentlyContinue
    $env:BONGO_PROBE_DIR = $null
}
exit $failures
