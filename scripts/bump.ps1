# Bump the version, commit, tag, and push — triggers the GitHub Actions release build.
#
# Usage:
#   .\scripts\bump.ps1 patch   # 0.1.0 → 0.1.1
#   .\scripts\bump.ps1 minor   # 0.1.0 → 0.2.0
#   .\scripts\bump.ps1 major   # 0.1.0 → 1.0.0
#   .\scripts\bump.ps1 0.3.0   # set exact version
param(
    [Parameter(Mandatory)][string]$Part
)

$ErrorActionPreference = "Stop"
$CargoToml = Join-Path (Split-Path $PSScriptRoot -Parent) "Cargo.toml"

# ── Read current version ──────────────────────────────────────────────────────
$content = Get-Content $CargoToml -Raw
if ($content -notmatch 'version\s*=\s*"(\d+)\.(\d+)\.(\d+)"') {
    throw "Could not find version in Cargo.toml"
}
$major = [int]$Matches[1]
$minor = [int]$Matches[2]
$patch = [int]$Matches[3]
$current = "$major.$minor.$patch"

# ── Compute new version ───────────────────────────────────────────────────────
$new = switch ($Part) {
    "major" { "$($major + 1).0.0" }
    "minor" { "$major.$($minor + 1).0" }
    "patch" { "$major.$minor.$($patch + 1)" }
    default {
        if ($Part -match '^\d+\.\d+\.\d+$') { $Part }
        else { throw "Usage: bump.ps1 major|minor|patch|<version>" }
    }
}

Write-Host "▶ Bumping $current → $new"

# ── Update Cargo.toml ─────────────────────────────────────────────────────────
$updated = $content -replace '(?m)(^version\s*=\s*)"[^"]*"', "`${1}`"$new`""
Set-Content $CargoToml $updated -NoNewline

# ── cargo check to update Cargo.lock ─────────────────────────────────────────
Write-Host "▶ Updating Cargo.lock..."
Push-Location (Split-Path $PSScriptRoot -Parent)
cargo check --quiet 2>$null
Pop-Location

# ── Commit, tag, push ─────────────────────────────────────────────────────────
$tag = "v$new"
Write-Host "▶ Committing and tagging $tag..."
git add (Join-Path (Split-Path $PSScriptRoot -Parent) "Cargo.toml") `
        (Join-Path (Split-Path $PSScriptRoot -Parent) "Cargo.lock")
git commit -m "chore: bump version to $new"
git tag $tag
git push
git push origin $tag

Write-Host ""
Write-Host "✦ Released $tag — GitHub Actions is building the binaries."
Write-Host "  Watch: https://github.com/suzent/enochian/actions"
Write-Host ""
Write-Host "  Once the build finishes, deploy:"
Write-Host "    .\scripts\deploy-rendezvous.ps1 <host> -Update"
