# Builds Offline Convertor
#   .\build.ps1          exe + engine\ffmpeg.exe into the "Offline Convertor" folder next to the sources
#                        (or dist\Offline Convertor when there is no such folder)
#   .\build.ps1 -Single  one self-contained exe (engine packed inside) into dist\OfflineConvertor.exe
# Requirements: Rust (MSVC), nasm in PATH, ffmpeg full build in PATH (winget install Gyan.FFmpeg)
# or ffmpeg.exe in the vendor folder next to this script

param(
    [switch]$Single
)

$ErrorActionPreference = 'Stop'
$src = $PSScriptRoot

# nasm cannot write into folders with non-ASCII names, so the build cache lives in an ASCII path
$env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA 'offline-convertor-build'

function Find-Ffmpeg {
    $vendor = Join-Path $src 'vendor\ffmpeg.exe'
    if (Test-Path $vendor) { return $vendor }
    $cmd = Get-Command ffmpeg -ErrorAction SilentlyContinue
    if (-not $cmd) { throw 'ffmpeg.exe not found: install it (winget install Gyan.FFmpeg) or put ffmpeg.exe into the vendor folder' }
    $item = Get-Item $cmd.Source
    if ($item.LinkType -and $item.Target) { return @($item.Target)[0] }
    return $cmd.Source
}

$ff = Find-Ffmpeg
if ($Single) { $env:OC_EMBED_FFMPEG = $ff } else { Remove-Item Env:OC_EMBED_FFMPEG -ErrorAction SilentlyContinue }

Push-Location $src
try {
    Copy-Item (Join-Path $src 'icons\icon.png') (Join-Path $src 'ui\icon.png') -Force
    # cargo reports progress on stderr; don't let PowerShell treat that as a failure
    $ErrorActionPreference = 'Continue'
    cargo build --release 2>&1 | ForEach-Object { "$_" }
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($code -ne 0) { throw 'cargo build failed' }
} finally {
    Pop-Location
    Remove-Item Env:OC_EMBED_FFMPEG -ErrorAction SilentlyContinue
}

$built = Join-Path $env:CARGO_TARGET_DIR 'release\offline-convertor.exe'

if ($Single) {
    $dist = Join-Path $src 'dist'
    New-Item -ItemType Directory -Force $dist | Out-Null
    $exe = Join-Path $dist 'OfflineConvertor.exe'
    Copy-Item $built $exe -Force
    Write-Host ('Done: {0} ({1:N1} MB)' -f $exe, ((Get-Item $exe).Length / 1MB))
    return
}

$sibling = Join-Path (Split-Path -Parent $src) 'Offline Convertor'
$out = if (Test-Path $sibling) { $sibling } else { Join-Path $src 'dist\Offline Convertor' }
New-Item -ItemType Directory -Force $out, (Join-Path $out 'engine') | Out-Null
Copy-Item $built (Join-Path $out 'Offline Convertor.exe') -Force
$ffDst = Join-Path $out 'engine\ffmpeg.exe'
if (-not (Test-Path $ffDst) -or (Get-Item $ff).Length -ne (Get-Item $ffDst).Length) {
    Copy-Item $ff $ffDst -Force
}

# Only what is needed to run stays in the output folder
Get-ChildItem $out -Force | Where-Object { $_.Name -notin @('Offline Convertor.exe', 'engine') } | Remove-Item -Recurse -Force
Get-ChildItem (Join-Path $out 'engine') -Force | Where-Object { $_.Name -ne 'ffmpeg.exe' } | Remove-Item -Recurse -Force

Write-Host "Done: $out"
Get-ChildItem $out -Recurse -File | ForEach-Object { '{0,10:N1} MB  {1}' -f ($_.Length / 1MB), $_.FullName.Substring($out.Length + 1) }
