# Fetch the official mpv-dev libmpv build for Windows x64 (MinGW-compatible).
# Idempotent: skips work if files already present.

$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$dest = Join-Path $here 'libmpv'

New-Item -ItemType Directory -Path $dest -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $dest 'include\mpv') -Force | Out-Null

$dll = Join-Path $dest 'libmpv-2.dll'
$dllA = Join-Path $dest 'libmpv.dll.a'
$header = Join-Path $dest 'include\mpv\client.h'

if ((Test-Path $dll) -and (Test-Path $header)) {
    Write-Host "libmpv already vendored at $dest"
    exit 0
}

Write-Host "Querying GitHub for latest mpv-dev release..."
$apiUrl = 'https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases'

$headers = @{ 'User-Agent' = 'tvplayer-fetch-mpv'; 'Accept' = 'application/vnd.github+json' }
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$releases = Invoke-RestMethod -Uri $apiUrl -Headers $headers -ErrorAction Stop

$asset = $null
foreach ($r in $releases) {
    foreach ($a in $r.assets) {
        if ($a.name -match '^mpv-dev-x86_64-(\d{8})-git-.*\.7z$' -and $a.name -notmatch 'v3') {
            $asset = $a
            break
        }
    }
    if ($asset) { break }
    foreach ($a in $r.assets) {
        if ($a.name -match '^mpv-dev-x86_64.*\.7z$') {
            $asset = $a
            break
        }
    }
    if ($asset) { break }
}

if (-not $asset) { throw 'No mpv-dev-x86_64 .7z asset found in any release' }

Write-Host "Selected: $($asset.name)"
$tmpZip = Join-Path $env:TEMP $asset.name
$tmpExtract = Join-Path $env:TEMP 'mpv-dev-extract'

if (Test-Path $tmpExtract) { Remove-Item $tmpExtract -Recurse -Force }
New-Item -ItemType Directory -Path $tmpExtract -Force | Out-Null

if (-not (Test-Path $tmpZip)) {
    Write-Host "Downloading $($asset.browser_download_url)"
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $tmpZip -UseBasicParsing
}

Write-Host "Extracting..."
& 7z.exe x $tmpZip -o"$tmpExtract" -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw "7z extraction failed: exit $LASTEXITCODE" }

$srcDll  = Get-ChildItem -Path $tmpExtract -Recurse -Filter 'libmpv-2.dll' | Select-Object -First 1
if (-not $srcDll) {
    $srcDll = Get-ChildItem -Path $tmpExtract -Recurse -Filter 'mpv-2.dll' | Select-Object -First 1
}
if (-not $srcDll) { throw 'libmpv-2.dll not found in extracted package' }

$srcDllA = Get-ChildItem -Path $tmpExtract -Recurse -Filter 'libmpv.dll.a' -ErrorAction SilentlyContinue | Select-Object -First 1
$srcLib  = Get-ChildItem -Path $tmpExtract -Recurse -Filter 'libmpv.lib'  -ErrorAction SilentlyContinue | Select-Object -First 1

$srcClient = Get-ChildItem -Path $tmpExtract -Recurse -Filter 'client.h' | Select-Object -First 1
$srcRender = Get-ChildItem -Path $tmpExtract -Recurse -Filter 'render.h' | Select-Object -First 1
$srcRenderGl = Get-ChildItem -Path $tmpExtract -Recurse -Filter 'render_gl.h' -ErrorAction SilentlyContinue | Select-Object -First 1
$srcStream = Get-ChildItem -Path $tmpExtract -Recurse -Filter 'stream_cb.h' -ErrorAction SilentlyContinue | Select-Object -First 1

if (-not $srcClient) { throw 'client.h not found in extracted package' }
if (-not $srcRender) { throw 'render.h not found in extracted package' }

Copy-Item $srcDll.FullName $dll -Force
if ($srcDllA) { Copy-Item $srcDllA.FullName $dllA -Force }
if ($srcLib)  { Copy-Item $srcLib.FullName  (Join-Path $dest 'libmpv.lib') -Force }
Copy-Item $srcClient.FullName (Join-Path $dest 'include\mpv\client.h') -Force
Copy-Item $srcRender.FullName (Join-Path $dest 'include\mpv\render.h') -Force
if ($srcRenderGl) { Copy-Item $srcRenderGl.FullName (Join-Path $dest 'include\mpv\render_gl.h') -Force }
if ($srcStream)   { Copy-Item $srcStream.FullName   (Join-Path $dest 'include\mpv\stream_cb.h') -Force }

"$($asset.name)`n$($asset.browser_download_url)`n$(Get-Date -Format o)" | Set-Content (Join-Path $dest '.mpv-dev-version')

if (-not (Test-Path $dllA) -and -not (Test-Path (Join-Path $dest 'libmpv.lib'))) {
    Write-Host "Neither libmpv.dll.a nor libmpv.lib found; generating libmpv.dll.a via dlltool"
    $gendef = (Get-Command gendef.exe -ErrorAction SilentlyContinue).Source
    $dlltool = (Get-Command dlltool.exe -ErrorAction SilentlyContinue).Source
    if ($gendef -and $dlltool) {
        Push-Location $dest
        & $gendef libmpv-2.dll
        & $dlltool --dllname libmpv-2.dll --def libmpv-2.def --output-lib libmpv.dll.a
        Pop-Location
    } else {
        Write-Warning "gendef/dlltool not found; linker may not be able to find libmpv import library"
    }
}

Write-Host ""
Write-Host "libmpv vendored to: $dest"
Get-ChildItem $dest -File | Select-Object Name, @{N='Size (KB)'; E={[math]::Round($_.Length/1KB,1)}} | Format-Table
Write-Host ""
