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
Filename: "{app}\lenswin.exe"; Description: "Launch lenswin"; Flags: nowait postinstall skipifsilent runasoriginaluser

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  RunValue = 'lenswin';

// 取命令行里的 exe 路径：去掉首尾空白和引号，忽略后面的参数
function ExePathOfCommand(Command: String): String;
var
  Rest: String;
  QuoteEnd: Integer;
begin
  Rest := Trim(Command);
  if (Length(Rest) > 0) and (Rest[1] = '"') then
  begin
    Delete(Rest, 1, 1);
    QuoteEnd := Pos('"', Rest);
    if QuoteEnd > 0 then
      Rest := Copy(Rest, 1, QuoteEnd - 1);
  end;
  Result := Rest;
end;

// 自启项指向本次卸载的安装目录时才算「属于安装版」
function RunValueBelongsToThisInstall: Boolean;
var
  Command: String;
begin
  Result := False;
  if RegQueryStringValue(HKCU, RunKey, RunValue, Command) then
    Result := CompareText(ExePathOfCommand(Command), ExpandConstant('{app}\lenswin.exe')) = 0;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    // 自启项由程序写入当前用户的注册表；指向别处（如便携版）的不能删
    if RunValueBelongsToThisInstall then
      RegDeleteValue(HKCU, RunKey, RunValue);
  if CurUninstallStep = usPostUninstall then
  begin
    // 只能处理执行卸载的这个用户的数据，其他用户的配置不动
    if not UninstallSilent then
      if MsgBox('Also delete the current user''s lenswin settings and logs?' + #13#10#13#10 +
                'Settings of other users are not removed.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      begin
        DelTree(ExpandConstant('{userappdata}\lenswin'), True, True, True);
        DelTree(ExpandConstant('{localappdata}\lenswin'), True, True, True);
      end;
  end;
end;
