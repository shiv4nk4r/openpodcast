# Sign OpenPodcast Windows EXE + MSI with Authenticode (signtool).
# Mirror of scripts/sign-notarize-dmg.sh for macOS.
#
# Usage:
#   . .\scripts\sign-env.ps1
#   .\scripts\sign-windows.ps1
#
# Requires Windows SDK signtool.exe and a code-signing cert in CurrentUser\My
# (or WINDOWS_PFX_PATH + WINDOWS_PFX_PASSWORD).

$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
$Thumb = ($env:WINDOWS_CERTIFICATE_THUMBPRINT -replace '\s', '').ToUpperInvariant()
$Timestamp = if ($env:WINDOWS_TIMESTAMP_URL) { $env:WINDOWS_TIMESTAMP_URL } else { "http://timestamp.digicert.com" }
$Pfx = $env:WINDOWS_PFX_PATH
$PfxPass = $env:WINDOWS_PFX_PASSWORD

function Find-SignTool {
  $where = Get-Command signtool.exe -ErrorAction SilentlyContinue
  if ($where) { return $where.Source }
  $kits = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending |
    Select-Object -First 1
  if ($kits) { return $kits.FullName }
  throw "signtool.exe not found. Install the Windows 10/11 SDK."
}

function Resolve-Artifacts {
  $candidates = @(
    (Join-Path $Root "src-tauri\target\release"),
    (Join-Path $env:LOCALAPPDATA "Temp\cursor-sandbox-cache")
  )
  $files = @()
  $localExe = Join-Path $Root "src-tauri\target\release\openpodcast.exe"
  $localMsi = Join-Path $Root "src-tauri\target\release\bundle\msi\OpenPodcast_0.1.0_x64_en-US.msi"
  $localNsis = Join-Path $Root "src-tauri\target\release\bundle\nsis\OpenPodcast_0.1.0_x64-setup.exe"
  foreach ($p in @($localExe, $localMsi, $localNsis)) {
    if (Test-Path $p) { $files += (Get-Item $p).FullName }
  }
  if ($files.Count -eq 0) {
    $latest = Get-ChildItem -Path (Join-Path $env:LOCALAPPDATA "Temp\cursor-sandbox-cache") `
      -Recurse -Filter "OpenPodcast_0.1.0_x64_en-US.msi" -ErrorAction SilentlyContinue |
      Sort-Object LastWriteTime -Descending |
      Select-Object -First 1
    if ($latest) {
      $bundleRoot = $latest.Directory.Parent.FullName
      $relRoot = $latest.Directory.Parent.Parent.FullName
      foreach ($name in @(
        (Join-Path $relRoot "openpodcast.exe"),
        $latest.FullName,
        (Join-Path $bundleRoot "nsis\OpenPodcast_0.1.0_x64-setup.exe")
      )) {
        if (Test-Path $name) { $files += (Get-Item $name).FullName }
      }
    }
  }
  if ($files.Count -eq 0) {
    throw "No release artifacts found. Run: npx tauri build --bundles msi"
  }
  return $files | Select-Object -Unique
}

if (-not $Thumb -and -not $Pfx) {
  throw "Set WINDOWS_CERTIFICATE_THUMBPRINT or WINDOWS_PFX_PATH (see scripts/sign-env.example.ps1)."
}
if ($Thumb -match 'REPLACE_WITH|A1B1A2B2') {
  throw "Replace the placeholder thumbprint in sign-env.ps1 with your real certificate thumbprint."
}

$SignTool = Find-SignTool
$Artifacts = Resolve-Artifacts

Write-Host "Using signtool: $SignTool"
Write-Host "Timestamp:     $Timestamp"
Write-Host "Signing $($Artifacts.Count) file(s)…"

foreach ($file in $Artifacts) {
  Write-Host "→ $file"
  $args = @(
    "sign",
    "/fd", "sha256",
    "/td", "sha256",
    "/tr", $Timestamp,
    "/d", "OpenPodcast"
  )
  if ($Pfx) {
    if (-not (Test-Path $Pfx)) { throw "PFX not found: $Pfx" }
    $args += @("/f", $Pfx)
    if ($PfxPass) { $args += @("/p", $PfxPass) }
  } else {
    $args += @("/sha1", $Thumb)
  }
  $args += $file
  & $SignTool @args
  if ($LASTEXITCODE -ne 0) { throw "signtool failed for $file (exit $LASTEXITCODE)" }
  & $SignTool verify /pa /v $file
  if ($LASTEXITCODE -ne 0) { throw "signtool verify failed for $file" }
}

Write-Host "Done. Signed:"
$Artifacts | ForEach-Object { Write-Host "  $_" }
