# Contributing to OpenPodcast

Thanks for helping. This project is MIT-licensed and built as a local-first podcast studio (Tauri 2 + React).

## Quick start

```bash
npm install
# macOS AEC builds need: brew install meson ninja cmake pkg-config
npm run tauri dev
```

## Workflow

1. Open an issue for larger changes (or comment on an existing one).
2. Fork the repo and create a branch from `main`.
3. Keep PRs focused — one concern per PR when possible.
4. Ensure **CI** is green (`Frontend` + `Rust check`).
5. Prefer clear commit messages that explain *why*.

## Code map

| Area | Path |
| --- | --- |
| UI | `src/` |
| Tauri / P2P / AEC | `src-tauri/src/` |
| Release signing | `CODE_SIGNING.md`, `signing/README.md` |
| Website | `website/` |

## Releases

Maintainers cut releases by tagging `v*` (see README). Windows installers may be signed via SignPath Foundation; macOS notarization is usually done locally with Developer ID.

## Security

Please do **not** open public issues for vulnerabilities. See [SECURITY.md](SECURITY.md).

## License

By contributing, you agree your contributions are licensed under the [MIT License](LICENSE).
