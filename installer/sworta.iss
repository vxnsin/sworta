#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{8F2C5B1E-6D3A-4E7B-9C41-5A0D7E2B9F13}
AppName=SWORTA
AppVersion={#AppVersion}
AppPublisher=Vensin
AppPublisherURL=https://github.com/vxnsin/sworta
AppSupportURL=https://github.com/vxnsin/sworta/issues
; Per-user install so the in-app updater can replace the exe without admin rights.
DefaultDirName={localappdata}\Programs\SWORTA
DefaultGroupName=SWORTA
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=SWORTA-Setup
SetupIconFile=sworta.ico
UninstallDisplayIcon={app}\SWORTA.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\sworta.exe"; DestDir: "{app}"; DestName: "SWORTA.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\SWORTA"; Filename: "{app}\SWORTA.exe"
Name: "{autodesktop}\SWORTA"; Filename: "{app}\SWORTA.exe"; Tasks: desktopicon

[Registry]
; The app registers its own autostart entry; this only removes it on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "SWORTA"; ValueType: none; Flags: uninsdeletevalue

[UninstallDelete]
Type: files; Name: "{app}\*.old"
Type: files; Name: "{app}\*.new"

[Run]
Filename: "{app}\SWORTA.exe"; Description: "Launch SWORTA"; Flags: nowait postinstall skipifsilent

[Code]
procedure StopApp;
var
  Code: Integer;
begin
  Exec('taskkill.exe', '/F /IM SWORTA.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

function InitializeSetup: Boolean;
begin
  StopApp;
  Result := True;
end;

function InitializeUninstall: Boolean;
begin
  StopApp;
  Result := True;
end;
