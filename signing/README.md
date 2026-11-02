# macOS signing materials

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
