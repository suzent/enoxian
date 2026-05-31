# Deploy enoxd to a Linux VPS as a rendezvous server.
#
# Build modes (in order of preference):
#   default          Download latest release binary from GitHub (fastest, no build needed)
#   -BuildOnRemote   Pipe source into Docker on the VPS and build there
#   -Local           Cross-compile locally using cross (Docker) or WSL2
#
# Usage:
#   .\scripts\deploy-rendezvous.ps1 user@host [-Port N] [-BuildOnRemote] [-Local] [-Update]
#
# Examples:
#   .\scripts\deploy-rendezvous.ps1 root@sg.example.com
#   .\scripts\deploy-rendezvous.ps1 root@sg.example.com -Update
#   .\scripts\deploy-rendezvous.ps1 root@sg.example.com -BuildOnRemote
param(
    [Parameter(Mandatory)][string]$Target,
    [int]$Port = 36521,
    [ValidateSet("x86_64","aarch64")][string]$Arch = "x86_64",
    [switch]$BuildOnRemote,
    [switch]$Local,
    [switch]$Update,
    [string]$Token = $env:GITHUB_TOKEN
)

$ErrorActionPreference = "Stop"
$RepoDir = Split-Path $PSScriptRoot -Parent
$Repo    = "suzent/enoxian"

# Load .env from repo root if token not already provided
if (-not $Token) {
    $EnvFile = Join-Path $RepoDir ".env"
    if (Test-Path $EnvFile) {
        Get-Content $EnvFile | Where-Object { $_ -match '^\s*GITHUB_TOKEN\s*=\s*(.+)' } | ForEach-Object {
            $Token = $Matches[1].Trim().Trim('"').Trim("'")
        }
    }
}
$Asset   = "enoxd-linux-$Arch"

# ── Get binary ────────────────────────────────────────────────────────────────
if ($BuildOnRemote) {
    # ── Build inside Docker on the VPS ───────────────────────────────────────
    Write-Host "▶ Packing source..."
    $TarFile = Join-Path $env:TEMP "enoxian-src.tar.gz"
    tar -czf $TarFile `
        --exclude=".git" --exclude="target" --exclude="node_modules" `
        -C $RepoDir .
    if ($LASTEXITCODE -ne 0) { throw "tar failed" }

    Write-Host "▶ Building on remote via Docker (piping source)..."
    Get-Content $TarFile -AsByteStream | ssh $Target @"
docker run --rm -i \
    -v enoxian-cargo-cache:/usr/local/cargo/registry \
    -v enoxian-out:/out \
    rust:alpine \
    sh -c 'apk add --no-cache musl-dev && mkdir /src && tar -xzf - -C /src && cd /src && cargo build --release --bin enoxd && cp target/release/enoxd /out/enoxd'
"@
    if ($LASTEXITCODE -ne 0) { throw "Remote build failed" }

    ssh $Target "docker run --rm -v enoxian-out:/out busybox cp /out/enoxd /tmp/enoxd && chmod +x /tmp/enoxd"
    if ($LASTEXITCODE -ne 0) { throw "Failed to extract binary from Docker volume" }

} elseif ($Local) {
    # ── Cross-compile locally ─────────────────────────────────────────────────
    $LinuxTarget = "$Arch-unknown-linux-musl"
    $BinaryPath  = Join-Path $RepoDir "target\$LinuxTarget\release\enoxd"

    $useCross = $null -ne (Get-Command cross -ErrorAction SilentlyContinue)
    $useWsl   = $null -ne (Get-Command wsl   -ErrorAction SilentlyContinue)

    Write-Host "▶ Building enoxd for Linux ($LinuxTarget)..."
    Push-Location $RepoDir

    if ($useCross) {
        Write-Host "  Using: cross (Docker)"
        cross build --release --bin enoxd --target $LinuxTarget
        if ($LASTEXITCODE -ne 0) { throw "cross build failed" }
    } elseif ($useWsl) {
        Write-Host "  Using: WSL2"
        $wslRepoDir = (wsl wslpath ($RepoDir.Replace('\','/'))).Trim()
        $tmpScript  = Join-Path $env:TEMP "enox-wsl-build.sh"
        @"
#!/usr/bin/env bash
set -eo pipefail
command -v musl-gcc &>/dev/null || sudo apt-get install -y -q build-essential musl-tools
. "`$HOME/.cargo/env"
rustup target add $LinuxTarget
cd "$wslRepoDir"
cargo build --release --bin enoxd --target $LinuxTarget 2>&1
"@ | Set-Content -Encoding utf8 $tmpScript
        $wslTmp = (wsl wslpath ($tmpScript.Replace('\','/'))).Trim()
        wsl bash $wslTmp
        if ($LASTEXITCODE -ne 0) { throw "WSL build failed" }
    } else {
        Pop-Location
        Write-Host "No local build tool found. Use default (GitHub release) or -BuildOnRemote."
        throw "No build method available"
    }

    Pop-Location
    if (-not (Test-Path $BinaryPath)) { throw "Binary not found at $BinaryPath" }
    $size = (Get-Item $BinaryPath).Length / 1MB
    Write-Host "  Built: $BinaryPath ($([math]::Round($size,1)) MB)"
    scp $BinaryPath "${Target}:/tmp/enoxd"
    if ($LASTEXITCODE -ne 0) { throw "scp failed" }

} else {
    # ── Download latest GitHub release (default) ──────────────────────────────
    Write-Host "▶ Downloading latest release from github.com/$Repo..."
    $headers = @{}
    if ($Token) { $headers["Authorization"] = "Bearer $Token" }
    $release  = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest" -Headers $headers
    $assetObj = $release.assets | Where-Object { $_.name -eq $Asset }
    if (-not $assetObj) {
        throw "Asset '$Asset' not found in latest release. Run the release workflow first, or use -BuildOnRemote."
    }
    # Use the API assets URL (not browser_download_url) so curl doesn't lose
    # the auth header on the GitHub→S3 cross-domain redirect.
    $apiUrl   = "https://api.github.com/repos/$Repo/releases/assets/$($assetObj.id)"
    Write-Host "  $($release.tag_name): $($assetObj.name)"
    $curlAuth = if ($Token) { "-H 'Authorization: Bearer $Token'" } else { "" }
    ssh $Target "curl -fsSL $curlAuth -H 'Accept: application/octet-stream' '$apiUrl' -o /tmp/enoxd && chmod +x /tmp/enoxd"
    if ($LASTEXITCODE -ne 0) { throw "Download failed" }
}

# ── Install on the VPS ────────────────────────────────────────────────────────
if ($Update) {
    Write-Host "▶ Updating binary and restarting service..."
    ssh $Target @'
set -e
cp /tmp/enoxd /usr/local/bin/enoxd
chmod +x /usr/local/bin/enoxd
systemctl restart enoxd-bootstrap
sleep 1
systemctl is-active enoxd-bootstrap && echo "✦ Service restarted" \
    || { journalctl -u enoxd-bootstrap -n 10 --no-pager; exit 1; }
'@
} else {
    Write-Host "▶ Running setup on $Target..."
    $SetupScript = Join-Path $PSScriptRoot "setup-rendezvous.sh"
    scp $SetupScript "${Target}:/tmp/setup-rendezvous.sh"
    if ($LASTEXITCODE -ne 0) { throw "scp of setup script failed" }
    ssh $Target "bash /tmp/setup-rendezvous.sh --port $Port"
}

if ($LASTEXITCODE -ne 0) { throw "Remote setup failed" }
