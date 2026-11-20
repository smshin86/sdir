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

[Tasks]
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
  Log('SDIR_UNINSTALL_PREFLIGHT_OK');
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Reason: String;
begin
  if CurUninstallStep = usPostUninstall then begin
    FinalizeUninstallRoot(ExpandConstant('{app}'), Reason);
    if Reason <> '' then
      Log('SDIR_UNINSTALL_SHORTCUT_CLEANUP_WARNING: ' + Reason);
  end;
end;
