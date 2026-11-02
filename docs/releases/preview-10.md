## What’s new

- **More reliable cancellation and follow-up turns.** Interrupted generation clears reused model state, and streamed output resets correctly when a tool round retries.
- **Better tool-call handling.** Explicit JSON tool calls are supported; unavailable tools are rejected instead of executed.
- **Improved model management.** Startup failures are shown with a retry path, model removal is blocked during generation, and the inference worker shuts down before downloaded files are removed.
- **Windows model cleanup.** The new NSIS uninstaller removes Cordial-managed model downloads on a normal uninstall, but preserves them during updates. Chats and workspace files are preserved.
- **Reusable instructions.** Import a Markdown or text file into the skill-instructions field, up to 4,000 characters. This is not a full skill-folder or bundled-script system.
- **macOS build fix.** Metal builds now link the required Apple compiler runtime.

## Downloads

Choose the installer for your device:

| Platform | Installers |
| --- | --- |
| macOS, Apple Silicon | `Cordial_0.1.0_aarch64.dmg` |
| macOS, Intel | `Cordial_0.1.0_x64.dmg` |
| Windows, x64 | `Cordial_0.1.0_x64_en-US.msi` or `Cordial_0.1.0_x64-setup.exe` |
| Windows 11, ARM64 | `Cordial_0.1.0_arm64_en-US.msi` or `Cordial_0.1.0_arm64-setup.exe` |
| Linux, x64 | `Cordial_0.1.0_amd64.deb` or `Cordial_0.1.0_amd64.AppImage` |
| Linux, ARM64 | `Cordial_0.1.0_arm64.deb` or `Cordial_0.1.0_aarch64.AppImage` |

`SHA256SUMS.txt` contains checksums for all ten installers. On first launch, Cordial downloads the model weights and vision projector (about 1.3 GB combined). Model weights are not included in the installers.

## Before installing

- These are **preview builds**, marked as a prerelease rather than the latest stable release.
- Installers are **unsigned**, and macOS builds are **not notarized**. Your operating system may show a security warning.
- Windows ARM64 uses OpenCL with CPU fallback and still needs validation on target hardware.
- Older installed versions do not include the new NSIS cleanup hook. Use **Remove downloaded model** in Cordial before uninstalling an older version if you also want to remove model downloads. Other installer formats still require manual model removal.

## Verification

Publication requires successful builds for all six platform targets, UI and core tests, and the real-model behavioural smoke suite. The source commit and workflow run are linked below.

[Full changes since Preview 9](https://github.com/Katsugachi/Cordial/compare/v0.1.0-preview-9...v0.1.0-preview-10)
