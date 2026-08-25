<#
.SYNOPSIS
    Remove the whycodes binary.

.DESCRIPTION
    Config and session data are left alone unless -Purge is given: a user
    uninstalling to reinstall should not lose their providers and history.
#>

[CmdletBinding()]
param(
    [string] $InstallDir = "$env:LOCALAPPDATA\Programs\whycodes",
    [switch] $Purge
)

$ErrorActionPreference = "Stop"
$removed = $false

foreach ($name in @("whycodes.exe", "whycodes.exe.old")) {
    $path = Join-Path $InstallDir $name
    if (Test-Path $path) {
        Remove-Item $path -Force
        Write-Host "Removed $path"
        $removed = $true
    }
}
if (-not $removed) {
    Write-Host "No binary in $InstallDir"
}

if ($Purge) {
    foreach ($dir in @(
        "$env:APPDATA\whycorporation\whycodes",
        "$env:APPDATA\whycorporation\whycode",
        "$env:LOCALAPPDATA\whycorporation\whycodes",
        "$env:LOCALAPPDATA\whycorporation\whycode"
    )) {
        if (Test-Path $dir) {
            Remove-Item $dir -Recurse -Force
            Write-Host "Removed $dir"
            $removed = $true
        }
    }
} else {
    Write-Host "Config and session data were kept. Pass -Purge to remove them too."
}

if (-not $removed) { Write-Host "Nothing to remove." }
