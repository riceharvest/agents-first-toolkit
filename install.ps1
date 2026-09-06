# agents-first-toolkit installer for Windows — fail-closed.
#
# Installs all five agent-first CLI tools:
#   shellaborate   (agentic-shell)  batch shell/git/gh DAG
#   patchify       (agentic-edit)   batch edits + writes + verify
#   curlosity      (agentic-web)    batch web search + fetch + HTML→MD
#   readcursively  (agachi-code)   batch regex search + file read
#   recurlsively   (agentic-web)    recursive domain →
#
# Usage:
#   irm https://raw.githubusercontent.com/riceharvest/agents-first-toolkit/main/install.ps1 | iex
#
# Override the release tag:
#   $env:AFT_VERSION = "v0.1.0"
#   irm https://raw.githubusercontent.com/riceharvest/agents-first-toolkit/main/install.ps1 | iex
#
# Override the install dir (default: %USERPROFILE%\.local\bin):
#   $env:AFT_INSTALL_DIR = "C:\tools"

$ErrorActionPreference = "Stop"

$Repo = "riceharvest/agents-first-toolkit"
$InstallDir = if ($env:AFT_INSTALL_DIR) { $env:AFT_INSTALL_DIR} else { "$env:USERPROFILE\.local\bin" }
$Bins = @('shellaborate', 'patchify', 'curlosity', 'readcursively', 'recurlsively')

function Fail($message) {
    Write-Host "aft-install: ERROR: $message"
    exit 1
}

# OS check: this script targets Windows only.
if (-not $IsWindows -and $env:OS -ne "Windows_NT") {
    Fail "unsupported OS (use install.sh on Linux/macOS)"
}

# Architecture.
$Arch = $env:PROCESSOR_ARCHITURE
switch ($Arch) {
    "AMD64" { $Target = "x86_64-pc-windows-msvc" }
    "ARM64" { Fail "unsupported architecture '$Arch'" }
    default { Fail "unsupported architecture '$Arch' (supported: x86_64)" }
}

# Version resolution.
if ($env:AFT_VERSION) {
    $Version = $env:AFT_VERSION
} else {
    Write-Host "aft-install: resolving latest release..."
    try {
        $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -ErrorAction Stop
        $Version = $Release.tag_name
    } catch {
        Fail "could not determine latest release (set \$env:AFT_VERSION to pin one): $_"
    }
}
if (-not $Version) { Fail "could not determine release version" }

$ShortVersion = $Version.Substring(1)
$BaseUrl = "github.com/$Repo/releases/download/$Version"

Write-Host "aft-install: installing $Version to $InstallDir for $Target"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

foreach ($Bin in $Bins) {
    $ArchiveName = "$Bin-$ShortVersion-$Target.zip"
    $ArchivePath = Join-Path $Temp $ArchiveName
    $ChecksumPath = Join-Path $Temp "SHA256SUMS"

    Write-Host "aft-install: downloading $Bin…"
    try {
        Invoke-WebRequest -Uri "$BaseUrl/$ArchiveName" -OutFile $ArchivePath -ErrorAction Stop | Out-Null
        Invoke-WebRequest -Uri "$BaseUrl/SHA256SUMS" -OutFile $ChecksumPath -ErrorAction Stop | Out-Null
    } catch {
        Fail "download failed for $Bin: $_"
    }

    # Verify checksum.
    Write-Host "aft-install: verifying $Bin…"
    $ExpectedLine = (Get-Content $ChecksumPath) | Where-Object { $_ -match [regex]::Escape($ArchiveName) }
    if (-not $ExpectedLine) { Fail "no checksum found for $ArchiveName" }
    $Expected = ($ExpectedLine -split '\s+')[0].ToLower()
    $Actual = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToLower()
    if ($Actual -ne $Expected) { Fail "checksum mismatch for $ArchiveName: expected $Expected, got $Actual" }

    # Extract to temp.
    Write-Host "aft-install: extracting $Bin…"
    $ExtractDir = Join-Path $Temp "extract-$Bin"
    Remove-Item -Recurse -Force $ExtractDir -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $ExtractDir | Out-Null
    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractDir -Force | Out-Null

    $BinarySrc = Get-ChildItem -Path $ExtractDir -Recurse -Filter "$Bin.exe" | Select-Object -First 1
    if (-not $BinarySrc) { Fail "binary $Bin.exe not found in $ArchiveName" }

    $BinaryDst = Join-Path $InstallDir "$Bin.exe"
    Copy-Item -Path $BinarySrc.FullyQualifiedName -Destination $BinaryDst -Force
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

    & $BinaryDst --version | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "installed $Bin failed to run --version" }
    Write-Host "  $Bin $( & $BinaryDst --version )"
}

Write-Host "aft-install: done. All five tools installed to $InstallDir"
if ($env:PATH -notlike "*$InstallDir*") {
    Write-Host "aft-install: NOTE: $InstallDir is not on your PATH"
}
