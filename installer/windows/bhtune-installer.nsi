; BHTune Windows installer source.
;
; Build this file with NSIS 3.12 and define the release contract explicitly:
;   makensis /DPRODUCT_VERSION=0.1.0 /DRELEASE_TAG=v0.1.0 \
;     /DPAYLOAD_DIR=path\to\staged-payload \
;     /DGATEWAY_PAYLOAD_DIR=path\to\verified-gateway-payload \
;     /DOUTPUT_DIR=path\to\output bhtune-installer.nsi
;
; The executable payload is intentionally kept out of this source tree.  The
; reusable release workflow supplies a staged BHTune payload and a separately
; verified upstream OPC DA gateway payload.

Unicode True
RequestExecutionLevel admin

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "nsDialogs.nsh"
!include "WinMessages.nsh"

!ifndef PRODUCT_VERSION
  !error "PRODUCT_VERSION=X.Y.Z is required."
!endif
!ifndef RELEASE_TAG
  !error "RELEASE_TAG=vX.Y.Z is required."
!endif
!ifndef PAYLOAD_DIR
  !error "PAYLOAD_DIR must point at the staged release payload."
!endif
!ifndef GATEWAY_PAYLOAD_DIR
  !error "GATEWAY_PAYLOAD_DIR must point at the verified OPC DA gateway payload."
!endif
!ifndef OUTPUT_DIR
  !define OUTPUT_DIR "."
!endif

!define PRODUCT_NAME "BHTune"
!define PRODUCT_PUBLISHER "ByteHound Corp."
!define PRODUCT_URL "https://github.com/bytehound-labs/bhtune"
!define BHTUNE_VERSION "${PRODUCT_VERSION}"
!define BHTUNE_RELEASE_TAG "${RELEASE_TAG}"
!define REQUIRED_NSIS "3.12"

Name "${PRODUCT_NAME} ${BHTUNE_VERSION}"
OutFile "${OUTPUT_DIR}\bhtune-v${BHTUNE_VERSION}-windows-x86_64-installer.exe"
InstallDir "$PROGRAMFILES64\ByteHound\bhtune"
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${BHTUNE_VERSION}.0"
VIAddVersionKey "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey "ProductVersion" "${BHTUNE_VERSION}"
VIAddVersionKey "FileVersion" "${BHTUNE_VERSION}.0"
VIAddVersionKey "CompanyName" "${PRODUCT_PUBLISHER}"
VIAddVersionKey "FileDescription" "${PRODUCT_NAME} Windows installer"
VIAddVersionKey "LegalCopyright" "Copyright © ${PRODUCT_PUBLISHER}"
VIAddVersionKey "OriginalFilename" "bhtune-v${BHTUNE_VERSION}-windows-x86_64-installer.exe"

!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Install BHTune ${BHTUNE_VERSION}"
!define MUI_WELCOMEPAGE_TEXT "This installer registers the BHTune web server as the LocalService Windows service BhtuneServer. It can also install the optional LocalSystem OpcdaBridgeGateway service. Existing ProgramData configuration, databases, indexes, logs, and rollback data are preserved."

Var PowerShellExe
Var AddToPath
Var StartService
Var InstallGateway
Var StartGateway
Var GatewayWasManaged
Var ExistingInstallerMarker
Var CustomDbBackupConfirmed
Var OptionValue
Var AddToPathCheckbox
Var StartServiceCheckbox
Var InstallGatewayCheckbox
Var StartGatewayCheckbox
Var CustomDbBackupCheckbox
Var ProgramDataRootPath

Function InstallerFatal
  Exch $0
  IfSilent silentFatal
  MessageBox MB_ICONSTOP|MB_OK "$0"
  Abort
silentFatal:
  SetErrorLevel 1
  Quit
FunctionEnd

Function un.InstallerFatal
  Exch $0
  IfSilent unSilentFatal
  MessageBox MB_ICONSTOP|MB_OK "$0"
  Abort
unSilentFatal:
  SetErrorLevel 1
  Quit
FunctionEnd

Function GetPowerShellPath
  ; Use the native 64-bit Windows PowerShell host when this installer is
  ; running under WOW64 so Program Files and HKLM use the intended view.
  StrCpy $PowerShellExe "$WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe"
  IfFileExists "$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" useSysnative useSystem32
