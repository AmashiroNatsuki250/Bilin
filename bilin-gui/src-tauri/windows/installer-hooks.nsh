!define BILIN_INSTALLER_SCRIPT "${__FILEDIR__}\installer-processes.ps1"

; Tauri uninstalls the old version on its reinstall page, before PREINSTALL.
; MUI consumes this callback on the first page (Welcome). Opening the installer
; alone does not stop anything; the user must first leave the welcome page.
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE BilinPrepareInstall
!define MUI_CUSTOMFUNCTION_GUIINIT BilinInstallerGuiInit

Var BilinHelper
Var BilinInstallerPid
Var BilinDone
Var BilinFailed

!macro BilinPrepare RESTORE
  ${If} $BilinHelper != ""
    Return
  ${EndIf}
  InitPluginsDir
  File "/oname=$PLUGINSDIR\installer-processes.ps1" "${BILIN_INSTALLER_SCRIPT}"
  System::Call 'kernel32::GetCurrentProcessId() i .r0'
  StrCpy $BilinInstallerPid $0
  nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\installer-processes.ps1" -InstallDir "$INSTDIR" -InstallerPid $BilinInstallerPid -RestoreService ${RESTORE}'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Could not close Bilin:$\r$\n$1" /SD IDOK
    Abort
  ${EndIf}
  IntOp $1 $1 + 0
  ${If} $1 != 0
    System::Call 'kernel32::OpenEventW(i 0x100000, i 0, w "Local\BilinInstaller-$BilinInstallerPid-done") p .r0'
    StrCpy $BilinDone $0
    System::Call 'kernel32::OpenEventW(i 0x100000, i 0, w "Local\BilinInstaller-$BilinInstallerPid-failed") p .r0'
    StrCpy $BilinFailed $0
    ${If} $BilinDone == 0
    ${OrIf} $BilinFailed == 0
      MessageBox MB_OK|MB_ICONSTOP "Could not wait for Bilin to finish updating." /SD IDOK
      Abort
    ${EndIf}
    StrCpy $BilinHelper 1
  ${EndIf}
!macroend

!macro BilinFinish
  ${If} $BilinHelper != ""
    System::Call 'kernel32::OpenEventW(i 2, i 0, w "Local\BilinInstaller-$BilinInstallerPid-finish") p .r0'
    ${If} $0 != 0
      System::Call 'kernel32::SetEvent(p r0)'
      System::Call 'kernel32::CloseHandle(p r0)'
    ${EndIf}
    System::Call 'kernel32::WaitForSingleObject(p $BilinDone, i 65000) i .r0'
    System::Call 'kernel32::WaitForSingleObject(p $BilinFailed, i 0) i .r1'
    System::Call 'kernel32::CloseHandle(p $BilinDone)'
    System::Call 'kernel32::CloseHandle(p $BilinFailed)'
    StrCpy $BilinHelper ""
    ${If} $0 != 0
    ${OrIf} $1 != 258
      MessageBox MB_OK|MB_ICONSTOP "Could not restore the Bilin service. Please check its status in Windows Services." /SD IDOK
      Abort
    ${EndIf}
  ${EndIf}
!macroend

Function BilinPrepareInstall
  !insertmacro BilinPrepare 1
FunctionEnd

Function BilinInstallerGuiInit
  ; Passive installs skip Welcome, but still run the old uninstaller page.
  ${GetOptions} $CMDLINE "/P" $0
  ${IfNot} ${Errors}
    Call BilinPrepareInstall
  ${EndIf}
FunctionEnd

Function un.BilinPrepareUninstall
  !insertmacro BilinPrepare 0
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  ; Silent installs skip all pages. Also handle a fresh/custom install path.
  Call BilinPrepareInstall
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro BilinFinish
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Call un.BilinPrepareUninstall
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro BilinFinish
!macroend
