# Signing and notarizing Cordial for macOS

Downloaded macOS builds need a **Developer ID Application** certificate and Apple notarization. Ad-hoc signing or a free Apple Development certificate is not enough for distribution outside the App Store.

The release workflow signs and notarizes both macOS targets automatically once these repository secrets exist. It fails the macOS builds if any secret is missing, rather than publishing another unsigned DMG.

## 1. Apple Developer setup

1. Enroll in the paid [Apple Developer Program](https://developer.apple.com/programs/).
2. As the Account Holder, create a **Developer ID Application** certificate in [Certificates, IDs & Profiles](https://developer.apple.com/account/resources/certificates/list). Create the CSR from Keychain Access on your Mac, then download and install the issued certificate.
3. In Keychain Access, expand the certificate, select its private key, and export a password-protected `.p12`.
4. In [App Store Connect](https://appstoreconnect.apple.com/access/users), open **Users and Access → Integrations → App Store Connect API**, create a key with **Developer** access, and download the `.p8` file. Note the **Key ID** and **Issuer ID**.

## 2. GitHub repository secrets

In `Katsugachi/Cordial`, open **Settings → Secrets and variables → Actions → New repository secret** and add:

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | Base64 of the exported `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Password chosen when exporting the `.p12` |
| `APPLE_API_KEY` | App Store Connect **Key ID** |
| `APPLE_API_ISSUER` | App Store Connect **Issuer ID** |
| `APPLE_API_KEY_P8` | Full text contents of the downloaded `.p8` file |

Create the base64 value on your Mac:

```bash
openssl base64 -A -in ~/Desktop/Cordial-Developer-ID.p12 -out ~/Desktop/certificate-base64.txt
```

Paste the contents of `certificate-base64.txt` into `APPLE_CERTIFICATE`. Do not commit the `.p12`, `.p8`, passwords, or base64 file, and do not paste them into chat.

## 3. Build a new preview

Do not overwrite Preview 10. Add release notes for the next preview, commit them, then push a new version tag, for example:

```bash
git tag v0.1.0-preview-11
git push origin v0.1.0-preview-11
```

The workflow imports the certificate into a temporary keychain, signs the app, submits it to Apple notarization, staples the ticket, verifies the app and DMG, and deletes the temporary credentials. The published DMGs should open without the “damaged” workaround.

## Local signed build on your Mac

With the certificate installed in your login keychain:

```bash
security find-identity -v -p codesigning
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_API_KEY="KEYID"
export APPLE_API_ISSUER="ISSUER-UUID"
export APPLE_API_KEY_PATH="$HOME/Downloads/AuthKey_KEYID.p8"
npm --prefix ui ci
cd crates/cordial-app
../../ui/node_modules/.bin/tauri build --target aarch64-apple-darwin
```

Use `--target x86_64-apple-darwin` for Intel. Verify the result:

```bash
codesign --verify --deep --strict --verbose=2 ../../target/aarch64-apple-darwin/release/bundle/macos/Cordial.app
xcrun stapler validate ../../target/aarch64-apple-darwin/release/bundle/macos/Cordial.app
xcrun stapler validate ../../target/aarch64-apple-darwin/release/bundle/dmg/Cordial_0.1.0_aarch64.dmg
```
