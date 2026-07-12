#!/usr/bin/env pwsh
#requires -Version 7.2

<#
.SYNOPSIS
  Validate or publish one Microsoft Store MSIX submission, including checked-in
  localized screenshots, without driving Partner Center in a browser.

.DESCRIPTION
  Without -Publish this command only validates local inputs and performs no
  network or Store mutation. With -Publish it uses the official packaged-app
  submission API to refuse existing drafts, preflight the inherited listing,
  create one API-owned draft, upload one package-and-image ZIP, and commit once.

  A failure before commit rolls back only the draft created by this invocation.
  Once commit has been attempted the draft is preserved because its server-side
  state is no longer safe to infer.

.NOTES
  -Publish reads AZURE_AD_TENANT_ID, AZURE_AD_APPLICATION_CLIENT_ID, and
  AZURE_AD_APPLICATION_SECRET from the environment. Secrets never enter the
  command line.
#>
[CmdletBinding()]
param(
    [string]$PackagePath,
    [string]$ManifestPath = (Join-Path $PSScriptRoot '..\store\listing\screenshots.json'),
    [switch]$Publish,
    [switch]$KeepFailedDraft,
    [ValidateRange(60, 1800)]
    [int]$PollTimeoutSeconds = 600,
    [ValidateRange(5, 60)]
    [int]$PollIntervalSeconds = 20
)

$ErrorActionPreference = 'Stop'
$DevCenterBaseUrl = 'https://manage.devcenter.microsoft.com'
$ListingTool = Join-Path $PSScriptRoot 'store-listing.mjs'
$createdSubmissionId = $null
$commitAttempted = $false
$workRoot = $null

function Write-Step([string]$Message) {
    Write-Host "-> $Message" -ForegroundColor Cyan
}

function Assert-Command([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command '$Name' was not found."
    }
}

function Invoke-ListingTool([string[]]$Arguments, [switch]$Json) {
    $output = @(& node $ListingTool @Arguments)
    if ($LASTEXITCODE -ne 0) {
        throw "Store listing tool failed with exit code $LASTEXITCODE."
    }
    if (-not $Json) { return $output }
    $lastLine = $output |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        Select-Object -Last 1
    if (-not $lastLine) {
        throw 'Store listing tool returned no result.'
    }
    try {
        return $lastLine | ConvertFrom-Json -Depth 100
    } catch {
        throw 'Store listing tool returned an invalid result.'
    }
}

function Write-JsonFile([string]$Path, [object]$Value) {
    $json = $Value | ConvertTo-Json -Depth 100
    [IO.File]::WriteAllText(
        $Path,
        $json + [Environment]::NewLine,
        [Text.UTF8Encoding]::new($false)
    )
}

function Get-HeaderValue([object]$Headers, [string[]]$Names) {
    if ($null -eq $Headers) { return $null }
    foreach ($name in $Names) {
        foreach ($key in $Headers.Keys) {
            if ([string]$key -ieq $name) {
                return [string]($Headers[$key] | Select-Object -First 1)
            }
        }
    }
    return $null
}

function Get-SafeApiError([object]$Response) {
    if ($null -eq $Response) { return $null }
    $candidates = @()
    if ($Response.errors) { $candidates += @($Response.errors) }
    if ($Response.statusDetails -and $Response.statusDetails.errors) {
        $candidates += @($Response.statusDetails.errors)
    }
    if ($candidates.Count -gt 0) {
        return ($candidates | ForEach-Object {
            @($_.code, $_.message, $_.target) |
                Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) } |
                Join-String -Separator ': '
        }) -join '; '
    }
    if ($Response.message -or $Response.code -or $Response.target) {
        return @($Response.code, $Response.message, $Response.target) |
            Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) } |
            Join-String -Separator ': '
    }
    if ($Response -is [string]) {
        $safe = $Response -replace 'https://[^\s"'']+\?[^\s"'']+', '<redacted-sas-url>'
        if ($safe.Length -gt 1000) { $safe = $safe.Substring(0, 1000) }
        return $safe.Trim()
    }
    return $null
}

