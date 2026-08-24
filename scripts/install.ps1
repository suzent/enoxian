# Install enoxian release binaries on Windows.
#
#   irm https://github.com/suzent/enoxian/releases/latest/download/install.ps1 | iex
[CmdletBinding()]
param(
    [string]$Version = $(if ($env:ENOXIAN_VERSION) { $env:ENOXIAN_VERSION } else { 'latest' }),
    [string]$BinDir = $env:ENOXIAN_BIN_DIR,
    [switch]$NoPathUpdate,
    [switch]$EnableService,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

if ($Help) {
    @'
Install enoxian on 64-bit Windows.

Usage: .\install.ps1 [-Version VERSION] [-BinDir DIRECTORY] [-NoPathUpdate] [-EnableService] [-Help]

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

$installedEnox = Join-Path $BinDir 'enox.exe'
$stateDir = if ($env:ENOXIAN_HOME) {
    [IO.Path]::GetFullPath($env:ENOXIAN_HOME)
} else {
    Join-Path ([Environment]::GetFolderPath('UserProfile')) '.enoxian'
}
$serviceDefinition = Join-Path $stateDir 'service\managed-task.txt'
$serviceWasInstalled = Test-Path $serviceDefinition -PathType Leaf
if (Get-Process -Name enoxd -ErrorAction SilentlyContinue) {
    throw "enoxian installer: legacy enoxd is still running. Run 'enox stop', then retry."
}

function Wait-EnoxianBinaryUnlocked([string]$Path, [int]$TimeoutMilliseconds = 10000) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMilliseconds)
    do {
        try {
            $stream = [IO.File]::Open(
                $Path,
                [IO.FileMode]::Open,
                [IO.FileAccess]::ReadWrite,
                [IO.FileShare]::None
            )
            $stream.Dispose()
            return
        } catch [IO.IOException] {
            Start-Sleep -Milliseconds 250
        }
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "enoxian installer: timed out waiting for the running service to release $Path"
}

function Invoke-EnoxianStopWithTimeout([string]$Path, [int]$TimeoutMilliseconds = 5000) {
    $stop = Start-Process -FilePath $Path -ArgumentList @('service', 'stop') `
        -PassThru -WindowStyle Hidden
    if (-not $stop.WaitForExit($TimeoutMilliseconds)) {
        Stop-Process -Id $stop.Id -Force -ErrorAction SilentlyContinue
        $stop.WaitForExit()
    }
}

function Stop-EnoxianDaemonProcesses {
    Get-CimInstance Win32_Process -Filter "Name = 'enox.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -match '(?i)(^|\s)"?daemon"?\s+"?run"?(\s|$)' } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

$Tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP ("enoxian-" + [guid]::NewGuid()))
$changed = $false
$committed = $false
$serviceRestarted = $false
$existing = @{}
try {
    if (Test-Path $installedEnox -PathType Leaf) {
        Invoke-EnoxianStopWithTimeout $installedEnox
        if ($serviceWasInstalled) {
            & schtasks.exe /End /TN Enoxian *> $null
        }
        Stop-EnoxianDaemonProcesses
        Wait-EnoxianBinaryUnlocked $installedEnox
    }

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
    if (-not (Test-Path (Join-Path $Tmp 'enox.exe') -PathType Leaf)) {
        throw 'enoxian installer: archive is missing enox.exe'
    }

    $stagedVersion = & (Join-Path $Tmp 'enox.exe') --version
    if ($LASTEXITCODE -ne 0) { throw 'enoxian installer: downloaded enox failed its pre-install check' }
    if ($Version -ne 'latest' -and $stagedVersion -notmatch [regex]::Escape($Version.TrimStart('v'))) {
        throw "enoxian installer: downloaded version '$stagedVersion' does not match requested $Version"
    }

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    $backupDir = Join-Path $Tmp 'backup'
    New-Item -ItemType Directory -Path $backupDir | Out-Null
    $destination = Join-Path $BinDir 'enox.exe'
    $existing['enox.exe'] = Test-Path $destination -PathType Leaf
    if ($existing['enox.exe']) { Copy-Item $destination (Join-Path $backupDir 'enox.exe') }
    $stagedDestination = Join-Path $BinDir '.enox.exe.new'
    Copy-Item (Join-Path $Tmp 'enox.exe') $stagedDestination -Force
    if ($existing['enox.exe']) {
        $replaceBackup = Join-Path $backupDir 'enox.exe.replace-backup'
        # File.Replace can fail for executables on Windows even after the
        # process exits (for example while an image-section handle is being
        # released). Rename the old binary out of the way first, then move the
        # staged binary into place. The catch block below still has the copied
        # backup available if the second move fails.
        Wait-EnoxianBinaryUnlocked $destination
        Move-Item $destination $replaceBackup
        $changed = $true
        Move-Item $stagedDestination $destination
        Remove-Item $replaceBackup -Force -ErrorAction SilentlyContinue
    } else {
        Move-Item $stagedDestination $destination
        $changed = $true
    }

    & (Join-Path $BinDir 'enox.exe') --version *> $null
    if ($LASTEXITCODE -ne 0) { throw 'enoxian installer: installed enox failed its post-install check' }
    & (Join-Path $BinDir 'enox.exe') update --record-stable *> $null
    if ($LASTEXITCODE -ne 0) { throw 'enoxian installer: failed to record the stable update channel' }
    $committed = $true
    Remove-Item (Join-Path $BinDir 'enoxd.exe') -Force -ErrorAction SilentlyContinue

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $pathEntries = @($userPath -split ';' | Where-Object { $_ })
    if (-not $NoPathUpdate -and -not ($pathEntries | Where-Object { $_.Trim().TrimEnd('\') -ieq $BinDir.TrimEnd('\') })) {
        $newPath = if ($userPath) { "$userPath;$BinDir" } else { $BinDir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Write-Host "enoxian installer: added $BinDir to your user PATH; open a new terminal"
    }
    Write-Host "enoxian installer: installed $stagedVersion"
    Write-Host "enoxian installer: binary: $BinDir\enox.exe"
    if ($EnableService) {
        & (Join-Path $BinDir 'enox.exe') service install --force
        if ($LASTEXITCODE -ne 0) { throw 'enoxian installer: enox installed, but login service setup failed' }
        $serviceRestarted = $true
    } elseif ($serviceWasInstalled) {
        & (Join-Path $BinDir 'enox.exe') service start
        if ($LASTEXITCODE -ne 0) { throw 'enoxian installer: enox installed, but the existing login service failed to restart' }
        $serviceRestarted = $true
        Write-Host 'enoxian installer: existing login service preserved and restarted'
    } else {
        Write-Host "enoxian installer: optional: run 'enox service install' to start automatically when you sign in"
    }
    Write-Host "enoxian installer: agents: adapters require system Node.js 22+ with npm"
    Write-Host "enoxian installer: next: open a new terminal and run 'enox init --name my-project'"
} catch {
    if ($changed -and -not $committed) {
        Write-Warning 'enoxian installer: installation failed; restoring the previous installation'
        foreach ($binary in @('enox.exe')) {
            $destination = Join-Path $BinDir $binary
            if ($existing.ContainsKey($binary) -and $existing[$binary]) {
                Copy-Item (Join-Path $Tmp "backup\$binary") $destination -Force
            } else {
                Remove-Item $destination -Force -ErrorAction SilentlyContinue
            }
        }
    }
    if ($serviceWasInstalled -and -not $serviceRestarted -and (Test-Path $installedEnox -PathType Leaf)) {
        try {
            & $installedEnox service start *> $null
            if ($LASTEXITCODE -eq 0) {
                Write-Warning 'enoxian installer: restored the previous login service after the failed update'
            } else {
                Write-Warning "enoxian installer: could not restore the previous login service; run 'enox service start'"
            }
        } catch {
            Write-Warning "enoxian installer: could not restore the previous login service; run 'enox service start'"
        }
    }
    throw
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
