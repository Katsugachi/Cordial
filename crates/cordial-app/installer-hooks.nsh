; Only remove Cordial-managed downloads, never user chats or workspace files.
; Tauri invokes the same uninstaller during updates: preserve models then.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    SetShellVarContext current
    Delete "$APPDATA\Cordial\models\LFM3-VL-2B-Q4_K_M.gguf"
    Delete "$APPDATA\Cordial\models\LFM3-VL-2B-Q4_K_M.gguf.part"
    Delete "$APPDATA\Cordial\models\mmproj-LFM3-VL-2B-Q8_0.gguf"
    Delete "$APPDATA\Cordial\models\mmproj-LFM3-VL-2B-Q8_0.gguf.part"
    RMDir "$APPDATA\Cordial\models"
  ${EndIf}
!macroend
