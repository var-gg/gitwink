#!/usr/bin/env pwsh
#requires -Version 7.2

$ErrorActionPreference = 'Stop'

# Dot-sourcing without -Publish performs local manifest validation only, then
# leaves the request-policy functions available for isolated network mocks.
. (Join-Path $PSScriptRoot 'publish-store.ps1')

$script:scenario = ''
$script:requestCount = 0
$script:sleepSeconds = @()
$script:requestContentTypes = @()
$script:requestHadBodies = @()
$script:requestMethods = @()

function Start-Sleep {
    param([int]$Seconds)
    $script:sleepSeconds += $Seconds
}

function Invoke-RestMethod {
    param(
        [string]$Method,
        [string]$Uri,
        [hashtable]$Headers,
        [switch]$SkipHttpErrorCheck,
        [string]$StatusCodeVariable,
        [string]$ResponseHeadersVariable,
        [object]$Body,
        [string]$ContentType
    )
    $script:requestCount++
    $script:requestContentTypes += $ContentType
    $script:requestHadBodies += $PSBoundParameters.ContainsKey('Body')
    $script:requestMethods += $Method
    if ($script:scenario -eq 'retry-get' -and
        $script:requestCount -eq 1) {
        Set-Variable -Name $StatusCodeVariable -Value 504 -Scope 1
        Set-Variable -Name $ResponseHeadersVariable -Value @{
            'Retry-After' = '1'
        } -Scope 1
        return '<!DOCTYPE html><html>gateway</html>'
    }
    if ($script:scenario -eq 'network-get' -and
        $script:requestCount -eq 1) {
        throw 'simulated connection reset'
    }
    if ($script:scenario -in @('reconcile-create', 'reconcile-non-api')) {
        if ($script:requestCount -eq 1) {
            Set-Variable -Name $StatusCodeVariable -Value 504 -Scope 1
            Set-Variable -Name $ResponseHeadersVariable -Value @{} -Scope 1
            return '<!DOCTYPE html><html>gateway</html>'
        }
        if ($script:requestCount -eq 2) {
            Set-Variable -Name $StatusCodeVariable -Value 200 -Scope 1
            Set-Variable -Name $ResponseHeadersVariable -Value @{} -Scope 1
            return [pscustomobject]@{
                id = 'app-id'
                pendingApplicationSubmission = [pscustomobject]@{
                    id = 'submission-11'
                }
            }
        }
        if ($script:requestCount -eq 3) {
            Set-Variable -Name $StatusCodeVariable -Value 200 -Scope 1
            Set-Variable -Name $ResponseHeadersVariable -Value @{} -Scope 1
            return [pscustomobject]@{
                id = 'submission-11'
                fileUploadUrl = if ($script:scenario -eq 'reconcile-create') {
                    'https://example.invalid/upload'
                } else {
                    $null
                }
            }
        }
    }
    if ($script:scenario -eq 'post-fail') {
        Set-Variable -Name $StatusCodeVariable -Value 504 -Scope 1
        Set-Variable -Name $ResponseHeadersVariable -Value @{} -Scope 1
        return '<!DOCTYPE html><html>gateway</html>'
    }
    Set-Variable -Name $StatusCodeVariable -Value 200 -Scope 1
    Set-Variable -Name $ResponseHeadersVariable -Value @{} -Scope 1
    return [pscustomobject]@{ id = 'ok' }
}

function Reset-RequestMock([string]$Scenario) {
    $script:scenario = $Scenario
    $script:requestCount = 0
    $script:sleepSeconds = @()
    $script:requestContentTypes = @()
    $script:requestHadBodies = @()
    $script:requestMethods = @()
}

Reset-RequestMock 'retry-get'
$result = Invoke-DevCenterJson GET '/retry' 'token' 'tenant'
if ($result.id -ne 'ok' -or
    $script:requestCount -ne 2 -or
    $script:sleepSeconds.Count -ne 1 -or
    $script:sleepSeconds[0] -ne 1) {
    throw 'Retryable GET HTTP status test failed.'
}

Reset-RequestMock 'network-get'
$result = Invoke-DevCenterJson GET '/network' 'token' 'tenant'
if ($result.id -ne 'ok' -or
    $script:requestCount -ne 2 -or
    $script:sleepSeconds.Count -ne 1 -or
    $script:sleepSeconds[0] -ne 2) {
    throw 'Retryable GET transport test failed.'
}

