; Inno Setup script for the Windows installer. CI builds it with:
;   iscc /DAppVersion=0.7.0 /DExe=path\to\ts-launcher.exe /DOutputName=ts-launcher-v0.7.0-windows-x86_64-setup installer\ts-launcher.iss
; It installs per user (no administrator prompt) into %LOCALAPPDATA%\Programs, so the launcher
; can still replace its own .exe when it updates itself.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Exe
  #define Exe "..\target\release\ts-launcher.exe"
#endif
#ifndef OutputName
  #define OutputName "ts-launcher-setup"
#endif

[Setup]
AppId={{6C1F3E0B-2D7A-4B8E-9F1C-5A3D7E2B9C40}
AppName=The Specialists Launcher
AppVersion={#AppVersion}
AppVerName=The Specialists Launcher {#AppVersion}
AppPublisher=The Specialists mod
AppPublisherURL=https://tsmod.org
AppSupportURL=https://github.com/the-specialists-mod/tsmod-launcher
AppUpdatesURL=https://github.com/the-specialists-mod/tsmod-launcher/releases
DefaultDirName={autopf}\The Specialists Launcher
DefaultGroupName=The Specialists
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\assets\icon\ts-launcher.ico
UninstallDisplayIcon={app}\ts-launcher.exe
UninstallDisplayName=The Specialists Launcher
WizardStyle=modern
WizardImageFile=wizard.bmp
WizardSmallImageFile=wizard-small.bmp
Compression=lzma2/max
SolidCompression=yes
OutputDir=..\target\installer
OutputBaseFilename={#OutputName}
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#Exe}"; DestDir: "{app}"; DestName: "ts-launcher.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\The Specialists Launcher"; Filename: "{app}\ts-launcher.exe"
Name: "{autodesktop}\The Specialists Launcher"; Filename: "{app}\ts-launcher.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ts-launcher.exe"; Description: "{cm:LaunchProgram,The Specialists Launcher}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; The previous binary a self-update leaves behind until the next start.
Type: files; Name: "{app}\ts-launcher.exe.old"
