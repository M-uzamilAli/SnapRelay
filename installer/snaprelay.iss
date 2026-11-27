; SnapRelay installer (Inno Setup 6). Build: installer\build.ps1
; Per-user install (no admin needed) into %LOCALAPPDATA%\Programs\SnapRelay.

#define AppName "SnapRelay"
#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppExe "snaprelay.exe"
#define RepoUrl "https://github.com/M-uzamilAli/SnapRelay"

[Setup]
; Keep this id the same for every version, so updates replace the old install.
AppId={{F1CEE34B-9D0D-4FB2-9FE6-426205C41944}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=M-uzamilAli
AppPublisherURL={#RepoUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=SnapRelay-Setup-{#AppVersion}
SetupIconFile=..\assets\icons\snaprelay-dark.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; SnapRelay has no window to close politely; we stop it ourselves (see [Code]).
CloseApplications=no

[Tasks]
Name: "startup"; Description: "Start SnapRelay when I sign in to Windows"; GroupDescription: "Start-up:"
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
; The Inter font is built into SnapRelay; its licence (SIL OFL) travels with it.
Source: "..\assets\fonts\Inter-LICENSE.txt"; DestDir: "{app}"; DestName: "Inter-font-LICENSE.txt"

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; The same sign-in entry SnapRelay's own "Start with Windows" switch uses,
; so the switch in Settings shows the right state.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#AppExe}"""; Tasks: startup; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start SnapRelay now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM {#AppExe}"; Flags: runhidden; RunOnceId: "StopSnapRelay"

[UninstallDelete]
Type: dirifempty; Name: "{app}"

[Code]
// An update can't replace snaprelay.exe while it's running in the tray.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;

// Uninstalling also removes the sign-in entry if it was turned on from Settings.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', '{#AppName}');
end;
