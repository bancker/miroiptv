# Extract latest tvplayer zip, run --selftest, do a brief test-pattern render check.
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$distDir = Join-Path $root 'dist'

$latestZip = Get-ChildItem (Join-Path $distDir 'tvplayer-v*.zip') -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $latestZip) { throw "no tvplayer zip in $distDir - run build-zip.ps1 first" }

$verifyDir = Join-Path $env:TEMP "tvplayer-verify-$(Get-Random)"
New-Item -ItemType Directory -Path $verifyDir -Force | Out-Null

try {
    Write-Host "== extracting $($latestZip.Name) =="
    Expand-Archive -Path $latestZip.FullName -DestinationPath $verifyDir -Force

    $exe = Join-Path $verifyDir 'tvplayer.exe'
    $dll = Join-Path $verifyDir 'libmpv-2.dll'

    if (-not (Test-Path $exe)) { throw "tvplayer.exe missing from zip" }
    if (-not (Test-Path $dll)) { throw "libmpv-2.dll missing from zip" }

    $exeKb = [math]::Round((Get-Item $exe).Length / 1KB, 1)
    $dllMb = [math]::Round((Get-Item $dll).Length / 1MB, 1)
    Write-Host "  tvplayer.exe   $exeKb KB"
    Write-Host "  libmpv-2.dll   $dllMb MB"

    Write-Host "== smoke: tvplayer --selftest =="
    $out = & $exe --selftest 2>&1
    $rc = $LASTEXITCODE
    if ($rc -ne 0) { throw "selftest exit code ${rc}; output:`n$out" }
    if ($out -notmatch 'selftest: args parsed OK') {
        throw "unexpected selftest output:`n$out"
    }
    Write-Host "  OK: $out"

    Write-Host "== smoke: tvplayer --version =="
    $verOut = & $exe --version 2>&1
    if ($LASTEXITCODE -ne 0) { throw "tvplayer --version failed:`n$verOut" }
    if ($verOut -notmatch 'tvplayer') {
        throw "version output unexpected:`n$verOut"
    }
    Write-Host "  OK: $verOut"

    Write-Host "== smoke: 6s test-pattern render =="
    $logOut = Join-Path $env:TEMP "tv-verify-out-$(Get-Random).log"
    $logErr = Join-Path $env:TEMP "tv-verify-err-$(Get-Random).log"
    $proc = Start-Process -FilePath $exe `
        -ArgumentList 'av://lavfi:smptebars=size=1280x720:rate=25:duration=20' `
        -PassThru -WindowStyle Minimized `
        -RedirectStandardOutput $logOut -RedirectStandardError $logErr
    Start-Sleep -Seconds 6
    if ($proc.HasExited) {
        $code = $proc.ExitCode
        Write-Host "  process exited early with code $code"
        if (Test-Path $logErr) { Get-Content $logErr | ForEach-Object { Write-Host "    stderr: $_" } }
        if (Test-Path $logOut) { Get-Content $logOut | ForEach-Object { Write-Host "    stdout: $_" } }
        if ($code -ne 0) { throw "test-pattern run failed with exit code $code" }
    } else {
        Write-Host "  running OK after 6s, terminating"
        $proc.Kill()
        $proc.WaitForExit(2000) | Out-Null
    }
    if (Test-Path $logErr) {
        $errs = Get-Content $logErr -Raw
        if ($errs -match 'panic' -or $errs -match 'PANIC') {
            throw "panic detected in stderr:`n$errs"
        }
    }
    Remove-Item $logOut, $logErr -Force -ErrorAction SilentlyContinue
    Write-Host "  OK"

    Write-Host ""
    Write-Host "==================================="
    Write-Host "  verify OK : $($latestZip.Name)"
    Write-Host "==================================="
}
finally {
    Remove-Item $verifyDir -Recurse -Force -ErrorAction SilentlyContinue
}
