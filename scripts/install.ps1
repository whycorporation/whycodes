<#
.SYNOPSIS
    Install whycodes from a GitHub release.

.DESCRIPTION
    irm https://why.codes/install.ps1 | iex

    GitHub raw is the same file if why.codes is unreachable:
    irm https://raw.githubusercontent.com/whycorporation/whycodes/main/scripts/install.ps1 | iex

    The downloaded archive is verified against the release's SHA256SUMS before
    anything is written to the install directory. The install directory is
    added to the current user's PATH when it is missing, so `whycodes` works
    in this session and in new terminals without extra setup.
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

function Normalize-PathEntry([string]$p) {
    return $p.Trim().TrimEnd('\', '/').ToLowerInvariant()
}

function PathList-HasDir([string]$pathList, [string]$dir) {
    if ([string]::IsNullOrEmpty($pathList)) { return $false }
    $want = Normalize-PathEntry $dir
    foreach ($part in $pathList.Split(';')) {
        if ((Normalize-PathEntry $part) -eq $want) { return $true }
    }
    return $false
}

function Add-UserPath([string]$dir) {
    $current = [Environment]::GetEnvironmentVariable('PATH', 'User')
    if (PathList-HasDir $current $dir) { return $false }
    if ([string]::IsNullOrEmpty($current)) {
        $new = $dir
    } elseif ($current.EndsWith(';')) {
        $new = "$current$dir"
    } else {
        $new = "$current;$dir"
    }
    [Environment]::SetEnvironmentVariable('PATH', $new, 'User')
    return $true
}

function Add-SessionPath([string]$dir) {
    if (PathList-HasDir $env:PATH $dir) { return }
    $env:PATH = "$dir;$env:PATH"
}

function Broadcast-Environment {
    try {
        if (-not ("WhyCodes.NativeMethods" -as [type])) {
            Add-Type -Namespace WhyCodes -Name NativeMethods -MemberDefinition @"
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
"@
        }
        $result = [UIntPtr]::Zero
        [void][WhyCodes.NativeMethods]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, "Environment", 2, 5000, [ref]$result)
    } catch {
        # New terminals still pick up the User PATH; broadcasting is best-effort.
    }
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

    Add-SessionPath $InstallDir
    if (Add-UserPath $InstallDir) {
        Broadcast-Environment
        Write-Host "Added $InstallDir to your user PATH"
        Write-Host "This terminal can already run 'whycodes'. New terminals pick it up automatically."
    } else {
        Write-Host "$InstallDir is already on your user PATH"
    }
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
