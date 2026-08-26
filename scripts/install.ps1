<#
.SYNOPSIS
    Install whycodes from a GitHub release.

.DESCRIPTION
    irm https://raw.githubusercontent.com/whycorporation/whycodes/main/scripts/install.ps1 | iex

    The downloaded archive is verified against the release's SHA256SUMS before
    anything is written to the install directory. PATH is not modified: an
    installer editing a user's PATH is intrusive and easy to get wrong, so the
    directory is printed instead.
#>

[CmdletBinding()]
param(
    [string] $InstallDir = "$env:LOCALAPPDATA\Programs\whycodes",
    [string] $Version = "latest"
)

$ErrorActionPreference = "Stop"
$Repo = "whycorporation/whycodes"

function Fail($message) {
    Write-Error $message
    exit 1
}

# Only x86_64 Windows is published; arm64 would need its own release target.
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne "AMD64") {
    Fail "unsupported architecture '$arch' — build from source with 'cargo build --release'"
}

$target  = "x86_64-pc-windows-msvc"
$archive = "whycodes-$target.zip"
$base = if ($Version -eq "latest") {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$Version"
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("whycodes-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
    Write-Host "Downloading $archive"
    $archivePath = Join-Path $tmp $archive
    $sumsPath = Join-Path $tmp "SHA256SUMS"

    try {
        Invoke-WebRequest -Uri "$base/$archive" -OutFile $archivePath -UseBasicParsing
    } catch {
        Fail "could not download $base/$archive : $_"
    }
    try {
        Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sumsPath -UseBasicParsing
    } catch {
        Fail "could not download the checksum file; refusing to install unverified"
    }

    $line = Get-Content $sumsPath | Where-Object { $_ -match "\s$([regex]::Escape($archive))$" }
    if (-not $line) { Fail "$archive is not listed in SHA256SUMS" }
    $expected = ($line -split '\s+')[0]
    $actual = (Get-FileHash -Path $archivePath -Algorithm SHA256).Hash.ToLower()

    if ($expected.ToLower() -ne $actual) {
        Fail "checksum mismatch for $archive`n  expected $expected`n  actual   $actual`nNothing was installed."
    }
    Write-Host "Checksum verified"

    Expand-Archive -Path $archivePath -DestinationPath (Join-Path $tmp "unpacked") -Force
    $exe = Join-Path $tmp "unpacked\whycodes.exe"
    if (-not (Test-Path $exe)) { Fail "the archive did not contain whycodes.exe" }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $dest = Join-Path $InstallDir "whycodes.exe"

    # Windows locks a running executable, so a copy over it fails. Move the old
    # one aside first — the rename succeeds even while it is running, and the
    # displaced file is cleaned up on the next install.
    if (Test-Path $dest) {
        $old = "$dest.old"
        Remove-Item $old -Force -ErrorAction SilentlyContinue
        try { Move-Item $dest $old -Force } catch {
            Fail "could not replace $dest — close any running whycodes and try again"
        }
    }
    Copy-Item $exe $dest -Force

    Write-Host "Installed to $dest"
    & $dest --version

    $onPath = ($env:PATH -split ';') -contains $InstallDir
    if (-not $onPath) {
        Write-Host ""
        Write-Host "$InstallDir is not on your PATH. Add it with:"
        Write-Host "    [Environment]::SetEnvironmentVariable('PATH', `"`$env:PATH;$InstallDir`", 'User')"
    }
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
