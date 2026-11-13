; MarkTurbo Windows installer.
;
; Compile through `scripts/mt.py release package`, which supplies the payload,
; version defines and output path. This script only describes where the packaged
; files go: it never owns user documents, settings or recovery data.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#ifndef AppVersionNumeric
  #define AppVersionNumeric "0.0.0"
#endif

#ifndef PayloadDir
  #define PayloadDir "payload"
#endif

#ifndef OutputDir
  #define OutputDir "dist"
#endif

#ifndef OutputBaseFilename
  #define OutputBaseFilename "MarkTurbo-Setup"
#endif

#ifndef SetupIcon
  #define SetupIcon ""
#endif

#ifndef RepoRoot
  #define RepoRoot ".."
#endif

[Setup]
AppId={{6BC0E0A7-53F2-4C61-9A59-5E8D7B4C2F10}
AppName=MarkTurbo
AppVersion={#AppVersion}
AppVerName=MarkTurbo {#AppVersion}
AppPublisher=MarkTurbo
AppPublisherURL=https://github.com/wxxb789/markturbo
AppSupportURL=https://github.com/wxxb789/markturbo/issues
AppUpdatesURL=https://github.com/wxxb789/markturbo/releases
DefaultDirName={localappdata}\Programs\MarkTurbo
DefaultGroupName=MarkTurbo
DisableProgramGroupPage=yes
DisableWelcomePage=no
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseFilename}
SetupIconFile={#SetupIcon}
LicenseFile={#RepoRoot}\LICENSE
UninstallDisplayIcon={app}\MarkTurbo.exe
UninstallDisplayName=MarkTurbo {#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
UsePreviousAppDir=yes
VersionInfoVersion={#AppVersionNumeric}.0
VersionInfoProductVersion={#AppVersionNumeric}
VersionInfoProductName=MarkTurbo
VersionInfoCompany=MarkTurbo
VersionInfoDescription=MarkTurbo installer

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional icons:"; Flags: unchecked

[Files]
Source: "{#PayloadDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\MarkTurbo"; Filename: "{app}\MarkTurbo.exe"
Name: "{autodesktop}\MarkTurbo"; Filename: "{app}\MarkTurbo.exe"; Tasks: desktopicon

[Registry]
; Lets the Windows shell and `start MarkTurbo` resolve the per-user install.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\MarkTurbo.exe"; ValueType: string; ValueData: "{app}\MarkTurbo.exe"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\MarkTurbo.exe"; ValueType: string; ValueName: "Path"; ValueData: "{app}"; Flags: uninsdeletekey

[Run]
Filename: "{app}\MarkTurbo.exe"; Description: "Launch MarkTurbo"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; The uninstaller must not leave a running instance behind. Setup's Restart
; Manager close path is not used by Uninstall, so end the app (and its WebView2
; children) before the installer-owned files are removed.
Filename: "{sys}\taskkill.exe"; Parameters: "/F /T /IM MarkTurbo.exe"; RunOnceId: "StopMarkTurbo"; Flags: runhidden
