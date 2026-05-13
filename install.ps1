# nit installer for Windows.
#
# Usage:
#   irm https://download.nit.tools/install.ps1 | iex
#
# Environment overrides:
#   $env:NIT_VERSION         Tag to install (default: latest). Example: v0.1.0.
#   $env:NIT_INSTALL_DIR     Where to put the binaries (default: $env:USERPROFILE\.nit\bin).
#   $env:NIT_DOWNLOAD_BASE   Override the download host (default: https://download.nit.tools).
#                            Useful for staging buckets or air-gapped mirrors.
#   $env:NIT_NO_MODIFY_PATH  Set to 1 to skip the PATH update.

$ErrorActionPreference = "Stop"

$Version      = if ($env:NIT_VERSION)       { $env:NIT_VERSION }       else { "latest" }
$InstallDir   = if ($env:NIT_INSTALL_DIR)   { $env:NIT_INSTALL_DIR }   else { Join-Path $env:USERPROFILE ".nit\bin" }
$DownloadBase = if ($env:NIT_DOWNLOAD_BASE) { $env:NIT_DOWNLOAD_BASE } else { "https://download.nit.tools" }

function Write-Info($msg)  { Write-Host "info:  $msg" -ForegroundColor Cyan }
function Write-Warn($msg)  { Write-Host "warn:  $msg" -ForegroundColor Yellow }
function Write-Err($msg)   { Write-Host "error: $msg" -ForegroundColor Red; exit 1 }

# Detect architecture.
$arch = $env:PROCESSOR_ARCHITECTURE
switch ($arch) {
  "AMD64" { $archTag = "x86_64" }
  "ARM64" { Write-Err "Windows ARM64 prebuilt binaries are not yet published." }
  default { Write-Err "Unsupported architecture: $arch" }
}
$target = "$archTag-pc-windows-msvc"

# Resolve tag: `latest.json` is published to the bucket by release.yml after
# every non-prerelease ship. Shape: {"tag":"v0.1.0",...}. No GitHub API auth
# required — works whether the source repo is public or private.
if ($Version -eq "latest") {
  $manifestUrl = "$DownloadBase/latest.json"
  try {
    $rel = Invoke-RestMethod -Uri $manifestUrl -Headers @{ "User-Agent" = "nit-installer" }
    $tag = $rel.tag
  } catch {
    Write-Err "Failed to fetch $manifestUrl (no latest release published yet?)"
  }
  if (-not $tag) { Write-Err "Could not parse 'tag' from $manifestUrl." }
} else {
  $tag = $Version
}

$asset    = "nit-$tag-$target.zip"
$assetUrl = "$DownloadBase/$tag/$asset"
$sumsUrl  = "$DownloadBase/$tag/SHA256SUMS"

Write-Info "Tag:           $tag"
Write-Info "Target:        $target"
Write-Info "Download base: $DownloadBase"
Write-Info "Install dir:   $InstallDir"

$tmp = New-Item -ItemType Directory -Force -Path (Join-Path $env:TEMP ("nit-install-" + [System.Guid]::NewGuid()))
try {
  $archivePath = Join-Path $tmp.FullName $asset
  Write-Info "Downloading $asset..."
  Invoke-WebRequest -Uri $assetUrl -OutFile $archivePath -UseBasicParsing

  # Checksum verification (best effort).
  $sumsPath = Join-Path $tmp.FullName "SHA256SUMS"
  try {
    Invoke-WebRequest -Uri $sumsUrl -OutFile $sumsPath -UseBasicParsing
    $expected = (Get-Content $sumsPath | Where-Object { $_ -match " +$([regex]::Escape($asset))$" } | Select-Object -First 1) -split "\s+" | Select-Object -First 1
    if ($expected) {
      Write-Info "Verifying checksum..."
      $actual = (Get-FileHash -Algorithm SHA256 -Path $archivePath).Hash.ToLower()
      if ($actual -ne $expected.ToLower()) {
        Write-Err "Checksum mismatch for $asset: expected $expected, got $actual"
      }
    } else {
      Write-Warn "Asset $asset not listed in SHA256SUMS; skipping verification."
    }
  } catch {
    Write-Warn "SHA256SUMS not available for $tag; skipping verification."
  }

  Write-Info "Extracting..."
  $extractRoot = Join-Path $tmp.FullName "extract"
  Expand-Archive -Path $archivePath -DestinationPath $extractRoot -Force
  $payload = Join-Path $extractRoot "nit-$tag-$target"
  if (-not (Test-Path $payload)) { Write-Err "Unexpected archive layout: $payload not found" }

  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  Copy-Item -Force -Path (Join-Path $payload "nit.exe")            -Destination (Join-Path $InstallDir "nit.exe")
  Copy-Item -Force -Path (Join-Path $payload "nit-mcp-server.exe") -Destination (Join-Path $InstallDir "nit-mcp-server.exe")

  Write-Info "Installed:"
  Write-Info "  $InstallDir\nit.exe"
  Write-Info "  $InstallDir\nit-mcp-server.exe"

  # PATH update (User scope).
  if ($env:NIT_NO_MODIFY_PATH -ne "1") {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not ($userPath -split ";" | Where-Object { $_ -eq $InstallDir })) {
      $newPath = if ([string]::IsNullOrEmpty($userPath)) { $InstallDir } else { "$userPath;$InstallDir" }
      [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
      Write-Info "Added $InstallDir to your User PATH (open a new shell to pick it up)."
    }
  }

  Write-Host "`nDone. Run ``nit --version`` (in a new shell) to verify." -ForegroundColor Green
}
finally {
  Remove-Item -Recurse -Force $tmp.FullName -ErrorAction SilentlyContinue
}