useSysnative:
    StrCpy $PowerShellExe "$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe"
  Goto powerShellPathDone
useSystem32:
powerShellPathDone:
FunctionEnd

Function ResolveProgramDataRoot
  ReadEnvStr $0 "ProgramData"
  ${If} $0 == ""
    Push "The ProgramData environment variable is unavailable. BHTune cannot determine its fixed data directory."
    Call InstallerFatal
  ${EndIf}
  StrCpy $ProgramDataRootPath "$0\ByteHound\bhtune"
FunctionEnd

Function ParseInstallerOptions
  StrCpy $AddToPath "1"
  StrCpy $StartService "1"
  StrCpy $CustomDbBackupConfirmed "0"

  ClearErrors
  ${GetOptions} $CMDLINE "/ADD_TO_PATH=" $OptionValue
  ${IfNot} ${Errors}
    StrCmp $OptionValue "1" addToPathTrue 0
    StrCmp $OptionValue "0" addToPathFalse addToPathInvalid
addToPathTrue:
    StrCpy $AddToPath "1"
    Goto addToPathDone
addToPathFalse:
    StrCpy $AddToPath "0"
    Goto addToPathDone
addToPathInvalid:
    Push "Invalid /ADD_TO_PATH value. Use 0 or 1."
    Call InstallerFatal
addToPathDone:
  ${EndIf}

  ClearErrors
  ${GetOptions} $CMDLINE "/START_SERVICE=" $OptionValue
  ${IfNot} ${Errors}
    StrCmp $OptionValue "1" startServiceTrue 0
    StrCmp $OptionValue "0" startServiceFalse startServiceInvalid
startServiceTrue:
    StrCpy $StartService "1"
    Goto startServiceDone
startServiceFalse:
    StrCpy $StartService "0"
    Goto startServiceDone
startServiceInvalid:
    Push "Invalid /START_SERVICE value. Use 0 or 1."
    Call InstallerFatal
startServiceDone:
  ${EndIf}

  ClearErrors
  ${GetOptions} $CMDLINE "/INSTALL_GATEWAY=" $OptionValue
  ${IfNot} ${Errors}
    StrCmp $OptionValue "1" installGatewayTrue 0
    StrCmp $OptionValue "0" installGatewayFalse installGatewayInvalid
installGatewayTrue:
    StrCpy $InstallGateway "1"
    Goto installGatewayDone
installGatewayFalse:
    StrCpy $InstallGateway "0"
    Goto installGatewayDone
installGatewayInvalid:
    Push "Invalid /INSTALL_GATEWAY value. Use 0 or 1."
    Call InstallerFatal
installGatewayDone:
  ${EndIf}

  ClearErrors
  ${GetOptions} $CMDLINE "/START_GATEWAY=" $OptionValue
  ${IfNot} ${Errors}
    StrCmp $OptionValue "1" startGatewayTrue 0
    StrCmp $OptionValue "0" startGatewayFalse startGatewayInvalid
startGatewayTrue:
    StrCpy $StartGateway "1"
    Goto startGatewayDone
startGatewayFalse:
    StrCpy $StartGateway "0"
    Goto startGatewayDone
startGatewayInvalid:
    Push "Invalid /START_GATEWAY value. Use 0 or 1."
    Call InstallerFatal
startGatewayDone:
  ${EndIf}

  ClearErrors
  ${GetOptions} $CMDLINE "/CUSTOM_DB_BACKUP_CONFIRMED=" $OptionValue
  ${IfNot} ${Errors}
    StrCmp $OptionValue "1" customDbTrue 0
    StrCmp $OptionValue "0" customDbFalse customDbInvalid
customDbTrue:
    StrCpy $CustomDbBackupConfirmed "1"
    Goto customDbDone
customDbFalse:
    StrCpy $CustomDbBackupConfirmed "0"
    Goto customDbDone
customDbInvalid:
    Push "Invalid /CUSTOM_DB_BACKUP_CONFIRMED value. Use 0 or 1."
    Call InstallerFatal
customDbDone:
  ${EndIf}

  ; An already installer-managed gateway remains part of an upgrade.  The
  ; component switch opts a gateway-free installation in; it never removes
  ; or silently abandons an owned service.
  ${If} $GatewayWasManaged == "1"
    StrCpy $InstallGateway "1"
  ${EndIf}
