# Release downloads

Prefer hosting installers on **GitHub Releases** (see repo Actions → Release workflow), then point the landing page `href`s at those asset URLs.

Example:

```text
https://github.com/shiv4nk4r/openpodcast/releases/download/v0.1.0/OpenPodcast_0.1.0_aarch64.dmg
https://github.com/shiv4nk4r/openpodcast/releases/download/v0.1.0/OpenPodcast_0.1.0_x64-setup.exe
```

## Optional: ship from this folder

| File | Platform |
| --- | --- |
| `OpenPodcast_0.1.0_aarch64.dmg` | macOS Apple Silicon |
| `OpenPodcast_0.1.0_x64-setup.exe` | Windows x64 |

1. Publish (or download) the GitHub Release assets.
2. Copy into this folder **or** update `index.html` to the GitHub download URLs.
3. Deploy `website/`.

Large binaries: don’t commit DMGs to git — use Releases or object storage.
