; lenswin 安装脚本，由 build-installer.ps1 传入版本号、exe、图标和输出目录。
#ifndef AppVersion
  #error 缺少 AppVersion，请通过 build-installer.ps1 编译
#endif
#ifndef SourceExe
  #error 缺少 SourceExe
#endif
#ifndef IconFile
  #error 缺少 IconFile
#endif
#ifndef LicenseFile
  #error 缺少 LicenseFile
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
AppId={{7C1D4E2A-5B3F-4A68-9E21-3D8F6A0B1C47}
AppName=lenswin
AppVersion={#AppVersion}
AppPublisher=LinYU
DefaultDirName={autopf}\lenswin
DisableProgramGroupPage=yes
; 安装开始时让用户选择「仅当前用户」或「所有用户」
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; 与程序的单实例互斥量同名，运行中时提示先退出
AppMutex=Local\lenswin
OutputDir={#OutputDir}
OutputBaseFilename=lenswin-{#AppVersion}-setup-x64
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\lenswin.exe
LicenseFile={#LicenseFile}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#LicenseFile}"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; 安装目录里不能有 portable 标记，否则会被误判为便携版
Type: files; Name: "{app}\portable"

[Icons]
Name: "{autoprograms}\lenswin"; Filename: "{app}\lenswin.exe"
Name: "{autodesktop}\lenswin"; Filename: "{app}\lenswin.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\lenswin.exe"; Description: "启动 lenswin"; Flags: nowait postinstall skipifsilent runasoriginaluser

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    // 开机自启项由程序写入当前用户的注册表，卸载时一并清理；不存在时无影响
    RegDeleteValue(HKCU, RunKey, 'lenswin');
  if CurUninstallStep = usPostUninstall then
  begin
    // 只能处理执行卸载的这个用户的数据，其他用户的配置不动
    if not UninstallSilent then
      if MsgBox('是否同时删除当前用户的 lenswin 配置和日志？' + #13#10#13#10 +
                '其他用户的配置不会被删除。', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      begin
        DelTree(ExpandConstant('{userappdata}\lenswin'), True, True, True);
        DelTree(ExpandConstant('{localappdata}\lenswin'), True, True, True);
      end;
  end;
end;
