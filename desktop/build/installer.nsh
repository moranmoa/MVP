; Uninstaller additions for the NSIS installer (electron-builder `nsis.include`).
;
; - Asks whether to delete the local data (messages, servers, settings in
;   %APPDATA%\MVS). The default is to keep it. A silent uninstall (/S) keeps
;   it too, unless /DELETEDATA is given (for scripted tests and IT).
; - Never touches the identity key in Windows Credential Manager: with it (or
;   the recovery words) the account comes back after a reinstall.
; - An update runs the old uninstaller too; then nothing is deleted.
; - Always removes the mvs:// registration the app made for this user.

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
