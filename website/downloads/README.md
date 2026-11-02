# Release downloads

Put built installers here so the landing page can serve them.

Expected filenames (linked from `index.html`):

| File | Platform |
| --- | --- |
| `OpenPodcast_0.1.0_aarch64.dmg` | macOS Apple Silicon |
| (optional) `OpenPodcast_0.1.0_x64-setup.exe` | Windows — update the page when ready |

## Workflow

1. Finish a signed + notarized Mac build.
2. Copy the DMG into this folder with the name above (or change the `href` in `index.html`).
3. Deploy the whole `website/` directory (GitHub Pages, Netlify, Cloudflare Pages, S3, etc.).

Large binaries: if Git complains, use [Git LFS](https://git-lfs.com/) or host the DMG on object storage and point the download `href` at that URL.
