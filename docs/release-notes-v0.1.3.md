# OwlWhisp 0.1.3 preview

This release adds an optional quiet startup in the system tray.

- In **Settings > Startup**, enable **Start minimized to tray** and click **Save**.
  The next launch starts with the main window hidden, including launches at login
  when **Launch OwlWhisp when I log in** is enabled.
- The window is created hidden, so it does not flash on screen before moving to
  the tray. Dictation hotkeys and background update checks remain active.
- Open the window from the tray menu. On Windows, launching the app again also
  restores the existing window.
- If a system tray cannot be created, the main window opens normally so the app
  remains accessible.

The option is off by default. Existing settings files keep their current startup
behavior. Windows installers and macOS bundles remain unsigned; macOS and Linux
desktop integration remains in preview.
