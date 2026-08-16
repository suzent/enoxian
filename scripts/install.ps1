# Install enoxian release binaries on Windows.
#
#   irm https://github.com/suzent/enoxian/releases/latest/download/install.ps1 | iex
$ErrorActionPreference = 'Stop'

$Repo = 'suzent/enoxian'
$Version = if ($env:ENOXIAN_VERSION) { $env:ENOXIAN_VERSION } else { 'latest' }

if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'install: only 64-bit Windows is supported'
}
$Asset = 'enoxian-windows-x86_64.zip'
$Base = if ($Version -eq 'latest') {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$Version"
}

$Tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP ("enoxian-" + [guid]::NewGuid()))
try {
    Write-Host "install: downloading $Asset ($Version)"
    Invoke-WebRequest -Uri "$Base/$Asset" -OutFile (Join-Path $Tmp $Asset) -UseBasicParsing
    Invoke-WebRequest -Uri "$Base/SHA256SUMS" -OutFile (Join-Path $Tmp 'SHA256SUMS') -UseBasicParsing

    $checksumLine = Get-Content (Join-Path $Tmp 'SHA256SUMS') |
        Where-Object { $_ -match "^([0-9a-fA-F]{64})\s+\*?$([regex]::Escape($Asset))$" } |
        Select-Object -First 1
    if (-not $checksumLine) { throw "install: SHA256SUMS has no entry for $Asset" }
    $expected = ([regex]::Match($checksumLine, '^([0-9a-fA-F]{64})')).Groups[1].Value.ToLowerInvariant()
    $actual = (Get-FileHash (Join-Path $Tmp $Asset) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { throw "install: checksum mismatch for $Asset" }
    Write-Host 'install: checksum verified'

    Expand-Archive -Path (Join-Path $Tmp $Asset) -DestinationPath $Tmp -Force
    foreach ($binary in @('enox.exe', 'enoxd.exe')) {
        if (-not (Test-Path (Join-Path $Tmp $binary))) {
            throw "install: archive missing $binary"
        }
    }

    $BinDir = if ($env:ENOXIAN_BIN_DIR) {
        $env:ENOXIAN_BIN_DIR
    } else {
        Join-Path $env:LOCALAPPDATA 'enoxian\bin'
    }
    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    Copy-Item (Join-Path $Tmp 'enox.exe') (Join-Path $BinDir 'enox.exe') -Force
    Copy-Item (Join-Path $Tmp 'enoxd.exe') (Join-Path $BinDir 'enoxd.exe') -Force

    & (Join-Path $BinDir 'enox.exe') --version *> $null
    if ($LASTEXITCODE -ne 0) { throw 'install: installed enox failed its smoke test' }
    & (Join-Path $BinDir 'enoxd.exe') --version *> $null
    if ($LASTEXITCODE -ne 0) { throw 'install: installed enoxd failed its smoke test' }

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $BinDir) {
        $newPath = if ($userPath) { "$userPath;$BinDir" } else { $BinDir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Write-Host "install: added $BinDir to your user PATH — restart the terminal"
    }
    Write-Host "install: installed enox and enoxd to $BinDir"
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
