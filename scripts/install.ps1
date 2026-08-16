# Install enoxian release binaries on Windows.
#
#   irm https://github.com/suzent/enoxian/releases/latest/download/install.ps1 | iex
[CmdletBinding()]
param(
    [string]$Version = $(if ($env:ENOXIAN_VERSION) { $env:ENOXIAN_VERSION } else { 'latest' }),
    [string]$BinDir = $env:ENOXIAN_BIN_DIR,
    [switch]$NoPathUpdate,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

if ($Help) {
    @'
Install enoxian on 64-bit Windows.

Usage: .\install.ps1 [-Version VERSION] [-BinDir DIRECTORY] [-NoPathUpdate] [-Help]

Environment equivalents: ENOXIAN_VERSION, ENOXIAN_BIN_DIR
'@ | Write-Host
    return
}

$Repo = 'suzent/enoxian'
if ($Version -ne 'latest' -and -not $Version.StartsWith('v')) { $Version = "v$Version" }
if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'enoxian installer: only 64-bit Windows is supported'
}
if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne [Runtime.InteropServices.Architecture]::X64) {
    throw "enoxian installer: unsupported Windows architecture $([Runtime.InteropServices.RuntimeInformation]::OSArchitecture)"
}

$Asset = 'enoxian-windows-x86_64.zip'
$Base = if ($env:ENOXIAN_DOWNLOAD_BASE) {
    $env:ENOXIAN_DOWNLOAD_BASE.TrimEnd('/')
} elseif ($Version -eq 'latest') {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$Version"
}
if (-not $BinDir) { $BinDir = Join-Path $env:LOCALAPPDATA 'enoxian\bin' }
$BinDir = [IO.Path]::GetFullPath($BinDir)

if (Get-Process -Name enoxd -ErrorAction SilentlyContinue) {
    throw "enoxian installer: enoxd is running. Run 'enox stop', then retry the installer."
}

$Tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP ("enoxian-" + [guid]::NewGuid()))
$changed = $false
$committed = $false
$existing = @{}
try {
    Write-Host 'enoxian installer: detected windows/x86_64'
    Write-Host "enoxian installer: downloading $Asset ($Version)"
    Invoke-WebRequest -Uri "$Base/$Asset" -OutFile (Join-Path $Tmp $Asset) -UseBasicParsing
    Invoke-WebRequest -Uri "$Base/SHA256SUMS" -OutFile (Join-Path $Tmp 'SHA256SUMS') -UseBasicParsing

    $escapedAsset = [regex]::Escape($Asset)
    $checksumLine = Get-Content (Join-Path $Tmp 'SHA256SUMS') |
        Where-Object { $_ -match "^([0-9a-fA-F]{64})\s+\*?$escapedAsset$" } |
        Select-Object -First 1
    if (-not $checksumLine) { throw "enoxian installer: SHA256SUMS has no entry for $Asset" }
    $expected = ([regex]::Match($checksumLine, '^([0-9a-fA-F]{64})')).Groups[1].Value.ToLowerInvariant()
    $actual = (Get-FileHash (Join-Path $Tmp $Asset) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { throw "enoxian installer: checksum mismatch for $Asset" }
    Write-Host 'enoxian installer: checksum verified'

    Expand-Archive -Path (Join-Path $Tmp $Asset) -DestinationPath $Tmp -Force
    foreach ($binary in @('enox.exe', 'enoxd.exe')) {
        if (-not (Test-Path (Join-Path $Tmp $binary) -PathType Leaf)) {
            throw "enoxian installer: archive is missing $binary"
        }
    }

    $stagedVersion = & (Join-Path $Tmp 'enox.exe') --version
    if ($LASTEXITCODE -ne 0) { throw 'enoxian installer: downloaded enox failed its pre-install check' }
    & (Join-Path $Tmp 'enoxd.exe') --version *> $null
    if ($LASTEXITCODE -ne 0) { throw 'enoxian installer: downloaded enoxd failed its pre-install check' }
    if ($Version -ne 'latest' -and $stagedVersion -notmatch [regex]::Escape($Version.TrimStart('v'))) {
        throw "enoxian installer: downloaded version '$stagedVersion' does not match requested $Version"
    }

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    $backupDir = Join-Path $Tmp 'backup'
    New-Item -ItemType Directory -Path $backupDir | Out-Null
    foreach ($binary in @('enox.exe', 'enoxd.exe')) {
        $destination = Join-Path $BinDir $binary
        $existing[$binary] = Test-Path $destination -PathType Leaf
        if ($existing[$binary]) { Copy-Item $destination (Join-Path $backupDir $binary) }
        $stagedDestination = Join-Path $BinDir ".$binary.new"
        Copy-Item (Join-Path $Tmp $binary) $stagedDestination -Force
        Move-Item $stagedDestination $destination -Force
        $changed = $true
    }

    & (Join-Path $BinDir 'enox.exe') --version *> $null
    if ($LASTEXITCODE -ne 0) { throw 'enoxian installer: installed enox failed its post-install check' }
    & (Join-Path $BinDir 'enoxd.exe') --version *> $null
    if ($LASTEXITCODE -ne 0) { throw 'enoxian installer: installed enoxd failed its post-install check' }
    $committed = $true

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $pathEntries = @($userPath -split ';' | Where-Object { $_ })
    if (-not $NoPathUpdate -and -not ($pathEntries | Where-Object { $_.Trim().TrimEnd('\') -ieq $BinDir.TrimEnd('\') })) {
        $newPath = if ($userPath) { "$userPath;$BinDir" } else { $BinDir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Write-Host "enoxian installer: added $BinDir to your user PATH; open a new terminal"
    }
    Write-Host "enoxian installer: installed $stagedVersion"
    Write-Host "enoxian installer: binaries: $BinDir\enox.exe and $BinDir\enoxd.exe"
    Write-Host "enoxian installer: next: open a new terminal and run 'enox init --name my-project'"
} catch {
    if ($changed -and -not $committed) {
        Write-Warning 'enoxian installer: installation failed; restoring the previous installation'
        foreach ($binary in @('enox.exe', 'enoxd.exe')) {
            $destination = Join-Path $BinDir $binary
            if ($existing.ContainsKey($binary) -and $existing[$binary]) {
                Copy-Item (Join-Path $Tmp "backup\$binary") $destination -Force
            } else {
                Remove-Item $destination -Force -ErrorAction SilentlyContinue
            }
        }
    }
    throw
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
