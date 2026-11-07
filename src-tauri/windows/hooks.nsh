; インストーラ(NSIS)のフック。
;
; 「プログラムから開く」の候補に、uni-zipperを出す。`.zip` を開く既定のアプリは、変えない。
; (Tauriの `fileAssociations` を使うと、`.zip` の既定のアプリが、uni-zipperに書き換わってしまうため、使わない。)
;
; 登録するもの(インストールのモードに応じて、HKCUまたはHKLM。`SHCTX`):
;   Software\Classes\Applications\uni-zipper.exe  ... アプリの登録(対応する拡張子、開くコマンド)
;   Software\Classes\.zip\OpenWithList\uni-zipper.exe ... .zip の「プログラムから開く」の候補

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe" "FriendlyAppName" "${PRODUCTNAME}"
  WriteRegStr SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  WriteRegStr SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe\SupportedTypes" ".zip" ""
  WriteRegStr SHCTX "Software\Classes\.zip\OpenWithList\${MAINBINARYNAME}.exe" "" ""
  ; エクスプローラーに、関連付けが変わったことを知らせる(SHCNE_ASSOCCHANGED)
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe"
  DeleteRegKey SHCTX "Software\Classes\.zip\OpenWithList\${MAINBINARYNAME}.exe"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend
