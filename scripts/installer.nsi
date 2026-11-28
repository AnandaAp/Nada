!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.2.1"
!endif

!ifndef EXE_PATH
  !define EXE_PATH "src-tauri\target\release\nada.exe"
!endif

!ifndef ICON_PATH
  !define ICON_PATH "src-tauri\icons\icon.ico"
!endif

!ifndef OUT_FILE
  !define OUT_FILE "release-extra\Nada_${VERSION}_x64-setup.exe"
!endif

Name "Nada"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\Nada"
InstallDirRegKey HKCU "Software\Nada" "InstallDir"
RequestExecutionLevel user
Unicode True

!define MUI_ICON "${ICON_PATH}"
!define MUI_UNICON "${ICON_PATH}"
!define MUI_ABORTWARNING

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\nada.exe"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Nada Desktop Player" SecMain
  ; Close any running Nada instance (old v0.1.x or hidden v0.2.x) before overwriting
  nsExec::ExecToLog 'taskkill /F /IM nada.exe'
  Sleep 400

  ; Remove legacy v0.1.x Start Menu shortcut so Start Menu launches the new install
  Delete "$SMPROGRAMS\Nada.lnk"

  SetOutPath "$INSTDIR"
  File "${EXE_PATH}"
  File "${ICON_PATH}"

  WriteRegStr HKCU "Software\Nada" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\Nada"
  CreateShortcut "$SMPROGRAMS\Nada\Nada.lnk" "$INSTDIR\nada.exe" "" "$INSTDIR\icon.ico" 0
  CreateShortcut "$DESKTOP\Nada.lnk" "$INSTDIR\nada.exe" "" "$INSTDIR\icon.ico" 0

  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nada" "DisplayName" "Nada"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nada" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nada" "Publisher" "Ananda"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nada" "DisplayIcon" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nada" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nada" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nada" "NoRepair" 1

  ; Notify Windows Shell to refresh shortcut & taskbar icon cache immediately
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\nada.exe"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\Nada\Nada.lnk"
  RMDir "$SMPROGRAMS\Nada"
  Delete "$DESKTOP\Nada.lnk"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nada"
  DeleteRegKey HKCU "Software\Nada"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd
