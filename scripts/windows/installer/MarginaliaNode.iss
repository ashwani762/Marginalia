; The setup wizard for the Marginalia compute node (Inno Setup 6). It installs for the current user only, with no
; administrator needed for the programs. It offers starting at sign in (on by default), the Windows Firewall rules that let
; phones on a private network reach the node (one Windows prompt, on by default) and the models. It stops a running node
; and tray before replacing them, and its uninstaller takes out what the setup made and asks about the settings and the
; models. Build it with scripts\windows\installer\build-installer.ps1, which passes the version and the paths.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\..\target\setup"
#endif

#define AppName "Marginalia Node"
; The folder setup.ps1 uses too, so an install by script and by this wizard are one install.
#define FolderName "MarginaliaNode"
#define NodeExe "marginalia-node.exe"
#define TrayExe "marginalia-node-tray.exe"
#define SpeakersExe "marginalia-node-speakers.exe"
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"
; The entry's name and command are the tray's own (compute-node/tray/src/autostart.rs), so its switch and this one agree.
#define RunName "Marginalia Node"
#define OwnKey "Software\Marginalia Node"
#define FwNode "Marginalia Node (UDP 47615)"
#define FwMdns "Marginalia Node (mDNS UDP 5353)"

[Setup]
AppId={{5B1D6F0E-3C4A-4C7B-9A52-8E2F7A1D0C43}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Ashwani Kumar
PrivilegesRequired=lowest
DefaultDirName={localappdata}\{#FolderName}
DisableProgramGroupPage=yes
DisableDirPage=auto
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir={#OutputDir}
OutputBaseFilename=Marginalia-Node-Setup-{#AppVersion}
SetupIconFile=..\..\..\compute-node\packaging\app.ico
UninstallDisplayIcon={app}\{#TrayExe}
UninstallDisplayName={#AppName}
VersionInfoVersion={#AppVersion}
VersionInfoDescription={#AppName} Setup
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; Programs in use are closed, not skipped, whatever the order of events.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
en.StartAtSignIn=Start the Marginalia node when I sign in
en.Firewall=Let phones on a private network reach the node (Windows asks once)
en.Models=Download the models now (about 3.7 GB, it can be stopped and continued)
en.DeleteSettings=Also delete the node's settings and the list of paired phones?%n%nThey are in:%n%1%n%nChoose No to keep them: they will be there if you install the node again.
en.DeleteModels=Also delete the downloaded models (about 4 GB or more)?%n%nThey are in:%n%1%n%nChoose No to keep them: installing again will not need to download them.

[Tasks]
Name: "startup"; Description: "{cm:StartAtSignIn}"
Name: "firewall"; Description: "{cm:Firewall}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#NodeExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\{#TrayExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\{#SpeakersExe}"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#TrayExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#TrayExe}"; Tasks: desktopicon

[Registry]
; The same entry the tray's own switch writes: quoted program, then the flag that makes the tray start the node too.
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "{#RunName}"; ValueData: """{app}\{#TrayExe}"" --at-login"; Tasks: startup
; What this setup added, so the uninstaller takes out only that.
Root: HKCU; Subkey: "{#OwnKey}"; ValueType: dword; ValueName: "Firewall"; ValueData: 1; Flags: uninsdeletekey; Tasks: firewall

[Run]
; The login tasks that scripts\windows\setup.ps1 makes would start the programs a second time: they go.
Filename: "{sys}\schtasks.exe"; Parameters: "/Delete /TN ""Marginalia Node"" /F"; Flags: runhidden; StatusMsg: "Removing the old login tasks"
Filename: "{sys}\schtasks.exe"; Parameters: "/Delete /TN ""Marginalia Node Tray"" /F"; Flags: runhidden
; The firewall rules need administrator: one Windows prompt for both. Program rules, private networks only.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""{#FwNode}"""; Verb: "runas"; Flags: shellexec runhidden waituntilterminated; Tasks: firewall; StatusMsg: "Allowing phones through the firewall"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""{#FwMdns}"""; Verb: "runas"; Flags: shellexec runhidden waituntilterminated; Tasks: firewall
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""{#FwNode}"" dir=in action=allow protocol=UDP localport=47615 program=""{app}\{#NodeExe}"" profile=private"; Verb: "runas"; Flags: shellexec runhidden waituntilterminated; Tasks: firewall
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""{#FwMdns}"" dir=in action=allow protocol=UDP localport=5353 program=""{app}\{#NodeExe}"" profile=private"; Verb: "runas"; Flags: shellexec runhidden waituntilterminated; Tasks: firewall
; The models, in a window that shows the progress, then the tray (which starts the node).
Filename: "{app}\{#NodeExe}"; Parameters: "models download"; Description: "{cm:Models}"; Flags: postinstall waituntilterminated skipifsilent; Check: ModelsMissing
Filename: "{app}\{#TrayExe}"; Parameters: "--at-login"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""{#FwNode}"""; Verb: "runas"; Flags: shellexec runhidden waituntilterminated; RunOnceId: "FwNode"; Check: FirewallWasAdded
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""{#FwMdns}"""; Verb: "runas"; Flags: shellexec runhidden waituntilterminated; RunOnceId: "FwMdns"; Check: FirewallWasAdded

[Code]
function ModelsFolder(): String;
begin
  Result := ExpandConstant('{userappdata}\marginalia\models');
end;

function SettingsFolder(): String;
begin
  Result := ExpandConstant('{userappdata}\marginalia-node');
end;

// The three files the node needs. When they are all there nothing is offered.
function ModelsMissing(): Boolean;
begin
  Result := not (FileExists(ModelsFolder() + '\embeddinggemma-2-Q8_0.gguf')
    and FileExists(ModelsFolder() + '\ggml-large-v3-turbo-q5_0.bin')
    and (FileExists(ModelsFolder() + '\gemma-4-E2B-it-Q4_0.gguf') or FileExists(ModelsFolder() + '\gemma-4-E4B-it-Q4_0.gguf')));
end;

function FirewallWasAdded(): Boolean;
var
  Added: Cardinal;
begin
  Result := RegQueryDWordValue(HKCU, '{#OwnKey}', 'Firewall', Added) and (Added = 1);
end;

function IsRunning(Image: String): Boolean;
var
  Code: Integer;
begin
  Result := Exec(ExpandConstant('{cmd}'), '/C tasklist /FI "IMAGENAME eq ' + Image + '" | find /I "' + Image + '"', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
end;

// Asks a running node to stop (its own `stop`, which lets a job in progress finish), takes the tray down, and waits.
// A program that has not gone after ten seconds is ended.
procedure StopPrograms(Folder: String);
var
  Code, Tries: Integer;
begin
  if FileExists(Folder + '\{#NodeExe}') and IsRunning('{#NodeExe}') then
    Exec(Folder + '\{#NodeExe}', 'stop', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM {#TrayExe} /F', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Tries := 0;
  while IsRunning('{#NodeExe}') and (Tries < 100) do
  begin
    Sleep(100);
    Tries := Tries + 1;
  end;
  if IsRunning('{#NodeExe}') then
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM {#NodeExe} /F', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM {#SpeakersExe} /F', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

// Once Install is clicked, not as the wizard opens: a cancelled setup leaves the node running.
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  StopPrograms(WizardDirValue());
end;

// The sign in entry may also have been switched on later, from the tray: take it out either way, but only when it starts this copy.
procedure RemoveStartupEntry();
var
  Value: String;
begin
  if RegQueryStringValue(HKCU, '{#RunKey}', '{#RunName}', Value) and (Pos(Lowercase(ExpandConstant('{app}')), Lowercase(Value)) > 0) then
    RegDeleteValue(HKCU, '{#RunKey}', '{#RunName}');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    StopPrograms(ExpandConstant('{app}'));
    RemoveStartupEntry();
  end;
  if CurUninstallStep = usPostUninstall then
  begin
    // No is the default answer and the one a silent uninstall takes: the identity, the paired phones and the models are worth keeping.
    if DirExists(SettingsFolder()) then
      if SuppressibleMsgBox(FmtMessage(CustomMessage('DeleteSettings'), [SettingsFolder()]), mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
        DelTree(SettingsFolder(), True, True, True);
    if DirExists(ModelsFolder()) then
      if SuppressibleMsgBox(FmtMessage(CustomMessage('DeleteModels'), [ModelsFolder()]), mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
        DelTree(ModelsFolder(), True, True, True);
  end;
end;
