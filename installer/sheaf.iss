; Sheaf installer (Inno Setup 6). Per user, no administrator rights, nothing outside the user's own
; folders and registry hive (ADR-EXPLORER-VERBS, SEC-NONET: the installer never touches the network).
;
;   iscc installer\sheaf.iss /DVersion=0.1.0 /DStage=C:\path\to\dist\stage /ODIST
;
; Build the staging folder first with tools\package\stage.ps1.

#ifndef Version
  #define Version "0.1.0"
#endif
#ifndef Stage
  #define Stage "..\dist\stage"
#endif

[Setup]
AppId={{6F1D2B8E-3A47-4C59-9B0E-5D7A21C84F13}
AppName=Sheaf
AppVersion={#Version}
AppPublisher=Sheaf
DefaultDirName={autopf}\Sheaf
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=Sheaf-Setup-{#Version}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\sheaf.exe
CloseApplications=yes
RestartApplications=no

[Files]
Source: "{#Stage}\sheaf.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\sheafc.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\sheaf-worker.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD_PARTY_LICENSES"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\vendor\*"; DestDir: "{app}\vendor"; Flags: ignoreversion recursesubdirs createallsubdirs

[Tasks]
; The action wheel needs its small helper to be running; offering to start it at sign-in makes it work with no further steps.
Name: "wheel"; Description: "Start the action wheel when I sign in"; GroupDescription: "Quick actions:"

[Registry]
; The same value the helper's own tray menu ("Start with Windows") writes, so its check mark agrees.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Sheaf"; ValueData: """{app}\sheaf.exe"" tray"; Flags: uninsdeletevalue; Tasks: wheel

[Icons]
Name: "{autoprograms}\Sheaf"; Filename: "{app}\sheaf.exe"

[Run]
; The right-click menu (current user). The console twin is used so that Setup waits for it.
Filename: "{app}\sheafc.exe"; Parameters: "install-verbs"; Flags: runhidden waituntilterminated; StatusMsg: "Adding Sheaf to the right-click menu..."
Filename: "{app}\sheaf.exe"; Parameters: "tray"; Description: "Start the action wheel now"; Flags: nowait postinstall skipifsilent; Tasks: wheel
Filename: "{app}\sheaf.exe"; Description: "Start Sheaf"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\sheafc.exe"; Parameters: "uninstall-verbs"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveSheafVerbs"

[UninstallDelete]
; Settings and presets (%APPDATA%\Sheaf) are the person's own and stay; scratch sessions do not.
Type: filesandordirs; Name: "{localappdata}\Sheaf\verbs"
