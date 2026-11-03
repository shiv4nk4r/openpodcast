# Code signing materials

## macOS

- `developerID_application.cer` — Apple **Developer ID Application** public certificate (safe to keep / commit).
- `AuthKey_*.p8` — App Store Connect API private key for notarization (**gitignored**, never commit).
- Code-signing private keys stay in your Mac **login** keychain (or a local `.key` / `.p12` that must **never** be committed).

Identity in use:

```text
Developer ID Application: Shivankar Sharma (NH6XX7B9AW)
```

Re-install the cert into Keychain if needed:

```bash
open signing/developerID_application.cer
# or
security add-certificates signing/developerID_application.cer
```

Build + notarize:

```bash
source scripts/sign-env.sh && bash scripts/sign-notarize-dmg.sh
```

## Windows — free OSS path (SignPath Foundation)

Preferred for this public MIT project. Publisher users see: **SignPath Foundation**.

### 1. Make the GitHub repo public

SignPath’s free OSS tier requires a public repository with an OSI license (MIT ✓).

### 2. Ship at least one release

Push a tag so CI produces installers (unsigned is fine the first time):

```bash
git tag v0.1.0
git push origin v0.1.0
```

### 3. Apply to SignPath

1. Enable MFA on GitHub
2. Create an account at [signpath.io](https://about.signpath.io/product/open-source) / apply for **Open Source Code Signing**
3. Install the **SignPath GitHub App** on `shiv4nk4r/openpodcast`
4. Create a project (slug: `openpodcast`)
5. Add artifact configuration from [`.signpath/artifact-configuration.xml`](../.signpath/artifact-configuration.xml)  
   (slug: `windows-installers`)
6. Create signing policy slug `release-signing` (manual approval is normal for OSS)
7. Create an API token with submitter rights

### 4. Add GitHub Actions secrets / variables

| Name | Type | Example |
| --- | --- | --- |
| `SIGNPATH_API_TOKEN` | **secret** | (from SignPath) |
| `SIGNPATH_ORGANIZATION_ID` | variable | UUID from SignPath org |
| `SIGNPATH_PROJECT_SLUG` | variable | `openpodcast` |
| `SIGNPATH_SIGNING_POLICY_SLUG` | variable | `release-signing` |
| `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` | variable | `windows-installers` |

Policy + workflow notes: [`CODE_SIGNING.md`](../CODE_SIGNING.md).

After that, the **Release** workflow signs Windows MSI/NSIS automatically when those values are present.

## Windows — paid Authenticode (your name on the cert)

If you later buy an OV/EV cert and want “your” publisher name instead of SignPath:

1. Import `.pfx` into `Cert:\CurrentUser\My`
2. Copy `scripts/sign-env.example.ps1` → `scripts/sign-env.ps1`
3. Set thumbprint in `src-tauri/tauri.windows-sign.conf.json`
4. Run:

```powershell
. .\scripts\sign-env.ps1
npx tauri build --bundles msi --config src-tauri/tauri.windows-sign.conf.json
# or
.\scripts\sign-windows.ps1
```

## Microsoft Store

Tauri ships EXE/MSI (not MSIX). Use Partner Center **EXE or MSI app**, silent install (`/S` or `msiexec /quiet`), and optionally `tauri.microsoftstore.conf.json`. See [Tauri Store docs](https://v2.tauri.app/distribute/microsoft-store/).
