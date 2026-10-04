# OwlWhisp 0.1.2 preview

This release fixes Windows packaging and adds application updates.

- Windows x64 and ARM64 executables now contain the OwlWhisp icon at every bundled
  size and product/version metadata. The overlay also uses the application icon,
  and installed shortcuts explicitly reference it.
- The installer displays Unicode RTF licence notices with actual tables and the
  full Apache 2.0 licence, rather than raw Markdown. Installed publisher metadata
  is Fil-Shtopor.
- Application updates are checked in the background at startup and once a day.
  Settings provide manual checks, automatic-check and preview-channel controls.
  A new version appears in the app and tray menu. Packaged Windows builds can
  download their matching x64/ARM64 installer, verify its size and SHA-256, and
  open the update wizard from the app. Models and settings are preserved.
- Trusted Windows code signing is ready to connect to an existing certificate/HSM
  or Azure Artifact Signing account. **This release is still unsigned** because
  no trusted signing identity has been configured. Fil-Shtopor metadata alone
  does not replace certificate validation.

Install 0.1.2 manually once to obtain the new in-app update mechanism. macOS/Linux
receive version notifications and a link to the matching release; direct
in-app installation is currently available on Windows.

The memory idle-unloading controls from 0.1.1 remain available.
