# macOS Sparkle Release Pipeline

This document defines the required release flow for Dictator macOS updates with Sparkle.

## What is automated

On every pushed tag `v*` (example: `v0.4.0`), GitHub Actions now:

1. Builds `DictatorMac.app` (Release).
2. Injects Sparkle settings into app Info.plist during build:
   - `SUPublicEDKey`
   - `SUFeedURL`
3. Packages artifacts:
   - `DictatorMac-<version>-macOS.zip` (Sparkle update archive)
   - `DictatorMac-<version>-macOS.dmg` (installer for first install)
4. Generates and signs `appcast.xml` using Sparkle `generate_appcast`.
5. Uploads macOS artifacts to GitHub Release.
6. Publishes `appcast.xml` to `gh-pages` at:
   - `https://<owner>.github.io/dictator/sparkle/appcast.xml`
7. Verifies release assets are present.

Windows release packaging remains in the same tag workflow.

## Required repository secrets

Set these repository secrets before pushing a release tag:

- `SPARKLE_PRIVATE_KEY` — Sparkle EdDSA private key (base64 secret).
- `SPARKLE_PUBLIC_KEY` — matching public key (value used in `SUPublicEDKey`).

If any secret is missing, macOS release job fails.

## One-time key setup (local)

Use Sparkle `generate_keys` and store output keys in GitHub Secrets.

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