Reset-RequestMock 'post-fail'
$failedAsExpected = $false
try {
    $null = Invoke-DevCenterJson POST '/mutate' 'token' 'tenant'
} catch {
    $expected =
        'Dev Center API POST /mutate failed with HTTP 504: ' +
        'non-JSON gateway response'
    if ($_.Exception.Message -ne $expected) { throw }
    $failedAsExpected = $true
}
if (-not $failedAsExpected -or
    $script:requestCount -ne 1 -or
    $script:sleepSeconds.Count -ne 0 -or
    $script:requestContentTypes.Count -ne 1 -or
    $script:requestContentTypes[0] -ne 'application/json' -or
    $script:requestHadBodies.Count -ne 1 -or
    $script:requestHadBodies[0]) {
    throw 'Non-retryable POST test failed.'
}

if (-not (Test-DevCenterAmbiguousMutationFailure (
    [Exception]::new(
        'Dev Center API POST /submissions ended before an HTTP response.'
    )
)) -or (Test-DevCenterAmbiguousMutationFailure (
    [Exception]::new(
        'Dev Center API POST /submissions failed with HTTP 400: invalid input'
    )
))) {
    throw 'Ambiguous mutation classification test failed.'
}

Reset-RequestMock 'reconcile-create'
$createFailure = $null
try {
    $null = Invoke-DevCenterJson POST '/submissions' 'token' 'tenant'
} catch {
    $createFailure = $_
}
if ($null -eq $createFailure) {
    throw 'Indeterminate create setup did not fail as expected.'
}
$reconcileArguments = @{
    ApplicationPath = '/application'
    ApplicationId   = 'app-id'
    AccessToken      = 'token'
    TenantId         = 'tenant'
    Reason           = $createFailure.Exception.Message
}
$reconciled = Resolve-IndeterminateStoreCreate @reconcileArguments
if ($reconciled.id -ne 'submission-11' -or
    $reconciled.draft.fileUploadUrl -ne 'https://example.invalid/upload' -or
    $script:requestCount -ne 3 -or
    ($script:requestMethods -join ',') -ne 'POST,GET,GET' -or
    $script:sleepSeconds.Count -ne 0) {
    throw 'Indeterminate create reconciliation test failed.'
}

Reset-RequestMock 'reconcile-non-api'
$nonApiCreateFailure = $null
try {
    $null = Invoke-DevCenterJson POST '/submissions' 'token' 'tenant'
} catch {
    $nonApiCreateFailure = $_
}
$reconcileArguments.Reason = $nonApiCreateFailure.Exception.Message
$nonApiDraftFailed = $false
try {
    $null = Resolve-IndeterminateStoreCreate @reconcileArguments
} catch {
    if ($_.Exception.Message -notmatch 'ownership is not safe to infer') {
        throw
    }
    $nonApiDraftFailed = $true
}
if (-not $nonApiDraftFailed -or $script:requestCount -ne 3) {
    throw 'Non-API draft reconciliation guard test failed.'
}

$applicationWithDraft = [pscustomobject]@{
    pendingApplicationSubmission = [pscustomobject]@{ id = 'submission-11' }
}
$resolvedId = Resolve-RequestedStoreSubmissionId $applicationWithDraft 'product' 'submission-11'
if ($resolvedId -ne 'submission-11') {
    throw 'Explicit draft resume test failed.'
}
$mismatchFailed = $false
try {
    $null = Resolve-RequestedStoreSubmissionId $applicationWithDraft 'product' 'submission-12'
} catch {
    if ($_.Exception.Message -ne (
        'Requested Store submission submission-12 does not match pending ' +
        'submission submission-11.'
    )) { throw }
    $mismatchFailed = $true
}
if (-not $mismatchFailed) {
    throw 'Mismatched draft resume test failed.'
}
$unselectedDraftFailed = $false
try {
    $null = Resolve-RequestedStoreSubmissionId $applicationWithDraft 'product' ''
} catch {
    if ($_.Exception.Message -notmatch 'without an exact resume id') { throw }
    $unselectedDraftFailed = $true
}
$missingDraftFailed = $false
try {
    $null = Resolve-RequestedStoreSubmissionId ([pscustomobject]@{}) 'product' 'submission-11'
} catch {
    if ($_.Exception.Message -notmatch 'has no pending submission') { throw }
    $missingDraftFailed = $true
}
if (-not $unselectedDraftFailed -or -not $missingDraftFailed) {
    throw 'Fail-closed draft resume tests failed.'
}

Write-Host '[OK] Store publishing request policy tests passed.' -ForegroundColor Green
