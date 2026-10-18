#ifndef AppVersion
  #define AppVersion "0.1.2"
#endif
[Setup]
AppId=io.github.zaydzaari.Lumen
AppName=Lumen
AppVersion={#AppVersion}
AppPublisher=Lumen
AppPublisherURL=https://github.com/zaydzaari/Lumen
DefaultDirName={localappdata}\Programs\Lumen
DefaultGroupName=Lumen
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=lumen-{#AppVersion}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
UninstallDisplayIcon={app}\lumen.exe
LicenseFile=..\..\LICENSE
[Files]
Source: "..\..\target\release\lumen.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\assets\fonts\*-OFL.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion
[Icons]
Name: "{group}\Lumen"; Filename: "{app}\lumen.exe"
[Run]
Filename: "{app}\lumen.exe"; Description: "Open Lumen"; Flags: nowait postinstall skipifsilent
