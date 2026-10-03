; OwlWhisp installer.
;
; Replaces the NSIS installer the Tauri bundler used to generate. Deliberately small: install to a
; folder, make a Start menu and desktop shortcut, register an uninstaller, and get out of the way.
;
; Per-user, not per-machine, unlike the Tauri configuration. This application needs no privilege it
; cannot get as the user -- the hotkey is a user-session keyboard hook, autostart is HKCU, and the
; models live under %APPDATA% -- so asking for an administrator prompt would be asking for a right
; it does not use.
;
; Driven by scripts\build\package-windows.ps1, which passes VERSION, STAGE and OUTFILE.

!ifndef VERSION
  !error "VERSION is not defined; run this through package-windows.ps1"
!endif
!ifndef STAGE
  !error "STAGE is not defined; run this through package-windows.ps1"
!endif
!ifndef OUTFILE
  !error "OUTFILE is not defined; run this through package-windows.ps1"
!endif

Unicode true
Name "OwlWhisp ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\OwlWhisp"
InstallDirRegKey HKCU "Software\OwlWhisp" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUnInstDetails show

!include "MUI2.nsh"

!define MUI_ICON "..\..\assets\icons\icon.ico"
!define MUI_UNICON "..\..\assets\icons\icon.ico"
!define MUI_ABORTWARNING

!insertmacro MUI_PAGE_LICENSE "${STAGE}\LICENSES.md"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\OwlWhisp.exe"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\OwlWhisp"

Section "OwlWhisp" SecMain
  SectionIn RO

  ; A running copy holds its executable open, and the single-instance mutex means a fresh launch
  ; would only wake the old one. Ask the user to close it rather than fail halfway through a copy.
  FindWindow $0 "" "OwlWhisp"
  StrCmp $0 0 notRunning
    MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION \
      "OwlWhisp is running. Quit it from the tray, then press OK to continue." \
      IDOK notRunning
    Abort "Installation cancelled: OwlWhisp is still running."
  notRunning:

  SetOutPath "$INSTDIR"
  File /r "${STAGE}\*.*"

  WriteRegStr HKCU "Software\OwlWhisp" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "OwlWhisp"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\OwlWhisp.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "OwlWhisp"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\OwlWhisp"
  CreateShortCut "$SMPROGRAMS\OwlWhisp\OwlWhisp.lnk" "$INSTDIR\OwlWhisp.exe"
  CreateShortCut "$DESKTOP\OwlWhisp.lnk" "$INSTDIR\OwlWhisp.exe"

  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  FindWindow $0 "" "OwlWhisp"
  StrCmp $0 0 notRunning
    MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION \
      "OwlWhisp is running. Quit it from the tray, then press OK to continue." \
      IDOK notRunning
    Abort "Uninstall cancelled: OwlWhisp is still running."
  notRunning:

  Delete "$DESKTOP\OwlWhisp.lnk"
  Delete "$SMPROGRAMS\OwlWhisp\OwlWhisp.lnk"
  RMDir "$SMPROGRAMS\OwlWhisp"

  ; The autostart registration, if the user turned it on. Left behind it would try to launch an
  ; executable that no longer exists, every login, silently.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "OwlWhisp"

  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR\runtime"
  ; These are immutable manifests bundled with the application, not downloaded model data.
  ; Actual model files stay under %APPDATA% and are deliberately preserved below.
  RMDir /r "$INSTDIR\models"
  Delete "$INSTDIR\OwlWhisp.exe"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\LICENSES.md"
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.md"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\OwlWhisp"

  ; Settings, downloaded models and logs live under %APPDATA%\ai.owlwhisp.app and are the
  ; user's, not ours. A model set can be tens of gigabytes and takes an hour to fetch again;
  ; deleting it because somebody uninstalled to reinstall would be indefensible. Say where it is
  ; and leave it.
  MessageBox MB_OK|MB_ICONINFORMATION \
    "OwlWhisp is uninstalled.$\n$\nYour settings, downloaded models and logs were left in:$\n$APPDATA\ai.owlwhisp.app$\n$\nDelete that folder by hand if you want them gone."
SectionEnd
