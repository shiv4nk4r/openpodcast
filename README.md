# OpenPodcast

[![CI](https://github.com/shiv4nk4r/openpodcast/actions/workflows/ci.yml/badge.svg)](https://github.com/shiv4nk4r/openpodcast/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub release](https://img.shields.io/github/v/release/shiv4nk4r/openpodcast)](https://github.com/shiv4nk4r/openpodcast/releases)

Local desktop podcast studio (Tauri 2 + React). Riverside-style lobby → recording studio → editor, with per-person local tracks, peer invites (no hosted server), Google Drive auto-upload, and captions.

**License:** [MIT](LICENSE) · **Contributing:** [CONTRIBUTING.md](CONTRIBUTING.md) · **Code signing:** [CODE_SIGNING.md](CODE_SIGNING.md) · **Security:** [SECURITY.md](SECURITY.md)

## Features

- **Host + guests**: create a studio, copy an invite ticket, guests paste it in the same app
- **Modes**: audio-only or audio + video; frame rate 24/25/30/60; 720p/1080p
- **Local HQ recording**: WAV via Rust (`cpal`) + browser MediaRecorder video tracks saved to disk
- **Google Drive**: desktop OAuth (`drive.file`) + resumable upload (set `OPENPODCAST_GOOGLE_CLIENT_ID`)
- **Captions**: Whisper CLI / optional cloud provider / offline energy fallback → SRT
- **Editor**: Riverside-like chrome — timeline, captions, tracks, Share/export

## Develop

```bash
npm install
npm run tauri dev
```

## Build installers

```bash
npm run tauri build
```

Outputs `.dmg` / Windows installers under `src-tauri/target/release/bundle/`.

### GitHub Releases (macOS + Windows)

Push a version tag (or run the **Release** workflow manually). CI builds both platforms and attaches installers to a **draft** release:

```bash
# bump version in package.json + src-tauri/tauri.conf.json first
git tag v0.1.0
git push origin v0.1.0
```

Then open the draft at `https://github.com/shiv4nk4r/openpodcast/releases`, check the assets, and click **Publish**.

| Asset | Platform |
| --- | --- |
| `OpenPodcast_*_aarch64.dmg` | macOS Apple Silicon (CI = unsigned unless you replace with notarized DMG) |
| `OpenPodcast_*_x64-setup.exe` / `*_en-US.msi` | Windows x64 (SignPath-signed when configured) |

### Windows signing (free via SignPath)

This repo is **public MIT**, which qualifies for [SignPath Open Source](https://about.signpath.io/product/open-source) signing:

1. Apply at SignPath and install the GitHub App on this repo
2. Paste [`.signpath/artifact-configuration.xml`](.signpath/artifact-configuration.xml) into the SignPath project
3. Add secrets/vars listed in [`signing/README.md`](signing/README.md) — including `SIGNPATH_ENABLED=true`
4. Cut the next `v*` tag; CI attaches **macOS + Windows**, with Windows signed when SignPath is connected

Signed Windows builds show publisher **SignPath Foundation**. Until then, releases still ship both platforms (Windows unsigned).

### macOS notarization (local)

```bash
source scripts/sign-env.sh
npm run tauri build
bash scripts/sign-notarize-dmg.sh
```

Upload/replace that DMG on the same GitHub Release.

## Google Drive

1. Create a Google Cloud **Desktop** OAuth client
2. Add redirect `http://127.0.0.1:17834/callback`
3. Export before launch: `export OPENPODCAST_GOOGLE_CLIENT_ID=....apps.googleusercontent.com`
4. Settings → Connect Google Drive

Without a client ID, recordings still save locally (and to any folder you choose).

## License

[MIT](LICENSE)
