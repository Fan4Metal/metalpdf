; MetalPDF installer (Inno Setup 6).
; Built by tools\make_release.py, which passes the version and the icon path:
;   ISCC /DMyAppVersion=0.1.0 /DVersionInfoVersion=0.1.0.0 /DAppIcon=..\target\app.ico tools\setup.iss

#define MyAppName "MetalPDF"
#define MyAppFileStem "MetalPDF"
#define MyAppExeName "metalpdf.exe"
#define MyAppPublisher "Fan4_Metal"
#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#ifndef VersionInfoVersion
  #define VersionInfoVersion "0.0.0.0"
#endif
#ifndef AppIcon
  #define AppIcon "..\target\app.ico"
#endif

[Setup]
; A new GUID for every app (the same one in tools\make_release.py).
AppId={{552B831C-3586-44E1-A282-FDAFAAE5A014}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
VersionInfoVersion={#VersionInfoVersion}
AppCopyright=Copyright (C) 2026 {#MyAppPublisher}
AppPublisher={#MyAppPublisher}
; Per-user installation without administrator rights:
; {autopf} points to %LOCALAPPDATA%\Programs.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename={#MyAppFileStem}_{#MyAppVersion}_Setup
SetupIconFile={#AppIcon}
UninstallDisplayIcon={app}\{#MyAppExeName}
LicenseFile=..\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; A running copy is closed (Restart Manager) before its exe is replaced.
CloseApplications=yes
; File associations change: Explorer is told to refresh its icons.
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
english.AssocGroup=File types:
russian.AssocGroup=Типы файлов:
english.AssocTask=Offer MetalPDF for PDF files (in "Open with" and Default apps)
russian.AssocTask=Предлагать MetalPDF для файлов PDF (в «Открыть с помощью» и приложениях по умолчанию)
english.AssocStatus=Registering the file type...
russian.AssocStatus=Регистрация типа файлов...
english.ChooseDefault=Choose MetalPDF as the default program for PDF files (Settings)
russian.ChooseDefault=Выбрать MetalPDF программой по умолчанию для PDF (Параметры)

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "assoc"; Description: "{cm:AssocTask}"; GroupDescription: "{cm:AssocGroup}"

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
; The rendering engine (PDFium), loaded from the program's folder.
Source: "..\target\release\pdfium.dll"; DestDir: "{app}"; Flags: ignoreversion
; The thumbnail provider Explorer loads for PDF files (registered by --register).
Source: "..\target\release\metalpdf_thumbs.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD_PARTY_LICENSES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\licenses\PDFium-LICENSE.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\licenses\THIRD_PARTY_NOTICES.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.ru.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
; The file type is registered by the program itself (src\assoc.rs, under
; HKCU); Windows lets only the user make it the default, in Settings.
Filename: "{app}\{#MyAppExeName}"; Parameters: "--register"; StatusMsg: "{cm:AssocStatus}"; Flags: runhidden waituntilterminated; Tasks: assoc
Filename: "ms-settings:defaultapps?registeredAppUser=MetalPDF"; Description: "{cm:ChooseDefault}"; Flags: shellexec nowait postinstall skipifsilent unchecked; Tasks: assoc
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Removes everything --register wrote; nothing happens if it was not run.
Filename: "{app}\{#MyAppExeName}"; Parameters: "--unregister"; RunOnceId: "UnregisterFileTypes"; Flags: runhidden waituntilterminated

; Optional blocks, taken from the sibling projects when needed:
; - Explorer context menu for folders and drives ([Registry] under
;   HKCU\Software\Classes\{Directory,Drive}\shell\<Verb>, task "contextmenu"):
;   disk_flashlight/tools/setup.iss.
; - File associations through the registry (ProgID, OpenWithProgids,
;   Capabilities, RegisteredApplications, a [Run] entry that opens
;   ms-settings:defaultapps, ChangesAssociations=yes): metalmd/tools/setup.iss.
;   qview registers them from the exe instead (qview --register).
; - Asking a running copy to quit through its own --quit ([Code]
;   PrepareToInstall and [UninstallRun]) and offering to delete the settings
;   on uninstallation (CurUninstallStepChanged): qrec/tools/setup.iss.
; - MinVersion=10.0.19041 when an API needs Windows 10 2004: qrec.

[UninstallDelete]
Type: files; Name: "{app}\metalpdf_thumbs.dll.old"

[Code]
// Explorer's surrogate (dllhost.exe) keeps the thumbnail provider loaded
// for a while after drawing thumbnails, and a loaded DLL cannot be
// overwritten, only renamed: the old one is moved aside before the files
// are copied and deleted at the next installation.
procedure CurStepChanged(CurStep: TSetupStep);
var
  Dll: String;
begin
  if CurStep = ssInstall then
  begin
    Dll := ExpandConstant('{app}\metalpdf_thumbs.dll');
    DeleteFile(Dll + '.old');
    if FileExists(Dll) and not DeleteFile(Dll) then
      RenameFile(Dll, Dll + '.old');
  end;
end;
