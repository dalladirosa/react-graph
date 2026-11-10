; Inno Setup script for the Rendermap Windows installer.
; Built by .github/workflows/release.yml; to build by hand (after `cargo build --release`):
;   iscc /DAppVersion=0.1.1 installer\rendermap.iss
; The installer is written to dist\rendermap-<version>-setup.exe.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define AppName "Rendermap"
#define AppExe "rendermap.exe"
#define AppPublisher "dalladirosa"
#define AppURL "https://github.com/dalladirosa/react-graph"

[Setup]
; keep this AppId forever: it is how upgrades and the uninstaller find an existing install
; (it dates from when the app was called React Graph, so those installs upgrade in place)
AppId={{3C401576-345D-4BF9-8BCB-2353F6E4FE11}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; installs for the current user without admin rights; the user can choose "all users" instead
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\assets\app.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir=..\dist
OutputBaseFilename=rendermap-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[InstallDelete]
; left behind by React Graph installs that this one upgrades (same AppId, same folder)
Type: files; Name: "{app}\react-graph.exe"
Type: files; Name: "{autoprograms}\React Graph.lnk"
Type: files; Name: "{autodesktop}\React Graph.lnk"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
