!include "MUI2.nsh"

!ifndef APP_VERSION
  !define APP_VERSION "0.1.7"
!endif

Name "AI Switcher ${APP_VERSION}"
OutFile "..\upload\AI-Switcher-v${APP_VERSION}-windows-x86_64-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\AI Switcher"
InstallDirRegKey HKCU "Software\AI Switcher" "InstallDir"
RequestExecutionLevel user
Unicode true

!define MUI_ICON "..\assets\AppIcon.ico"
!define MUI_UNICON "..\assets\AppIcon.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\AI-Switcher.exe"
!define MUI_FINISHPAGE_RUN_TEXT "立即启动 AI Switcher"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

Section "AI Switcher 主程序 (必选)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"

  ; 如果旧实例在运行先结束，避免覆盖失败
  nsExec::ExecToStack 'taskkill /F /IM AI-Switcher.exe'
  Pop $0

  File /oname=AI-Switcher.exe "..\target\release\ai-switcher.exe"
  File "..\assets\AppIcon.ico"
  File "..\README.md"

  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; 写入安装路径与卸载信息
  WriteRegStr HKCU "Software\AI Switcher" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AI Switcher" "DisplayName" "AI Switcher"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AI Switcher" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AI Switcher" "Publisher" "xw-yin"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AI Switcher" "DisplayIcon" "$INSTDIR\AI-Switcher.exe,0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AI Switcher" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AI Switcher" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AI Switcher" "NoRepair" 1

  ; 开始菜单快捷方式
  CreateDirectory "$SMPROGRAMS\AI Switcher"
  CreateShortcut "$SMPROGRAMS\AI Switcher\AI Switcher.lnk" "$INSTDIR\AI-Switcher.exe" "" "$INSTDIR\AI-Switcher.exe" 0
  CreateShortcut "$SMPROGRAMS\AI Switcher\卸载 AI Switcher.lnk" "$INSTDIR\Uninstall.exe" "" "$INSTDIR\Uninstall.exe" 0
SectionEnd

Section "创建桌面快捷方式" SecDesktop
  CreateShortcut "$DESKTOP\AI Switcher.lnk" "$INSTDIR\AI-Switcher.exe" "" "$INSTDIR\AI-Switcher.exe" 0
SectionEnd

Section "Uninstall"
  nsExec::ExecToStack 'taskkill /F /IM AI-Switcher.exe'
  Pop $0

  Delete "$DESKTOP\AI Switcher.lnk"
  RMDir /r "$SMPROGRAMS\AI Switcher"
  Delete "$INSTDIR\AI-Switcher.exe"
  Delete "$INSTDIR\AppIcon.ico"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\AI Switcher"
  DeleteRegKey HKCU "Software\AI Switcher"
SectionEnd