FunctionEnd

Function InitializeGatewayDefaults
  StrCpy $ExistingInstallerMarker "0"
  StrCpy $GatewayWasManaged "0"
  StrCpy $InstallGateway "1"
  StrCpy $StartGateway "1"

  ClearErrors
  ReadRegDWORD $0 HKLM "Software\ByteHound\bhtune" "InstallerOwned"
  ${IfNot} ${Errors}
    StrCpy $ExistingInstallerMarker "1"
    ; Existing schema-v2 and gateway-free schema-v3 installations preserve
    ; gateway absence unless the operator opts in explicitly.
    StrCpy $InstallGateway "0"
    ClearErrors
    ReadRegDWORD $1 HKLM "Software\ByteHound\bhtune" "GatewayManaged"
    ${IfNot} ${Errors}
      ${If} $1 == 1
        StrCpy $GatewayWasManaged "1"
        StrCpy $InstallGateway "1"
      ${EndIf}
    ${EndIf}
  ${EndIf}
FunctionEnd

Function ApplySilentGatewayDefault
  IfSilent silentGatewayDefault applySilentGatewayDefaultDone
silentGatewayDefault:
  ; Interactive clean installs display the exposure warning and select the
  ; gateway by default. Silent clean installs cannot display that warning, so
  ; they require an explicit /INSTALL_GATEWAY=1 acknowledgement.
  ${If} $ExistingInstallerMarker == "0"
    StrCpy $InstallGateway "0"
  ${EndIf}
applySilentGatewayDefaultDone:
FunctionEnd

Function .onInit
  SetRegView 64
  Call GetPowerShellPath
  Call ResolveProgramDataRoot
  Call InitializeGatewayDefaults
  Call ApplySilentGatewayDefault
  Call ParseInstallerOptions
FunctionEnd

Function UpdateGatewayStartControl
  ${If} $GatewayWasManaged == "1"
    EnableWindow $StartGatewayCheckbox 0
    Return
  ${EndIf}
  ${NSD_GetState} $InstallGatewayCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    EnableWindow $StartGatewayCheckbox 1
  ${Else}
    EnableWindow $StartGatewayCheckbox 0
  ${EndIf}
FunctionEnd

Function OptionsPageCreate
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}

  ${NSD_CreateLabel} 0 0 100% 22u "Choose the optional machine-wide integration settings. BHTune always installs under the fixed ByteHound Program Files and ProgramData locations."
  Pop $0

  ${NSD_CreateCheckbox} 0 28u 100% 14u "Add the BHTune install directory to the machine PATH"
  Pop $AddToPathCheckbox
  ${If} $AddToPath == "1"
    ${NSD_SetState} $AddToPathCheckbox ${BST_CHECKED}
  ${EndIf}

  ${NSD_CreateCheckbox} 0 49u 100% 14u "Start BhtuneServer after installation"
  Pop $StartServiceCheckbox
  ${If} $StartService == "1"
    ${NSD_SetState} $StartServiceCheckbox ${BST_CHECKED}
  ${EndIf}

  ${NSD_CreateCheckbox} 0 70u 100% 14u "Install the local OPC DA gateway (OpcdaBridgeGateway)"
  Pop $InstallGatewayCheckbox
  ${If} $InstallGateway == "1"
    ${NSD_SetState} $InstallGatewayCheckbox ${BST_CHECKED}
  ${EndIf}
  ${NSD_OnClick} $InstallGatewayCheckbox UpdateGatewayStartControl
  ${If} $GatewayWasManaged == "1"
    EnableWindow $InstallGatewayCheckbox 0
  ${EndIf}

  ${NSD_CreateCheckbox} 12u 91u 88% 14u "Start OpcdaBridgeGateway after validation"
  Pop $StartGatewayCheckbox
  ${If} $StartGateway == "1"
    ${NSD_SetState} $StartGatewayCheckbox ${BST_CHECKED}
  ${EndIf}
  ${If} $GatewayWasManaged == "1"
    ${NSD_SetText} $StartGatewayCheckbox "Preserve the existing gateway running/stopped state during upgrade"
    ${NSD_SetState} $StartGatewayCheckbox ${BST_UNCHECKED}
  ${EndIf}
  Call UpdateGatewayStartControl

  ${NSD_CreateLabel} 0 113u 100% 35u "Security warning: the gateway is unauthenticated and listens on all interfaces at TCP port 7600. The installer does not create a Windows Firewall rule. Expose this port only on a trusted OT network."
  Pop $0

  ${NSD_CreateCheckbox} 0 153u 100% 28u "I independently backed up an external/custom database and authorize an upgrade without automatic database rollback"
  Pop $CustomDbBackupCheckbox
  ${If} $CustomDbBackupConfirmed == "1"
    ${NSD_SetState} $CustomDbBackupCheckbox ${BST_CHECKED}
  ${EndIf}

  nsDialogs::Show
