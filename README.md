# OpenPodcast

Local desktop podcast studio (Tauri 2 + React). Riverside-style lobby → recording studio → editor, with per-person local tracks, peer invites (no hosted server), Google Drive auto-upload, and captions.

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

Outputs unsigned `.dmg` / Windows installers under `src-tauri/target/release/bundle/`.

## Google Drive

1. Create a Google Cloud **Desktop** OAuth client
2. Add redirect `http://127.0.0.1:17834/callback`
3. Export before launch: `export OPENPODCAST_GOOGLE_CLIENT_ID=....apps.googleusercontent.com`
4. Settings → Connect Google Drive

Without a client ID, recordings still save locally (and to any folder you choose).

## License

MIT