function Get-DevCenterAccessToken(
    [string]$TenantId,
    [string]$ClientId,
    [string]$ClientSecret
) {
    $parameters = @{
        Method      = 'Post'
        Uri         = "https://login.microsoftonline.com/$TenantId/oauth2/v2.0/token"
        ContentType = 'application/x-www-form-urlencoded'
        Body        = @{
            client_id     = $ClientId
            client_secret = $ClientSecret
            scope         = "$DevCenterBaseUrl/.default"
            grant_type    = 'client_credentials'
        }
    }
    try {
        $response = Invoke-RestMethod @parameters
    } catch {
        throw 'Microsoft Entra authentication failed. Credentials were not printed.'
    }
    if ([string]::IsNullOrWhiteSpace([string]$response.access_token)) {
        throw 'Microsoft Entra authentication returned no access token.'
    }
    return [string]$response.access_token
}

function Invoke-DevCenterJson(
    [ValidateSet('GET', 'POST', 'PUT', 'DELETE')]
    [string]$Method,
    [string]$Path,
    [string]$AccessToken,
    [string]$TenantId,
    [string]$Body
) {
    $statusCode = 0
    $responseHeaders = $null
    $parameters = @{
        Method                  = $Method
        Uri                     = "$DevCenterBaseUrl$Path"
        Headers                 = @{
            Authorization = "Bearer $AccessToken"
            TenantId      = $TenantId
        }
        SkipHttpErrorCheck      = $true
        StatusCodeVariable      = 'statusCode'
        ResponseHeadersVariable = 'responseHeaders'
    }
    if ($PSBoundParameters.ContainsKey('Body')) {
        $parameters.Body = $Body
        $parameters.ContentType = 'application/json'
    }
    $response = Invoke-RestMethod @parameters
    if ($statusCode -lt 200 -or $statusCode -ge 300) {
        $correlationId = Get-HeaderValue $responseHeaders @(
            'x-correlation-id',
            'ms-correlationid',
            'request-id'
        )
        $detail = Get-SafeApiError $response
        $message = "Dev Center API $Method $Path failed with HTTP $statusCode"
        if ($correlationId) { $message += " (correlation $correlationId)" }
        if ($detail) { $message += ": $detail" }
        throw $message
    }
    return $response
}

function Send-StoreUploadZip([string]$UploadUrl, [string]$ZipPath) {
    if ([string]::IsNullOrWhiteSpace($UploadUrl)) {
        throw 'Submission has no package upload URL.'
    }
    $safeUploadUrl = $UploadUrl.Replace('+', '%2B')
    $client = [Net.Http.HttpClient]::new()
    $client.Timeout = [TimeSpan]::FromMinutes(15)
    $stream = [IO.File]::OpenRead($ZipPath)
    $content = [Net.Http.StreamContent]::new($stream)
    $content.Headers.ContentType =
        [Net.Http.Headers.MediaTypeHeaderValue]::new('application/zip')
    $request = [Net.Http.HttpRequestMessage]::new(
        [Net.Http.HttpMethod]::Put,
        $safeUploadUrl
    )
    $request.Headers.TryAddWithoutValidation('x-ms-blob-type', 'BlockBlob') |
        Out-Null
    $request.Headers.TryAddWithoutValidation('x-ms-version', '2021-12-02') |
        Out-Null
    $request.Headers.TryAddWithoutValidation(
        'ms-correlationid',
        [guid]::NewGuid().ToString()
    ) | Out-Null
    $request.Content = $content
    try {
        $response = $client.SendAsync($request).GetAwaiter().GetResult()
        try {
            if (-not $response.IsSuccessStatusCode) {
                $requestId = $null
                if ($response.Headers.Contains('x-ms-request-id')) {
                    $requestId =
                        $response.Headers.GetValues('x-ms-request-id') |
                        Select-Object -First 1
                }
                $message =
                    "Store upload failed with HTTP $([int]$response.StatusCode)"
                if ($requestId) { $message += " (request $requestId)" }
                throw $message
            }
        } finally {
            $response.Dispose()
        }
    } finally {
        $request.Dispose()
        $client.Dispose()
    }
}