FunctionEnd

Function OptionsPageLeave
  ${NSD_GetState} $AddToPathCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $AddToPath "1"
  ${Else}
    StrCpy $AddToPath "0"
  ${EndIf}

  ${NSD_GetState} $StartServiceCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $StartService "1"
  ${Else}
    StrCpy $StartService "0"
  ${EndIf}

  ${NSD_GetState} $InstallGatewayCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $InstallGateway "1"
  ${Else}
    StrCpy $InstallGateway "0"
  ${EndIf}
  ${If} $GatewayWasManaged == "1"
    StrCpy $InstallGateway "1"
  ${EndIf}

  ${NSD_GetState} $StartGatewayCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $StartGateway "1"
  ${Else}
    StrCpy $StartGateway "0"
  ${EndIf}

  ${NSD_GetState} $CustomDbBackupCheckbox $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $CustomDbBackupConfirmed "1"
  ${Else}
    StrCpy $CustomDbBackupConfirmed "0"
  ${EndIf}
FunctionEnd

!insertmacro MUI_PAGE_WELCOME
Page custom OptionsPageCreate OptionsPageLeave
; The options page is intentionally before the file-copy page.  The custom
; page is skipped automatically for /S installs, while .onInit still applies
; the explicit command-line defaults.
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_LANGUAGE "English"

UninstPage uninstConfirm
UninstPage instfiles

Section "Install"
  SetRegView 64
  InitPluginsDir
  SetOutPath "$PLUGINSDIR\payload"
  File /oname=bhtune.exe "${PAYLOAD_DIR}\bhtune.exe"
  File /oname=bhtune-server.exe "${PAYLOAD_DIR}\bhtune-server.exe"
  File /oname=LICENSE "${PAYLOAD_DIR}\LICENSE"
  File /oname=README.md "${PAYLOAD_DIR}\README.md"

  SetOutPath "$PLUGINSDIR\gateway-payload"
  File /oname=opcda-bridge-gateway.exe "${GATEWAY_PAYLOAD_DIR}\opcda-bridge-gateway.exe"
  File /oname=opcda-gateway-release.json "${GATEWAY_PAYLOAD_DIR}\opcda-gateway-release.json"
  File /oname=opcda-gateway-provenance.json "${GATEWAY_PAYLOAD_DIR}\opcda-gateway-provenance.json"
  File /oname=LICENSE-opcda-bridge.txt "${GATEWAY_PAYLOAD_DIR}\LICENSE-opcda-bridge.txt"
  File /oname=NOTICE-opcda-bridge.txt "${GATEWAY_PAYLOAD_DIR}\NOTICE-opcda-bridge.txt"

  SetOutPath "$PLUGINSDIR\installer"
  File /oname=InstallerSupport.ps1 "${__FILEDIR__}\InstallerSupport.ps1"
  File /oname=Install-Bhtune.ps1 "${__FILEDIR__}\Install-Bhtune.ps1"

  ; Generate the final uninstaller into the staging directory.  The
  ; PowerShell orchestrator copies it into Program Files only after all
  ; ownership and payload checks have passed.
  WriteUninstaller "$PLUGINSDIR\uninstall.exe"

  DetailPrint "Validating BHTune ${BHTUNE_VERSION} (${BHTUNE_RELEASE_TAG}) payload..."
  ExecWait '"$PowerShellExe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\installer\Install-Bhtune.ps1" -Mode Install -ExpectedVersion "${BHTUNE_VERSION}" -ReleaseTag "${BHTUNE_RELEASE_TAG}" -PayloadRoot "$PLUGINSDIR\payload" -GatewayPayloadRoot "$PLUGINSDIR\gateway-payload" -InstallerScriptRoot "$PLUGINSDIR\installer" -UninstallerSource "$PLUGINSDIR\uninstall.exe" -InstallRoot "$INSTDIR" -ProgramDataRoot "$ProgramDataRootPath" -AddToPath $AddToPath -StartService $StartService -InstallGateway $InstallGateway -StartGateway $StartGateway -CustomDbBackupConfirmed $CustomDbBackupConfirmed -TracePath "$ProgramDataRootPath\installer\install-trace.jsonl"' $0
  ${If} $0 != 0
    Push "BHTune installation failed. No unowned service or partial installation was left behind. Review the installer details for the exact error."
    Call InstallerFatal
  ${EndIf}
