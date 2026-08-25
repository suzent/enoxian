# Prepare a version bump commit on a `chore/release-vX.Y.Z` branch.
#
# Merging that branch is what cuts the release: .github/workflows/tag-release.yml
# tags the merge commit and starts the release pipeline. Normally you do not run
# this by hand — use the "Prepare release" workflow in the Actions tab.
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
$RepoDir = Split-Path $PSScriptRoot -Parent
$CargoToml = Join-Path $RepoDir "Cargo.toml"
$Changelog = Join-Path $RepoDir "CHANGELOG.md"
$Readme = Join-Path $RepoDir "README.md"
$RepoUrl = "https://github.com/suzent/enoxian"

# CHANGELOG.md, Cargo.toml, and README.md are LF in the repository (see
# .gitattributes). Write LF explicitly so a bump prepared on Windows does not
# produce a whole-file line-ending diff.
function Set-LfContent {
    param([string]$Path, [string]$Text)
    $lf = $Text -replace "`r`n", "`n"
    [System.IO.File]::WriteAllText($Path, $lf, (New-Object System.Text.UTF8Encoding $false))
}

if ((git -C $RepoDir branch --show-current) -ne 'main') {
    throw 'Release preparation must start from main'
}
git -C $RepoDir diff --quiet
if ($LASTEXITCODE -ne 0) { throw 'Tracked files have uncommitted changes' }
git -C $RepoDir diff --cached --quiet
if ($LASTEXITCODE -ne 0) { throw 'The index has uncommitted changes' }

# A stale main silently drops CHANGELOG entries: anything merged after your last
# pull stays under [Unreleased] and ships in the *next* release notes while its
# code ships in this one.
git -C $RepoDir remote get-url origin *> $null
if ($LASTEXITCODE -eq 0) {
    git -C $RepoDir fetch --quiet origin main
    git -C $RepoDir merge-base --is-ancestor origin/main HEAD
    if ($LASTEXITCODE -ne 0) { throw "main is behind origin/main — run 'git pull' first" }
}

# ── Read current version ──────────────────────────────────────────────────────
$content = Get-Content $CargoToml -Raw
if ($content -notmatch '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"') {
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

# ── Refuse to cut an empty release ────────────────────────────────────────────
# The release notes are this section. An empty one fails the release pipeline
# after five platform builds; catching it here costs nothing.
$changelogContent = Get-Content $Changelog -Raw
$unreleased = [regex]::Match(
    $changelogContent,
    '(?ms)^## \[Unreleased\][^\n]*\n(.*?)(?=^## )'
)
if (-not $unreleased.Success -or -not ($unreleased.Groups[1].Value -match '\S')) {
    throw 'The [Unreleased] section is empty — nothing to release'
}

Write-Host "▶ Bumping $current → $new"
$branch = "chore/release-v$new"
git -C $RepoDir switch -c $branch
if ($LASTEXITCODE -ne 0) { throw "Could not create $branch" }

# Cut the Unreleased section and update compare links.
$date = Get-Date -Format 'yyyy-MM-dd'
$releaseHeader = "## [Unreleased]`n`n## [$new] — $date"
$changelogContent = [regex]::Replace(
    $changelogContent,
    '(?m)^## \[Unreleased\]\r?$',
    $releaseHeader,
    1
)
# Insert the new compare link directly below [Unreleased] so the reference list
# stays newest-first, instead of appending it to the end of the file.
$changelogContent = [regex]::Replace(
    $changelogContent,
    '(?m)^\[Unreleased\]: .+$',
    "[Unreleased]: $RepoUrl/compare/v$new...HEAD`n[$new]: $RepoUrl/compare/v$current...v$new",
    1
)
Set-LfContent $Changelog $changelogContent

# ── Update Cargo.toml ─────────────────────────────────────────────────────────
$updated = $content -replace '(?m)(^version\s*=\s*)"[^"]*"', "`${1}`"$new`""
Set-LfContent $CargoToml $updated

# Keep the user-facing package version synchronized.
$readmeContent = Get-Content $Readme -Raw
$readmeContent = [regex]::Replace(
    $readmeContent,
    '(?m)^The current package version is \*\*[^*]+\*\*\.',
    "The current package version is **$new**.",
    1
)
Set-LfContent $Readme $readmeContent

# ── Update Cargo.lock ─────────────────────────────────────────────────────────
# Only the workspace member's own version changed, so refresh just that entry
# rather than compiling the tree with `cargo check`.
Write-Host "▶ Updating Cargo.lock..."
Push-Location $RepoDir
try {
    cargo update --workspace --offline --quiet 2>$null
    if ($LASTEXITCODE -ne 0) { cargo update --workspace --quiet }
} finally {
    Pop-Location
}

# ── Commit only; CI must pass before the tag is created ───────────────────────
Write-Host "▶ Creating release preparation commit..."
git -C $RepoDir add $CargoToml (Join-Path $RepoDir "Cargo.lock") $Changelog $Readme
git -C $RepoDir commit -m "chore: bump version to $new"

Write-Host ""
Write-Host "✦ Prepared v$new on $branch. Push it and open a PR:"
Write-Host "    git push -u origin $branch"
Write-Host "  Merging that PR tags v$new and runs the release pipeline automatically."
