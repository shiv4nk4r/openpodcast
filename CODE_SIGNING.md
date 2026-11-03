# Code signing policy

OpenPodcast is free and open source under the [MIT License](LICENSE).

## Windows (SignPath Foundation)

Release Windows installers built by GitHub Actions may be Authenticode-signed
through [SignPath Foundation](https://signpath.org/) as a free service for
qualifying open-source projects.

- **Who signs:** SignPath Foundation (publisher name on the certificate)
- **What is signed:** CI-built `OpenPodcast_*_x64_en-US.msi` and
  `OpenPodcast_*_x64-setup.exe` from the public repository
- **How:** GitHub Actions uploads unsigned artifacts, then
  [`signpath/github-action-submit-signing-request`](https://github.com/signpath/github-action-submit-signing-request)
  submits them to SignPath for signing on GitHub-hosted runners only
- **Source of truth:** Only binaries produced from this public repository’s
  automated workflows are submitted for signing

Maintainers must use multi-factor authentication on GitHub and SignPath.

Local developer builds and macOS notarization use separate signing material
(see [`signing/README.md`](signing/README.md)) and are not covered by the
SignPath Foundation certificate.

## macOS

macOS release builds can be signed and notarized with Apple Developer ID
Application identity `Developer ID Application: Shivankar Sharma (NH6XX7B9AW)`.
CI macOS artifacts may ship unsigned unless notarized assets are attached
manually.
