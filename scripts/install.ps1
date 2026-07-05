# enoxian quick-install for Windows (PowerShell).
#
#   irm https://raw.githubusercontent.com/suzent/enoxian/main/scripts/install.ps1 | iex
#
# Downloads the latest (or a pinned) release zip and installs enox.exe /
# enoxd.exe to a per-user dir on PATH. Override with env vars:
#   $env:ENOXIAN_VERSION = 'v0.1.4'                 # pin a release (default: latest)
#   $env:ENOXIAN_BIN_DIR = 'C:\tools\enoxian'       # install dir

$ErrorActionPreference = 'Stop'
$Repo = 'suzent/enoxian'
$Version = if ($env:ENOXIAN_VERSION) { $env:ENOXIAN_VERSION } else { 'latest' }

# Only x86_64 Windows binaries are published today.
if ([System.Environment]::Is64BitOperatingSystem -eq $false) {
    throw 'install: only 64-bit Windows is supported.'
}
$Asset = 'enoxian-windows-x86_64.zip'

$Url = if ($Version -eq 'latest') {
    "https://github.com/$Repo/releases/latest/download/$Asset"
} else {
    "https://github.com/$Repo/releases/download/$Version/$Asset"
}

$BinDir = if ($env:ENOXIAN_BIN_DIR) { $env:ENOXIAN_BIN_DIR } else { Join-Path $env:LOCALAPPDATA 'enoxian\bin' }
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

$Tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP ("enoxian-" + [guid]::NewGuid()))
try {
    $Zip = Join-Path $Tmp $Asset
    Write-Host "install: downloading $Asset ($Version)"
    Invoke-WebRequest -Uri $Url -OutFile $Zip -UseBasicParsing
    Expand-Archive -Path $Zip -DestinationPath $Tmp -Force
    if (-not (Test-Path (Join-Path $Tmp 'enox.exe')) -or -not (Test-Path (Join-Path $Tmp 'enoxd.exe'))) {
        throw 'install: archive missing enox.exe/enoxd.exe'
    }
    Copy-Item (Join-Path $Tmp 'enox.exe')  (Join-Path $BinDir 'enox.exe')  -Force
    Copy-Item (Join-Path $Tmp 'enoxd.exe') (Join-Path $BinDir 'enoxd.exe') -Force
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}

Write-Host "install: installed enox and enoxd to $BinDir"

# Add to the user PATH if not already present.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $BinDir) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$BinDir", 'User')
    Write-Host "install: added $BinDir to your user PATH — restart the terminal to pick it up."
}
Write-Host "install: run 'enox --help' to get started."
