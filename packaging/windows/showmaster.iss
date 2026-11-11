#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{B7E1B2B4-5C0E-4E63-9A4D-7B0F5D2C8E31}
AppName=Show Master
AppVersion={#AppVersion}
AppPublisher=Cheter
DefaultDirName={autopf}\Show Master
DefaultGroupName=Show Master
DisableProgramGroupPage=yes
OutputDir=..\..\dist
OutputBaseFilename=ShowMaster-Setup-{#AppVersion}
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\showmaster.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ChangesAssociations=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\..\stage\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{autoprograms}\Show Master"; Filename: "{app}\showmaster.exe"; Parameters: "--open"
Name: "{autodesktop}\Show Master"; Filename: "{app}\showmaster.exe"; Parameters: "--open"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\.smshow"; ValueType: string; ValueData: "ShowMaster.Show"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\ShowMaster.Show"; ValueType: string; ValueData: "Show Master show"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\ShowMaster.Show\DefaultIcon"; ValueType: string; ValueData: "{app}\showmaster.exe,0"
Root: HKA; Subkey: "Software\Classes\ShowMaster.Show\shell\open\command"; ValueType: string; ValueData: """{app}\showmaster.exe"" --open ""%1"""

[Run]
Filename: "{app}\showmaster.exe"; Parameters: "--open"; Description: "{cm:LaunchProgram,Show Master}"; Flags: nowait postinstall
