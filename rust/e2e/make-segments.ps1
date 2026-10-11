# Generates the mock portal's test segments (15 x 4 s, 640x360, keyframe every
# second) with ffmpeg. Run once; the output folder is git-ignored.
param([string]$Out = (Join-Path $PSScriptRoot 'segs'))
$ErrorActionPreference = 'Stop'
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    throw 'ffmpeg not found on PATH (winget install Gyan.FFmpeg)'
}
New-Item -ItemType Directory -Force $Out | Out-Null
& ffmpeg -hide_banner -loglevel error -y `
    -f lavfi -i 'testsrc=size=640x360:rate=25' `
    -f lavfi -i 'sine=frequency=440:sample_rate=48000' `
    -t 60 -c:v libx264 -profile:v baseline -g 25 -pix_fmt yuv420p `
    -c:a aac -ac 1 -b:a 96k `
    -f hls -hls_time 4 -hls_list_size 0 `
    -hls_segment_filename (Join-Path $Out 'seg%d.ts') (Join-Path $Out 'index.m3u8')
if ($LASTEXITCODE -ne 0) { throw "ffmpeg failed ($LASTEXITCODE)" }
"segments in ${Out}: $((Get-ChildItem $Out -Filter *.ts).Count)"
