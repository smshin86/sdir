#include "payload-constants.iss"

#define ProductionAppId "SDIR-PerUser-4E7E0A18-79E0-41C0-97C5-65A10A79B3F4"
#define ProductionAppName "SDIR"
#define ProductionGroupName "SDIR"
#define ProductionRootName "SDIR"
#define ProductionDir "{localappdata}\Programs\SDIR"

#ifdef TEST_BUILD
  #if AppIdValue != TestAppIdGuardValue
    #error TEST_BUILD application identifier override rejected.
  #endif
  #if AppNameValue != TestAppNameGuardValue
    #error TEST_BUILD display name override rejected.
  #endif
  #if GroupNameValue != TestGroupNameGuardValue
    #error TEST_BUILD Start Menu group override rejected.
  #endif
  #if RootNameValue != TestRootNameGuardValue
    #error TEST_BUILD root override rejected.
  #endif
  #if DefaultDirValue != TestDefaultDirGuardValue
    #error TEST_BUILD default directory override rejected.
  #endif
  #if AppIdValue == ProductionAppId
    #error TEST_BUILD must use a separate application identifier.
  #endif
  #if AppNameValue == ProductionAppName
    #error TEST_BUILD must use a separate display name.
  #endif
  #if GroupNameValue == ProductionGroupName
    #error TEST_BUILD must use a separate Start Menu group.
  #endif
  #if RootNameValue == ProductionRootName
    #error TEST_BUILD must use a separate installation root.
  #endif
  #ifdef TEST_FAULT_BACKUP
    #ifdef TEST_FAULT_PARTIAL
      #error Only one TEST_FAULT_* hook may be enabled.
    #endif
    #ifdef TEST_FAULT_LATE
      #error Only one TEST_FAULT_* hook may be enabled.
    #endif
  #endif
  #ifdef TEST_FAULT_PARTIAL
    #ifdef TEST_FAULT_LATE
      #error Only one TEST_FAULT_* hook may be enabled.
    #endif
  #endif
#else
  #if AppIdValue != ProductionAppId
    #error Production application identity override rejected.
  #endif
  #if AppNameValue != ProductionAppName
    #error Production display name override rejected.
  #endif
  #if GroupNameValue != ProductionGroupName
    #error Production Start Menu group override rejected.
  #endif
  #if RootNameValue != ProductionRootName
    #error Production installation root override rejected.
  #endif
  #if DefaultDirValue != ProductionDir
    #error Production default directory override rejected.
  #endif
  #ifdef TEST_FAULT_BACKUP
    #error TEST_FAULT_BACKUP requires TEST_BUILD.
  #endif
  #ifdef TEST_FAULT_PARTIAL
    #error TEST_FAULT_PARTIAL requires TEST_BUILD.
  #endif
  #ifdef TEST_FAULT_LATE
    #error TEST_FAULT_LATE requires TEST_BUILD.
  #endif
  #ifdef TEST_MISSING_RUNTIME
    #error TEST_MISSING_RUNTIME requires TEST_BUILD.
  #endif
#endif

