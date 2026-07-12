#!/usr/bin/env pwsh
#requires -Version 7.2

$ErrorActionPreference = 'Stop'

# Dot-sourcing without -Publish performs local manifest validation only, then
# leaves the request-policy functions available for isolated network mocks.
. (Join-Path $PSScriptRoot 'publish-store.ps1')

$script:scenario = ''
$script:requestCount = 0
$script:sleepSeconds = @()

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
    $script:sleepSeconds.Count -ne 0) {
    throw 'Non-retryable POST test failed.'
}

Write-Host '[OK] Store publishing request policy tests passed.' -ForegroundColor Green
