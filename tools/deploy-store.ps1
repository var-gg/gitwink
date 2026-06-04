#!/usr/bin/env pwsh
#requires -Version 7

<#
.SYNOPSIS
  Fetch the exact latest Microsoft Store MSIX bundle that CI built, for manual
  Partner Center upload — removing the "which file?" risk from manual submission.

.DESCRIPTION
  The Store auto-publish path (store.yml -> msstore) is blocked on Microsoft Entra
  app association, so Store submission stays manual for now. The risky part of a
  manual upload was *picking the right file* (correct version + the x64/arm64
  .msixbundle, not a stray local build). This removes that risk:

    1. Find the newest non-expired `gitwink-msix` artifact (by artifact date, NOT
       run conclusion -- see note below).
    2. Download it into one fixed, gitignored folder (wiping any stale copy first).
    3. Print the bundle's version + size; unless -NoLaunch, open Explorer with it
       pre-selected and open the Partner Center product page.

  You then drag that ONE file into the submission's Packages step and submit.
  Every other channel (GitHub Release, winget, Scoop) is already fully automated by
  release.yml on tag push -- this covers only the Store (MSIX) channel.

  NOTE on "by artifact, not run conclusion": store.yml's run turns red when the
  (still-unconfigured) Store auth/publish step fails, but the MSIX build + artifact
  upload run BEFORE that step and succeed. Filtering on conclusion=success would
  therefore grab a stale OLDER release (e.g. v0.4.0 instead of v0.5.0). So we
  resolve the artifact directly and ignore run color.

.PARAMETER Tag
  Optional. Restrict to artifacts whose run was on this ref (e.g. v0.5.0 -- only
  matches if store.yml actually ran on that tag). Default: newest of any ref.

.PARAMETER OutDir
  Where to drop the bundle. Default: <repo>/.store-upload (gitignored).

.PARAMETER NoLaunch
  Download + report only; don't open Explorer or the browser.

.EXAMPLE
  pwsh tools/deploy-store.ps1
.EXAMPLE
  pwsh tools/deploy-store.ps1 -NoLaunch

.NOTES
  Prereq: GitHub CLI (gh) installed and authenticated (gh auth status).
#>
[CmdletBinding()]
param(
    [string]$Tag,
    [string]$OutDir = (Join-Path $PSScriptRoot '..\.store-upload'),
    [switch]$NoLaunch
)

$ErrorActionPreference = 'Stop'

$Repo       = 'var-gg/gitwink'
$Artifact   = 'gitwink-msix'
$ProductId  = '9P0S21GJD53F'
$ProductUrl = "https://partner.microsoft.com/dashboard/products/$ProductId/overview"

function Die([string]$msg) { Write-Host "[X] $msg" -ForegroundColor Red; exit 1 }

# 0. gh present + authenticated -------------------------------------------------
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Die 'GitHub CLI (gh) not found. Install from https://cli.github.com/ then: gh auth login'
}
gh auth status 2>$null | Out-Null
if ($LASTEXITCODE -ne 0) { Die 'gh is not authenticated. Run: gh auth login' }

# 1. resolve the run -- by newest non-expired ARTIFACT, not run conclusion ------
Write-Host "-> Finding newest '$Artifact' artifact..." -ForegroundColor Cyan
$arts = (gh api "repos/$Repo/actions/artifacts?name=$Artifact&per_page=30" | ConvertFrom-Json).artifacts |
        Where-Object { -not $_.expired }
if ($Tag) { $arts = $arts | Where-Object { $_.workflow_run.head_branch -eq $Tag } }
if (-not $arts) {
    Die ("No non-expired '$Artifact' artifact found" + $(if ($Tag) { " for ref $Tag" } else { '' }) +
         '. Re-run store.yml: Actions -> MSIX (Microsoft Store) -> Run workflow.')
}
$art   = $arts | Sort-Object { [datetime]$_.created_at } -Descending | Select-Object -First 1
$runId = $art.workflow_run.id
Write-Host "   artifact #$($art.id)  run #$runId  [$($art.workflow_run.head_branch)]  $($art.created_at)" -ForegroundColor DarkGray

# 2. wipe stale + download ------------------------------------------------------
if (Test-Path $OutDir) { Remove-Item $OutDir -Recurse -Force }
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$OutDir = (Resolve-Path $OutDir).Path

Write-Host "-> Downloading artifact from run #$runId..." -ForegroundColor Cyan
& gh run download $runId --repo $Repo --name $Artifact --dir $OutDir
if ($LASTEXITCODE -ne 0) { Die "Download failed for run #$runId." }

# 3. locate the bundle ----------------------------------------------------------
$bundle = Get-ChildItem $OutDir -Recurse -Filter *.msixbundle | Select-Object -First 1
if (-not $bundle) { Die "No .msixbundle inside the '$Artifact' artifact." }

Write-Host ''
Write-Host '[OK] Ready to upload:' -ForegroundColor Green
Write-Host "     $($bundle.Name)" -ForegroundColor White
Write-Host "     $($bundle.FullName)" -ForegroundColor DarkGray
Write-Host "     $([math]::Round($bundle.Length / 1MB, 1)) MB" -ForegroundColor DarkGray
Write-Host ''

if (-not $NoLaunch) {
    Start-Process explorer.exe "/select,`"$($bundle.FullName)`""
    Start-Process $ProductUrl
    Write-Host 'Next steps in Partner Center (page opened):' -ForegroundColor Yellow
    Write-Host '  1. Product -> Start update / 업데이트 시작' -ForegroundColor Yellow
    Write-Host '  2. Packages step -> drag the .msixbundle above (Explorer has it selected)' -ForegroundColor Yellow
    Write-Host '  3. Submit / 제출' -ForegroundColor Yellow
} else {
    Write-Host '(-NoLaunch: skipped opening Explorer + Partner Center)' -ForegroundColor DarkGray
}
