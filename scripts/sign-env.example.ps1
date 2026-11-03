# Copy to sign-env.ps1 (gitignored) and fill in, then:
#   . .\scripts\sign-env.ps1
#   .\scripts\sign-windows.ps1
#
# Or build + sign in one step (thumbprint must be set):
#   . .\scripts\sign-env.ps1
#   npx tauri build --bundles msi --config src-tauri/tauri.windows-sign.conf.json

# --- Required: Authenticode certificate thumbprint (SHA1, no spaces) ---
# Import your .pfx first, then copy Thumbprint from certmgr.msc → Personal → Certificates
#   Import-PfxCertificate -FilePath .\signing\windows-codesign.pfx `
#     -CertStoreLocation Cert:\CurrentUser\My `
#     -Password (ConvertTo-SecureString -String $env:WINDOWS_PFX_PASSWORD -Force -AsPlainText)
$env:WINDOWS_CERTIFICATE_THUMBPRINT = "A1B1A2B2A3B3A4B4A5B5A6B6A7B7A8B8A9B9A0B0"

# Optional: path to .pfx if you prefer signing from a file instead of the store
# $env:WINDOWS_PFX_PATH = "$PSScriptRoot\..\signing\windows-codesign.pfx"
# $env:WINDOWS_PFX_PASSWORD = "your-export-password"

# Timestamp server (RFC3161)
$env:WINDOWS_TIMESTAMP_URL = "http://timestamp.digicert.com"
