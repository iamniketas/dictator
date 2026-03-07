# macOS Sparkle Release Pipeline

This document defines the required release flow for Dictator macOS updates with Sparkle.

## What is automated

On every pushed tag `v*` (example: `v0.4.0`), GitHub Actions now:

1. Builds `Dictator.app` (Release).
2. Injects Sparkle settings into app Info.plist during build:
   - `SUPublicEDKey`
   - `SUFeedURL`
3. Signs app bundle with `Developer ID Application` certificate.
4. Notarizes app bundle and staples ticket.
5. Packages artifacts:
   - `Dictator-<version>-macOS.zip` (Sparkle update archive)
   - `Dictator-<version>-macOS.dmg` (installer for first install, includes `Applications` shortcut)
6. Signs + notarizes DMG and staples ticket.
7. Generates and signs `appcast.xml` using Sparkle `generate_appcast`.
7. Uploads macOS artifacts to GitHub Release.
8. Publishes `appcast.xml` to `gh-pages` at:
   - `https://<owner>.github.io/dictator/sparkle/appcast.xml`
9. Verifies release assets are present.

Windows release packaging remains in the same tag workflow.

## Required repository secrets

Required now:

- `SPARKLE_PRIVATE_KEY` — Sparkle EdDSA private key (base64 secret).
- `SPARKLE_PUBLIC_KEY` — matching public key (value used in `SUPublicEDKey`).

Optional (enable trusted distribution without Gatekeeper warnings):

- `MACOS_DEVELOPER_ID_APP_CERT_P12_BASE64` — base64-encoded `.p12` for `Developer ID Application`.
- `MACOS_DEVELOPER_ID_APP_CERT_PASSWORD` — password for the `.p12`.
- `MACOS_DEVELOPER_ID_APP_SIGNING_IDENTITY` — exact codesign identity string, e.g. `Developer ID Application: Your Name (TEAMID)`.
- `MACOS_NOTARY_KEY_ID` — App Store Connect API key ID.
- `MACOS_NOTARY_ISSUER_ID` — App Store Connect issuer ID.
- `MACOS_NOTARY_API_KEY_P8_BASE64` — base64-encoded contents of `AuthKey_<KEY_ID>.p8`.

If Developer ID/notary secrets are missing, workflow still builds and publishes macOS artifacts for internal testing, but those artifacts are not notarized and macOS will show Gatekeeper warnings.

## One-time key setup (local)

Use Sparkle `generate_keys` and store output keys in GitHub Secrets.

Create Developer ID and notarization credentials:

1. In Apple Developer account, issue `Developer ID Application` certificate.
2. Export it from Keychain as `.p12` (with password), then base64-encode it.
3. In App Store Connect, create API key for notarization (`.p8`, key id, issuer id).
4. Store all values in repository secrets listed above.

- Keep the private key secret out of source control.
- Public key can be shared and embedded in app build settings.

## Release command

```bash
git tag v0.4.0
git push origin v0.4.0
```

Do not create macOS release artifacts manually. The tag workflow is the source of truth.

## How to ensure the process is always followed

1. Only publish releases by pushing tags (`v*`).
2. Keep branch protection on `main` and require green CI before merge.
3. Do not upload macOS release assets manually in GitHub UI.
4. Treat failed `release-macos` or `verify-release-macos-assets` as a blocked release.

## Runtime feed used by the app

The app resolves Sparkle feed in this order:

1. `DICTATOR_SPARKLE_FEED_URL` environment variable (override for tests).
2. `SUFeedURL` in app Info.plist (release default from pipeline).

For production releases, the pipeline sets `SUFeedURL` to GitHub Pages appcast URL.
