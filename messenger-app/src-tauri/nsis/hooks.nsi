; Veto installer hook — flush Windows icon cache after install
; Called by Tauri-generated NSIS installer via installerHooks.
!macro customInstall
  ExecWait '"$SYSDIR\ie4uinit.exe" -show' $0
!macroend
