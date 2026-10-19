#ifndef AppVersion
  #define AppVersion "0.2.0"
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
Source: "..\..\LICENSE-MIT"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\..\THIRD-PARTY.md"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\..\vendor\sonora\UPSTREAM.md"; DestDir: "{app}\licenses"; DestName: "Sonora-UPSTREAM.md"; Flags: ignoreversion
Source: "..\..\vendor\sonora\crates\widevine\shim\LICENSE"; DestDir: "{app}\licenses"; DestName: "Widevine-shim-LICENSE"; Flags: ignoreversion
Source: "..\..\vendor\sonora\crates\widevine\shim\cdm\LICENSE"; DestDir: "{app}\licenses"; DestName: "CDM-interface-LICENSE"; Flags: ignoreversion
Source: "..\..\assets\fonts\*-OFL.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion
[Icons]
Name: "{group}\Lumen"; Filename: "{app}\lumen.exe"
[Run]
Filename: "{app}\lumen.exe"; Description: "Open Lumen"; Flags: nowait postinstall skipifsilent
