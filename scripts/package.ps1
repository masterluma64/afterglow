# Builds Afterglow, with libmpv packed into the exe, ready to run on another
# PC: dist\afterglow-<version>-windows-x64.exe on its own, and a zip of it
# with the README and a notice.
#
#   scripts\package.cmd               (or double-click it)
#   scripts\package.cmd -SkipTests
#
# Needs vendor\libmpv-2.dll (see Building in the README). The version comes
# from Cargo.toml; building the same version again replaces them.
param([switch]$SkipTests)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Run($what, [scriptblock]$command) {
    Write-Host "== $what" -ForegroundColor Cyan
    & $command
    if ($LASTEXITCODE -ne 0) { throw "$what failed" }
}

if (-not (Test-Path vendor\libmpv-2.dll)) {
    throw "vendor\libmpv-2.dll is missing; see Building in the README"
}
# libmpv decodes every video Afterglow opens, so only the build pinned in
# vendor\libmpv-2.dll.sha256 is packaged. To update it, test the new DLL and
# put its SHA-256 there.
$pinned = ((Get-Content vendor\libmpv-2.dll.sha256 -TotalCount 1) -split '\s+')[0]
function Check-Libmpv($path) {
    $hash = (Get-FileHash $path -Algorithm SHA256).Hash
    if ($hash -ne $pinned) {
        throw "$path has SHA-256 $hash, not the pinned $pinned (vendor\libmpv-2.dll.sha256). If you meant to update libmpv, test it and put its checksum there."
    }
}
Check-Libmpv vendor\libmpv-2.dll
if (-not $SkipTests) {
    Run 'Testing' { cargo test --release }
}
Run 'Building' { cargo build --release }

$version = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages[0].version
$commit = git rev-parse --short HEAD
if (git status --porcelain) { $commit += ' (with uncommitted changes)' }
$mpv = (Get-Item vendor\libmpv-2.dll).VersionInfo.ProductVersion

$name = "afterglow-$version"
$folder = Join-Path dist $name
$zip = Join-Path dist "$name-windows-x64.zip"
$exe = Join-Path dist "$name-windows-x64.exe"
Remove-Item -Recurse -Force $folder, $zip, $exe -ErrorAction SilentlyContinue
New-Item -ItemType Directory $folder | Out-Null
Copy-Item target\release\afterglow.exe $exe
Copy-Item target\release\afterglow.exe, README.md $folder
@"
Afterglow $version, built from commit $commit.
This build of Afterglow is for private use only; please don't share it.

The libmpv packed into afterglow.exe (unpacked into its data folder on the
first start) is mpv $mpv (https://mpv.io), as built by
https://github.com/shinchiro/mpv-winbuild-cmake. It is a GPL build: mpv and
the FFmpeg libraries in it are free software under the GNU GPL and LGPL.
Their source code and exact license terms are at
https://github.com/mpv-player/mpv and https://ffmpeg.org, and the build
recipe at https://github.com/shinchiro/mpv-winbuild-cmake.

The icons are Phosphor (https://phosphoricons.com), under the MIT license.
"@ | Set-Content -Encoding utf8 (Join-Path $folder 'THIRD-PARTY.txt')
Compress-Archive -Path $folder -DestinationPath $zip
Remove-Item -Recurse -Force $folder

function Size($path) { [math]::Round((Get-Item $path).Length / 1e6, 1) }
# The exe is meant to stay under 50 MB.
if ((Get-Item $exe).Length -ge 50e6) { Write-Host "Warning: the exe is 50 MB or more" -ForegroundColor Yellow }
Write-Host "== Done: $exe ($(Size $exe) MB) and $zip ($(Size $zip) MB)" -ForegroundColor Green
