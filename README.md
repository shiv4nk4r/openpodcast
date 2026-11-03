# OpenPodcast

Local desktop podcast studio (Tauri 2 + React). Riverside-style lobby → recording studio → editor, with per-person local tracks, peer invites (no hosted server), Google Drive auto-upload, and captions.

**License:** [MIT](LICENSE) · **Code signing:** [CODE_SIGNING.md](CODE_SIGNING.md)

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

1. Make this GitHub repo **public**
2. Publish one release (unsigned is OK)
3. Apply at [SignPath Open Source](https://about.signpath.io/product/open-source)
4. Add secrets/vars listed in [`signing/README.md`](signing/README.md)
5. Paste [`.signpath/artifact-configuration.xml`](.signpath/artifact-configuration.xml) into the SignPath project

Signed Windows builds show publisher **SignPath Foundation**.

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