[Setup]
AppId={#AppIdValue}
AppName={#AppNameValue}
AppVersion=0.1
AppVerName={#AppNameValue} 0.1
AppPublisher=SDIR
DefaultDirName={#DefaultDirValue}
DefaultGroupName={#GroupNameValue}
UninstallDisplayName={#AppNameValue}
UninstallDisplayIcon={app}\SDIR.exe
UninstallFilesDir={app}
PrivilegesRequired=lowest
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
DisableDirPage=yes
DisableProgramGroupPage=yes
UsePreviousAppDir=no
CloseApplications=no
RestartApplications=no
RestartIfNeededByRun=no
CreateUninstallRegKey=yes
Uninstallable=yes
SetupLogging=yes
InfoBeforeFile=README-ko.txt
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir=output
OutputBaseFilename=SDIR-windows-x64-setup

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"

[Messages]
korean.ConfirmUninstall=%1을(를) 제거하시겠습니까?%n%n함께 설치한 색인 서비스와 색인 캐시도 정리합니다. 개인 설정은 보존합니다.

[Tasks]
Name: "indexservice"; Description: "색인 서비스 포함"; GroupDescription: "통합 검색:"; Check: ShowIndexServiceOption
Name: "desktopicon"; Description: "바탕 화면 바로 가기 추가"; GroupDescription: "바로 가기:"; Flags: unchecked

[Files]
#include "payload-files.iss"

[Run]
#ifdef TEST_BUILD
Filename: "{app}\sdir-installer-test-launcher.cmd"; Description: "SDIR 테스트 빌드 실행"; WorkingDir: "{app}"; Flags: postinstall nowait skipifsilent unchecked
#else
Filename: "{app}\SDIR.exe"; Description: "SDIR 실행"; WorkingDir: "{app}"; Flags: postinstall nowait skipifsilent unchecked
#endif

[Code]
const
  AppIdText = '{#AppIdValue}';
  AppNameText = '{#AppNameValue}';
  GroupNameText = '{#GroupNameValue}';

#define PAYLOAD_CONSTANTS_CODE
#include "payload-constants.iss"
#undef PAYLOAD_CONSTANTS_CODE
#include "rollback.iss"

function ShowIndexServiceOption(): Boolean;
begin
  Result := ExpandConstant('{param:SDIR-INDEX-OPTION|}') = '1';
end;

function GetCustomSetupExitCode(): Integer;
begin
  Result := 0;
  { The combined setup consumes 42 as a successful app-only installation. }
  if ShowIndexServiceOption() and not WizardIsTaskSelected('indexservice') then Result := 42;
end;

function InitializeSetup(): Boolean;
var
  ErrorText: String;
begin
  InitializePayloadManifest;
  Result := False;
  if not AcquireInstallerMutex(ErrorText) then begin
    Log('SDIR_SETUP_MUTEX_REFUSED: ' + ErrorText);
    SuppressibleMsgBox(ErrorText, mbError, MB_OK, IDOK);
    Exit;
  end;
  if not HasWebViewRuntime then begin
    ErrorText := 'Microsoft Edge WebView2 Evergreen Runtime이 필요합니다.' + #13#10 +
      'Microsoft 공식 안내에서 설치한 뒤 다시 실행하세요:' + #13#10 +
      'https://developer.microsoft.com/microsoft-edge/webview2/';
    Log('SDIR_WEBVIEW2_MISSING');
    SuppressibleMsgBox(ErrorText, mbError, MB_OK, IDOK);
    ReleaseInstallerMutex;
    Exit;
  end;
  Result := True;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Reason: String;
begin
  Result := '';
  NeedsRestart := False;
  if HasForbiddenCommandLineOverride then begin
    Result := '설치 경로, 프로그램 종료 또는 재부팅 옵션 재정의는 허용하지 않습니다.';
    Log('SDIR_PREFLIGHT_REFUSED: forbidden command-line override');
    Exit;
  end;
  if not ValidateRootAndOwnership(ExpandConstant('{app}'), Reason) then begin
    Result := Reason;
    Log('SDIR_PREFLIGHT_REFUSED: ' + Reason);
    Exit;
  end;
  if not SnapshotUninstallRegistry(Reason) then begin
    Log('SDIR_UNINSTALL_REGISTRY_PREFLIGHT_REFUSED: ' + Reason);
    Result := '설치 제거 정보를 안전하게 확인하지 못해 설치를 중단했습니다.' + #13#10 + Reason;
    SuppressibleMsgBox(Result,
      mbError, MB_OK, IDOK);
    ReleaseInstallerMutex;
    Exit;
  end;
  if not ProbeExternalLocks(ExpandConstant('{app}'), Reason) then begin
    Result := Reason;
    Log('SDIR_PREFLIGHT_REFUSED: ' + Reason);
    Exit;
  end;
  Log('SDIR_PREFLIGHT_OK: no filesystem mutation performed');
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Reason: String;
begin
  if CurStep = ssInstall then begin
    if not CapturePreInstallState(ExpandConstant('{app}'), Reason) then begin
      Log('SDIR_SNAPSHOT_FAILED: ' + Reason);
      RaiseException('설치 전 상태를 보존할 수 없습니다: ' + Reason);
    end;
    TransactionStarted := True;
    try
      if not QuarantineOwnedState(ExpandConstant('{app}'), Reason) then begin
        Log('SDIR_QUARANTINE_FAILED: ' + Reason);
        RestoreAfterFailure;
        RaiseException('기존 SDIR 파일 격리에 실패했습니다: ' + Reason);
      end;
      MarkObservedInstallerDirectories(ExpandConstant('{app}'));
      Log('SDIR_QUARANTINE_COMPLETE');
    except
      Reason := GetExceptionMessage;
      RestoreAfterFailure;
      RaiseException(Reason);
    end;
  end;

  if CurStep = ssPostInstall then begin
    NativeInstallCommitted := True;
    Log('SDIR_NATIVE_COMMIT');
    if not ReceiptAlreadyWritten then begin
      Log('SDIR_RECEIPT_MISSING_AFTER_COMMIT');
      SuppressibleMsgBox('설치는 완료됐지만 설치 영수증을 확인하지 못했습니다. 제거·업그레이드 전에 지원을 요청하세요.',
        mbError, MB_OK, IDOK);
    end;
    if not DisposeCommittedBackups then
      Log('SDIR_RECOVERY_BACKUP_RETAINED');
  end;
end;

procedure DeinitializeSetup;
begin
  if TransactionStarted and not NativeInstallCommitted and not RollbackCompleted then begin
    Log('SDIR_DEINITIALIZE_COMPENSATION_BEGIN');
    RestoreAfterFailure;
    if RollbackCompleted then
      Log('SDIR_DEINITIALIZE_COMPENSATION_COMPLETE')
    else
      Log('SDIR_DEINITIALIZE_COMPENSATION_INCOMPLETE');
  end;
  ReleaseInstallerMutex;
end;

const
  IndexOwnerKey = 'SOFTWARE\SDIR\IndexService';
  IndexImageKey = 'SYSTEM\CurrentControlSet\Services\SDIRIndex';
  IndexRemovalKey = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\SDIR-Index-Service-BF7D31D4-84CC-482A-8AE5-45C9BEA4100F_is1';

function IndexLinkKey(): String;
begin
  Result := 'Software\SDIR\Installations\' + AppIdText;
end;

function OpenIndexManager(MachineName, DatabaseName: Integer; Access: Cardinal): Integer;
  external 'OpenSCManagerW@advapi32.dll stdcall';
function OpenIndexService(Manager: Integer; Name: String; Access: Cardinal): Integer;
  external 'OpenServiceW@advapi32.dll stdcall';
function CloseIndexHandle(Handle: Integer): Boolean;
  external 'CloseServiceHandle@advapi32.dll stdcall';

function IndexRoot(): String;
begin
  Result := ExpandConstant('{commonpf64}\SDIR Index Service');
end;

function CurrentIndexOwner(): String;
var
  Code, Start, I: Integer;
  Output: TExecOutput;
  Line: String;
begin
  Result := '';
  if not ExecAndCaptureOutput(ExpandConstant('{sys}\whoami.exe'), '/user /fo csv /nh',
    ExpandConstant('{sys}'), SW_HIDE, ewWaitUntilTerminated, Code, Output) then Exit;
  if (Code <> 0) or Output.Error or (GetArrayLength(Output.StdOut) <> 1) then Exit;
  Line := Trim(Output.StdOut[0]);
  Start := Pos('","', Line);
  if (Start <= 1) or (Length(Line) <= Start + 3) then Exit;
  Line := Copy(Line, Start + 3, Length(Line) - Start - 3);
  if (Copy(Line, 1, 9) <> 'S-1-5-21-') or (Length(Line) > 54) then Exit;
  for I := 10 to Length(Line) do
    if Pos(Line[I], '0123456789-') = 0 then Exit;
  Result := Line;
end;

function IndexServiceAbsent(): Boolean;
var
  Manager, Service: Integer;
  Code: Cardinal;
begin
  Result := False;
  Manager := OpenIndexManager(0, 0, 1);
  if Manager = 0 then Exit;
  try
    Service := OpenIndexService(Manager, 'SDIRIndex', 4);
    Code := DLLGetLastError;
    if Service <> 0 then CloseIndexHandle(Service)
    else Result := Code = 1060;
  finally
    CloseIndexHandle(Manager);
  end;
end;

function IndexArtifactsGone(): Boolean;
begin
  Result := IndexServiceAbsent() and not RegKeyExists(HKLM64, IndexImageKey) and
    not RegValueExists(HKLM64, IndexOwnerKey, 'OwnerSid') and
    not RegKeyExists(HKLM64, IndexRemovalKey) and
    (GetFileAttributesW(IndexRoot() + '\SDIRIndexService.exe') = $FFFFFFFF) and
    (GetFileAttributesW(IndexRoot() + '\ResumeCache') = $FFFFFFFF);
end;

function ProtectedIndexPath(const Path: String; Directory: Boolean): Boolean;
var
  Attributes: Cardinal;
begin
  Attributes := GetFileAttributesW(Path);
  Result := (Attributes <> $FFFFFFFF) and ((Attributes and $400) = 0) and
    (((Attributes and $10) <> 0) = Directory);
end;

function FindIndexUninstaller(const ProgramHash, DataHash: String; var ProgramName: String): Boolean;
var
  Root, Registered, Quiet, Location, BaseName, DataFile, Image: String;
  I: Integer;
begin
  Result := False;
  Root := IndexRoot();
  if not ProtectedIndexPath(ExpandConstant('{commonpf64}'), True) or not ProtectedIndexPath(Root, True) then Exit;
  if not RegQueryStringValue(HKLM64, IndexImageKey, 'ImagePath', Image) or
    (CompareText(Image, '"' + Root + '\SDIRIndexService.exe"') <> 0) then Exit;
  if not RegQueryStringValue(HKLM64, IndexRemovalKey, 'InstallLocation', Location) or
    (CompareText(Location, Root + '\') <> 0) then Exit;
  if not RegQueryStringValue(HKLM64, IndexRemovalKey, 'UninstallString', Registered) or
    not RegQueryStringValue(HKLM64, IndexRemovalKey, 'QuietUninstallString', Quiet) then Exit;
  if (Length(Registered) <> Length(Root) + 15) or (Registered[1] <> '"') or
    (Registered[Length(Registered)] <> '"') then Exit;
  BaseName := Copy(Registered, Length(Root) + 3, 12);
  if (CompareText(Copy(BaseName, 1, 5), 'unins') <> 0) or
    (CompareText(Copy(BaseName, 9, 4), '.exe') <> 0) then Exit;
  for I := 6 to 8 do
    if Pos(BaseName[I], '0123456789') = 0 then Exit;
  ProgramName := Root + '\' + BaseName;
  if (CompareText(Registered, '"' + ProgramName + '"') <> 0) or
    (CompareText(Quiet, Registered + ' /SILENT') <> 0) then Exit;
  DataFile := ChangeFileExt(ProgramName, '.dat');
  if not ProtectedIndexPath(ProgramName, False) or not ProtectedIndexPath(DataFile, False) then Exit;
  Result := (Length(ProgramHash) = 64) and (Length(DataHash) = 64) and
    (CompareText(GetSHA256OfFile(ProgramName), ProgramHash) = 0) and
    (CompareText(GetSHA256OfFile(DataFile), DataHash) = 0);
end;

procedure RemoveLinkedIndexService();
var
  Pair, Owner, Current, ProgramName: String;
  ExitCode, I: Integer;
begin
  if not RegQueryStringValue(HKCU, IndexLinkKey(), 'IndexRemover', Pair) then Exit;
  if IndexArtifactsGone() then Exit;
  Current := CurrentIndexOwner();
  if Current = '' then RaiseException('현재 Windows 계정을 확인할 수 없어 SDIR 제거를 중단했습니다.');
  if not RegQueryStringValue(HKLM64, IndexOwnerKey, 'OwnerSid', Owner) then
    RaiseException('색인 서비스의 소유자를 확인할 수 없어 SDIR 제거를 중단했습니다.');
  if Owner <> Current then begin
    Log('SDIR_INDEX_REMOVE_DIFFERENT_OWNER');
    Exit;
  end;
  if (Length(Pair) <> 129) or (Pair[65] <> ':') or
    not FindIndexUninstaller(Copy(Pair, 1, 64), Copy(Pair, 66, 64), ProgramName) then
    RaiseException('연결된 색인 서비스의 제거 파일이 달라 SDIR 제거를 중단했습니다. 최신 SDIR 설치 파일로 다시 설치한 뒤 제거하세요.');
  if not ShellExec('runas', ProgramName,
    '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SDIR-EXPECTED-OWNER=' + Current,
    IndexRoot(), SW_HIDE, ewWaitUntilTerminated, ExitCode) then begin
    Log('SDIR_INDEX_REMOVE_LAUNCH_FAILED: ' + IntToStr(ExitCode));
    if ExitCode = 1223 then Abort;
    RaiseException('색인 서비스 제거기를 시작하지 못했습니다. ' + SysErrorMessage(ExitCode));
  end;
  if ExitCode <> 0 then begin
    Log('SDIR_INDEX_REMOVE_EXIT: ' + IntToStr(ExitCode));
    RaiseException('색인 서비스 제거가 완료되지 않아 SDIR 제거를 중단했습니다.');
  end;
  for I := 1 to 120 do begin
    if IndexArtifactsGone() then begin
      Log('SDIR_INDEX_REMOVE_COMPLETE');
      Exit;
    end;
    Sleep(100);
  end;
  RaiseException('색인 서비스와 캐시가 정리됐는지 확인할 수 없어 SDIR 제거를 중단했습니다.');
end;

procedure DeinitializeUninstall;
begin
  ReleaseInstallerMutex;
end;

function InitializeUninstall(): Boolean;
var
  Reason: String;
begin
  InitializePayloadManifest;
  Result := False;
  if not AcquireInstallerMutex(Reason) then begin
    Log('SDIR_UNINSTALL_MUTEX_REFUSED: ' + Reason);
    SuppressibleMsgBox(Reason, mbError, MB_OK, IDOK);
    Exit;
  end;
  if not ValidateRootAndOwnership(ExpandConstant('{app}'), Reason) then begin
    Log('SDIR_UNINSTALL_REFUSED: ' + Reason);
    SuppressibleMsgBox('SDIR 설치 소유권을 확인하지 못해 제거를 중단했습니다.' + #13#10 + Reason,
      mbError, MB_OK, IDOK);
    ReleaseInstallerMutex;
    Exit;
  end;
  if not ProbeExternalLocks(ExpandConstant('{app}'), Reason) then begin
    Log('SDIR_UNINSTALL_REFUSED: ' + Reason);
    SuppressibleMsgBox('실행 중인 SDIR 또는 외부 파일 잠금이 있어 제거를 중단했습니다.' + #13#10 + Reason,
      mbError, MB_OK, IDOK);
    ReleaseInstallerMutex;
    Exit;
  end;
  if CheckForMutexes('Local\SDIR-InstallBundle-' + AppIdText) then begin
    SuppressibleMsgBox('SDIR 설치가 끝난 뒤 제거를 다시 실행하세요.', mbError, MB_OK, IDOK);
    Exit;
  end;
  Log('SDIR_UNINSTALL_PREFLIGHT_OK');
  Result := True;
end;

procedure RemoveOwnedLoginStartup;
var
  Command, Expected: String;
  Key: String;
begin
  Key := 'Software\Microsoft\Windows\CurrentVersion\Run';
  Expected := '"' + ExpandConstant('{app}\SDIR.exe') + '" --login-start';
  if RegQueryStringValue(HKCU, Key, 'SDIR', Command) and (CompareText(Command, Expected) = 0) then
    if not RegDeleteValue(HKCU, Key, 'SDIR') then Log('SDIR_LOGIN_STARTUP_CLEANUP_WARNING');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Reason: String;
begin
  if CurUninstallStep = usUninstall then RemoveLinkedIndexService();
  if CurUninstallStep = usPostUninstall then begin
    RegDeleteValue(HKCU, IndexLinkKey(), 'IndexRemover');
    RegDeleteKeyIfEmpty(HKCU, IndexLinkKey());
    RemoveOwnedLoginStartup;
    FinalizeUninstallRoot(ExpandConstant('{app}'), Reason);
    if Reason <> '' then
      Log('SDIR_UNINSTALL_SHORTCUT_CLEANUP_WARNING: ' + Reason);
  end;
end;
