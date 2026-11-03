; Uninstaller additions for the NSIS installer (electron-builder `nsis.include`).
;
; - Asks whether to delete the local data (messages, servers, settings in
;   %APPDATA%\MVS). The default is to keep it. A silent uninstall (/S) keeps
;   it too, unless /DELETEDATA is given (for scripted tests and IT).
; - Never touches the identity key in Windows Credential Manager: with it (or
;   the recovery words) the account comes back after a reinstall.
; - An update runs the old uninstaller too; then nothing is deleted.
; - Always removes the mvs:// registration the app made for this user.
; - Leaves nothing else: the installer cache and an empty Programs folder go
;   too (customUnInstallSection below).

!include "FileFunc.nsh"

!macro customUnInstall
  DeleteRegKey HKCU "Software\Classes\mvs"

  ${ifNot} ${isUpdated}
    ${GetParameters} $R0
    ClearErrors
    ${GetOptions} $R0 "/DELETEDATA" $R1
    ${ifNot} ${Errors}
      RMDir /r "$APPDATA\MVS"
    ${else}
      MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "למחוק גם את ההודעות והשרתים השמורים במחשב הזה?$\r$\nDelete the messages and servers stored on this computer as well?$\r$\n$\r$\nהחשבון עצמו לא נמחק: אפשר לשחזר אותו עם מילות השחזור.$\r$\nYour account is not deleted: you can restore it with your recovery words." /SD IDNO IDNO keepData
        RMDir /r "$APPDATA\MVS"
      keepData:
    ${endIf}
  ${endIf}
!macroend

; Runs after electron-builder's own uninstall section, i.e. after the program
; folder is gone (issue #94). Not on an update: the new version is being
; installed in the same place.
;   - %LOCALAPPDATA%\<package>-updater: electron-builder's installer cache.
;   - %LOCALAPPDATA%\Programs: only if it is now empty (RMDir without /r
;     never deletes anything else).
!macro customUnInstallSection
  Section "un.mvsCleanup"
    ${ifNot} ${isUpdated}
      RMDir /r "$LOCALAPPDATA\${APP_PACKAGE_NAME}-updater"
      RMDir "$LOCALAPPDATA\Programs"
    ${endIf}
  SectionEnd
!macroend

; B8 (in-app update, core/updater/docs/B8-in-app-update.md §0): the shortcuts
; start mvs-launcher, which picks the healthy version (the base copy in
; $INSTDIR or one under $INSTDIR\versions\) and forwards argv verbatim. Only
; when the launcher is installed (builds before B8-3 have none), and only the
; shortcuts the installer actually created.
!macro customInstall
  ${if} ${FileExists} "$INSTDIR\mvs-launcher.exe"
    ${if} ${FileExists} "$SMPROGRAMS\${SHORTCUT_NAME}.lnk"
      CreateShortCut "$SMPROGRAMS\${SHORTCUT_NAME}.lnk" "$INSTDIR\mvs-launcher.exe" "" "$INSTDIR\${APP_EXECUTABLE_FILENAME}" 0
    ${endIf}
    ${if} ${FileExists} "$DESKTOP\${SHORTCUT_NAME}.lnk"
      CreateShortCut "$DESKTOP\${SHORTCUT_NAME}.lnk" "$INSTDIR\mvs-launcher.exe" "" "$INSTDIR\${APP_EXECUTABLE_FILENAME}" 0
    ${endIf}
  ${endIf}
!macroend
