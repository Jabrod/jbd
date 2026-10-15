; JBD installer for the jabrod.com download (NSIS 3, per-user, no administrator rights needed).
; Built by packaging/windows/package-jbd.ps1:
;
;   makensis /DVER=1.0.0 /DSTAGE=<folder with jbd.exe> /DICON=<jbd.ico> /DOUT=<installer.exe> jbd.nsi
;
; It installs into %LOCALAPPDATA%\JBD under the uninstall entry "JBD": the same place and entry as
; the first (Tauri) JBD, so installing over it upgrades it in place and Windows lists one JBD.
; The old app's FFmpeg files are removed (JBD 1.0 has its own video engine).
;
; Silent install: JBD-<ver>-setup.exe /S   (the first JBD's updater runs it with its own flags,
; which NSIS ignores).

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user
ManifestDPIAware true

!define PRODUCT "JBD"
!define PUBLISHER "Jabrod"
!define EXE "jbd.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\JBD"

Name "${PRODUCT}"
Caption "${PRODUCT} ${VER} Setup"
BrandingText "Jabrod"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\JBD"
Icon "${ICON}"
UninstallIcon "${ICON}"

VIProductVersion "${VER}.0"
VIAddVersionKey "ProductName" "JBD"
VIAddVersionKey "CompanyName" "Jabrod"
VIAddVersionKey "FileDescription" "JBD - AI video editor by Jabrod (setup)"
VIAddVersionKey "FileVersion" "${VER}"
VIAddVersionKey "ProductVersion" "${VER}"
VIAddVersionKey "LegalCopyright" "Copyright (c) Jabrod"

!include "MUI2.nsh"
!include "FileFunc.nsh"

; The first JBD's updater (Tauri, installMode "passive") runs this installer with /P (passive:
; no questions) and /R (reopen JBD afterwards). Honour both, and ignore its other flags.
Var Relaunch
Function .onInit
  StrCpy $Relaunch "0"
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/P" $1
  IfErrors +2 0
    SetSilent silent
  ClearErrors
  ${GetOptions} $0 "/R" $1
  IfErrors +2 0
    StrCpy $Relaunch "1"
FunctionEnd

Function .onInstSuccess
  StrCmp $Relaunch "1" 0 +2
    Exec '"$INSTDIR\${EXE}"'
FunctionEnd

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_LICENSEPAGE_TEXT_TOP "Please read the JBD terms. Click I Agree to install."
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Open JBD"

!insertmacro MUI_PAGE_LICENSE "${STAGE}\EULA.txt"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "JBD" Main
  SetOutPath "$INSTDIR"

  ; JBD must be closed to replace jbd.exe (the first JBD's updater closes it itself)
  retry:
    ClearErrors
    Delete "$INSTDIR\${EXE}"
    IfErrors 0 +3
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "JBD is open. Close JBD, then click Retry." /SD IDCANCEL IDRETRY retry
      Abort "JBD is still open."

  ; leftovers of the first JBD (Tauri + FFmpeg)
  Delete "$INSTDIR\ffmpeg.exe"
  Delete "$INSTDIR\ffprobe.exe"
  RMDir /r "$INSTDIR\resources"
  RMDir /r "$INSTDIR\third-party"

  File "${STAGE}\${EXE}"
  ; JBD's MCP program (Settings > Connections: Claude Code / Claude Desktop)
  File "${STAGE}\jbd-mcp.exe"
  File "${STAGE}\THIRD-PARTY-NOTICES.txt"
  File "${STAGE}\EULA.txt"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\JBD.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0
  CreateShortcut "$DESKTOP\JBD.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0

  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\App Paths\${EXE}" "" "$INSTDIR\${EXE}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "JBD"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VER}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${EXE}"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://jabrod.com/jbd"
  WriteRegStr HKCU "${UNINST_KEY}" "HelpLink" "mailto:contact@jabrod.com"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" ${SIZE_KB}
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.txt"
  Delete "$INSTDIR\jbd-mcp.exe"
  Delete "$INSTDIR\EULA.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\JBD.lnk"
  Delete "$DESKTOP\JBD.lnk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\App Paths\${EXE}"
  DeleteRegKey HKCU "${UNINST_KEY}"
  ; Your projects and settings (%APPDATA%\JBD, and wherever you saved projects) are kept.
SectionEnd