function Remove-StoreWorkingDirectory([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path) -or
        -not (Test-Path -LiteralPath $Path)) {
        return
    }
    $resolved = [IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Path).Path)
    $trimChars = [char[]]@('\', '/')
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd($trimChars)
    $parent = [IO.Path]::GetDirectoryName($resolved).TrimEnd($trimChars)
    $leaf = [IO.Path]::GetFileName($resolved)
    if ($parent -ne $tempRoot -or $leaf -notlike 'gitwink-store-*') {
        throw "Refusing to remove unexpected Store working directory: $resolved"
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}

Assert-Command node
$ManifestPath = (Resolve-Path -LiteralPath $ManifestPath).Path
Write-Step 'Validating Microsoft Store screenshot manifest'
Invoke-ListingTool @('validate', $ManifestPath) |
    ForEach-Object { Write-Host $_ }

if ($PackagePath) {
    $PackagePath = (Resolve-Path -LiteralPath $PackagePath).Path
    $extension = [IO.Path]::GetExtension($PackagePath).ToLowerInvariant()
    if ($extension -notin @('.msix', '.msixbundle', '.msixupload')) {
        throw "Unsupported Store package extension: $extension"
    }
    if ((Get-Item -LiteralPath $PackagePath).Length -eq 0) {
        throw 'Store package must not be empty.'
    }
}

if (-not $Publish) {
    if ($PackagePath) {
        Write-Host (
            "[OK] Local package input is valid: " +
            [IO.Path]::GetFileName($PackagePath)
        ) -ForegroundColor Green
    }
    Write-Host '[OK] Validation only; Partner Center was not contacted.' -ForegroundColor Green
    exit 0
}

if ([string]::IsNullOrWhiteSpace($PackagePath)) {
    throw '-PackagePath is required with -Publish.'
}
$PackagePath = (Resolve-Path -LiteralPath $PackagePath).Path
$manifest = Get-Content -LiteralPath $ManifestPath -Raw |
    ConvertFrom-Json -Depth 100
$productId = [string]$manifest.productId

$tenantId = [string]$env:AZURE_AD_TENANT_ID
$clientId = [string]$env:AZURE_AD_APPLICATION_CLIENT_ID
$clientSecret = [string]$env:AZURE_AD_APPLICATION_SECRET
if (@($tenantId, $clientId, $clientSecret) |
    Where-Object { [string]::IsNullOrWhiteSpace($_) }) {
    throw (
        'Store publication requires AZURE_AD_TENANT_ID, ' +
        'AZURE_AD_APPLICATION_CLIENT_ID, and AZURE_AD_APPLICATION_SECRET.'
    )
}

$workRoot = Join-Path (
    [IO.Path]::GetTempPath()
) "gitwink-store-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $workRoot | Out-Null

try {
    Write-Step 'Authenticating to the packaged-app submission API'
    $accessToken = Get-DevCenterAccessToken $tenantId $clientId $clientSecret
    $encodedProductId = [Uri]::EscapeDataString($productId)
    $application = Invoke-DevCenterJson GET "/v1.0/my/applications/$encodedProductId" $accessToken $tenantId
    $applicationId = [string]$application.id
    if ([string]::IsNullOrWhiteSpace($applicationId)) {
        throw "Store product $productId returned no application id."
    }
    if ($application.pendingApplicationSubmission -and
        $application.pendingApplicationSubmission.id) {
        throw (
            "Store product $productId already has pending submission " +
            "$($application.pendingApplicationSubmission.id). " +
            'The automation will not modify or delete an existing draft.'
        )
    }

    $lastPublishedId =
        [string]$application.lastPublishedApplicationSubmission.id
    if ([string]::IsNullOrWhiteSpace($lastPublishedId)) {
        throw (
            'The first Store submission must be completed before this update ' +
            'automation can run.'
        )
    }
    $encodedApplicationId = [Uri]::EscapeDataString($applicationId)
    $encodedPublishedId = [Uri]::EscapeDataString($lastPublishedId)
    $publishedPathApi =
        "/v1.0/my/applications/$encodedApplicationId/submissions/$encodedPublishedId"
    $published = Invoke-DevCenterJson GET $publishedPathApi $accessToken $tenantId
    $publishedPath = Join-Path $workRoot 'published-submission.json'
    Write-JsonFile $publishedPath $published
    Write-Step 'Preflighting package replacement and localized listing ownership'
    $preflight = Invoke-ListingTool @(
        'preflight',
        $ManifestPath,
        $publishedPath,
        $PackagePath
    ) -Json
    Write-Host (
        "   package $($preflight.packageName); " +
        "screenshots $($preflight.screenshotCount); " +
        "replaced locales $(@($preflight.replacedLocales).Count)"
    ) -ForegroundColor DarkGray

    Write-Step 'Creating one API-owned Store draft'
    $createPath = "/v1.0/my/applications/$encodedApplicationId/submissions"
    $created = Invoke-DevCenterJson POST $createPath $accessToken $tenantId
    $createdSubmissionId = [string]$created.id
    if ([string]::IsNullOrWhiteSpace($createdSubmissionId)) {
        throw 'Dev Center created a submission but returned no submission id.'
    }
    $encodedSubmissionId = [Uri]::EscapeDataString($createdSubmissionId)
    $submissionPath =
        "/v1.0/my/applications/$encodedApplicationId/submissions/$encodedSubmissionId"
    $draft = Invoke-DevCenterJson GET $submissionPath $accessToken $tenantId
    if ([string]::IsNullOrWhiteSpace([string]$draft.fileUploadUrl)) {
        throw (
            "Submission $createdSubmissionId has no API upload URL. " +
            'Do not mix a Partner Center-created draft with this API workflow.'
        )
    }

    $draftPath = Join-Path $workRoot 'draft-submission.json'
    Write-JsonFile $draftPath $draft
    $preparedRoot = Join-Path $workRoot 'prepared'
    $prepared = Invoke-ListingTool @(
        'prepare',
        $ManifestPath,
        $draftPath,
        $PackagePath,
        $preparedRoot
    ) -Json
    $metadataJson = Get-Content -LiteralPath $prepared.metadataPath -Raw

    Write-Step 'Updating the draft metadata once'
    $updated = Invoke-DevCenterJson PUT $submissionPath $accessToken $tenantId $metadataJson
    $uploadUrl = [string]$updated.fileUploadUrl
    if ([string]::IsNullOrWhiteSpace($uploadUrl)) {
        $updated =
            Invoke-DevCenterJson GET $submissionPath $accessToken $tenantId
        $uploadUrl = [string]$updated.fileUploadUrl
    }

    $uploadZip = Join-Path $workRoot 'Upload.zip'
    Compress-Archive -Path (Join-Path $prepared.stagingDirectory '*') -DestinationPath $uploadZip -CompressionLevel Optimal
    Write-Step 'Uploading the exact package and pending screenshots as one ZIP'
    Send-StoreUploadZip $uploadUrl $uploadZip

    Write-Step "Committing Store submission $createdSubmissionId"
    $commitAttempted = $true
    $null = Invoke-DevCenterJson POST "$submissionPath/commit" $accessToken $tenantId

    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($PollTimeoutSeconds)
    do {
        $status =
            Invoke-DevCenterJson GET "$submissionPath/status" $accessToken $tenantId
        $statusName = [string]$status.status
        if ([string]::IsNullOrWhiteSpace($statusName)) {
            throw "Submission $createdSubmissionId returned no ingestion status."
        }
        Write-Host "   ingestion status: $statusName" -ForegroundColor DarkGray
        if ($statusName -eq 'CommitFailed') {
            $detail = Get-SafeApiError $status
            if (-not $detail) {
                $detail = 'Partner Center reported CommitFailed without details.'
            }
            throw "Store submission $createdSubmissionId failed ingestion: $detail"
        }
        if ($statusName -notin @('PendingCommit', 'CommitStarted')) { break }
        if ([DateTimeOffset]::UtcNow -ge $deadline) {
            throw (
                "Timed out waiting for Store ingestion of submission " +
                "$createdSubmissionId. The commit was attempted, so the draft " +
                'was preserved for status inspection.'
            )
        }
        Start-Sleep -Seconds $PollIntervalSeconds
    } while ($true)

    Write-Host (
        "[OK] Store submission $createdSubmissionId was accepted for " +
        "downstream processing (status: $statusName)."
    ) -ForegroundColor Green
} catch {
    if ($createdSubmissionId -and
        -not $commitAttempted -and
        -not $KeepFailedDraft) {
        try {
            Write-Warning "Rolling back uncommitted Store draft $createdSubmissionId."
            $encodedSubmissionId =
                [Uri]::EscapeDataString($createdSubmissionId)
            $deletePath =
                "/v1.0/my/applications/$encodedApplicationId/submissions/$encodedSubmissionId"
            $null = Invoke-DevCenterJson DELETE $deletePath $accessToken $tenantId
        } catch {
            Write-Warning (
                "Could not roll back Store draft $createdSubmissionId. " +
                'Remove only that draft before retrying.'
            )
        }
    } elseif ($createdSubmissionId -and $commitAttempted) {
        Write-Warning (
            "Commit was attempted for Store submission $createdSubmissionId; " +
            'it was preserved for status inspection.'
        )
    }
    throw
} finally {
    if ($workRoot) {
        Remove-StoreWorkingDirectory $workRoot
    }
}
