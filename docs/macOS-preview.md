# Opening the free macOS preview

Cordial's macOS previews are ad-hoc signed, not notarized with a paid Apple Developer ID. macOS may therefore warn that the developer cannot be verified, or show a misleading "damaged" message for a downloaded copy.

Only do this for a DMG downloaded from the official [Cordial releases page](https://github.com/Katsugachi/Cordial/releases), and verify its checksum first.

## Apple Silicon, Preview 10

```bash
shasum -a 256 ~/Downloads/Cordial_0.1.0_aarch64.dmg
```

Expected SHA-256:

```text
7a9dd21dcfbe5082e93f9a023382624668cedcc428d146d7737608b885a150d5
```

For Intel, use `Cordial_0.1.0_x64.dmg` and the checksum listed in `SHA256SUMS.txt`.

## Open it

1. Open the DMG and drag `Cordial.app` into `Applications`.
2. Eject the DMG.
3. In `Applications`, right-click `Cordial.app` and choose **Open**. If macOS still blocks it, open **System Settings → Privacy & Security** and choose **Open Anyway**.
4. If macOS reports the app as "damaged" even though the checksum matched, remove the browser quarantine flag from the installed copy:

```bash
xattr -dr com.apple.quarantine /Applications/Cordial.app
open /Applications/Cordial.app
```

Do not disable Gatekeeper globally. The command above only affects the copy of Cordial you installed.

## Why this happens

Apple requires a paid Developer Program membership for Developer ID signing and notarization. Ad-hoc signing is free and satisfies Apple Silicon's local code-signature requirement, but it does not establish a trusted developer identity for downloaded apps. A future paid release can be notarized so users do not need these steps.