SectionEnd

Function un.onInit
  SetRegView 64
  Call un.GetPowerShellPath
  Call un.ResolveProgramDataRoot
FunctionEnd

Function un.GetPowerShellPath
  StrCpy $PowerShellExe "$WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe"
  IfFileExists "$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" un.useSysnative un.useSystem32
un.useSysnative:
  StrCpy $PowerShellExe "$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe"
  Goto un.powerShellPathDone
un.useSystem32:
un.powerShellPathDone:
FunctionEnd

Function un.ResolveProgramDataRoot
  ReadEnvStr $0 "ProgramData"
  ${If} $0 == ""
    Push "The ProgramData environment variable is unavailable. BHTune cannot determine its fixed data directory."
    Call un.InstallerFatal
  ${EndIf}
  StrCpy $ProgramDataRootPath "$0\ByteHound\bhtune"
FunctionEnd

Section "Uninstall"
  SetRegView 64
  ; Extract a fresh copy of the ownership-aware orchestrator into the
  ; uninstaller's private temporary directory.  Do not depend on the copy
  ; under $INSTDIR: a locked or unexpected residual file may prevent NSIS
  ; from deleting that tree, and a retry must still be able to run the
  ; ownership checks.
  InitPluginsDir
  SetOutPath "$PLUGINSDIR\installer"
  File /oname=InstallerSupport.ps1 "${__FILEDIR__}\InstallerSupport.ps1"
  File /oname=Install-Bhtune.ps1 "${__FILEDIR__}\Install-Bhtune.ps1"
  DetailPrint "Verifying installer ownership before uninstall..."
  ExecWait '"$PowerShellExe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\installer\Install-Bhtune.ps1" -Mode Uninstall -InstallRoot "$INSTDIR" -ProgramDataRoot "$ProgramDataRootPath" -LeaveInstallRoot -TracePath "$ProgramDataRootPath\installer\uninstall-trace.jsonl"' $0
  ${If} $0 != 0
    Push "BHTune uninstall refused or failed. ProgramData was preserved."
    Call un.InstallerFatal
  ${EndIf}
  ; Use the fixed root rather than a registry-supplied $INSTDIR value.  The
  ; PowerShell ownership check already validated this exact location, and
  ; keeping the deletion fixed prevents conflicting metadata from redirecting
  ; cleanup into an unrelated directory.
  ; NSIS keeps the running uninstaller open until this section exits.  The
  ; Delete instruction has special self-removal handling; without it, the
  ; recursive removal below can leave the entire install root behind on
  ; Windows versions that do not allow an executing image to be removed.
  Delete "$PROGRAMFILES64\ByteHound\bhtune\uninstall.exe"
  RMDir /r "$PROGRAMFILES64\ByteHound\bhtune"
  ; The PowerShell side deliberately retains ownership metadata until the
  ; fixed Program Files tree has been removed.  The PowerShell pass has
  ; already started a guarded finalizer which waits for this NSIS process to
  ; exit before removing ownership metadata and the journal.  This boundary
  ; is required on older Windows versions where the running uninstaller
  ; cannot be removed synchronously from inside its own process.
  IfFileExists "$PROGRAMFILES64\ByteHound\bhtune\." un_cleanup_deferred un_cleanup_done
un_cleanup_deferred:
  DetailPrint "Program Files cleanup will finish after the uninstaller exits; installer ownership metadata remains protected until then."
  Goto un_cleanup_done
un_cleanup_done:
  DetailPrint "BHTune uninstall finalization is running in the background. ProgramData and operator data are preserved."
SectionEnd
