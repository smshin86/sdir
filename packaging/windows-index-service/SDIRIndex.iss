#define ServiceName "SDIRIndex"
#ifndef PriorServiceSha256
  #define PriorServiceSha256 ""
#endif

[Setup]
AppId=SDIR-Index-Service-BF7D31D4-84CC-482A-8AE5-45C9BEA4100F
AppName=SDIR 색인 서비스
AppVersion=0.1.0
DefaultDirName={commonpf64}\SDIR Index Service
PrivilegesRequired=admin
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
DisableDirPage=yes
DisableProgramGroupPage=yes
Uninstallable=yes
UsePreviousAppDir=no
CloseApplications=no
RestartApplications=no
SetupLogging=yes
OutputBaseFilename=SDIR-windows-x64-index-service-setup

[Files]
Source: "{#ServiceExecutable}"; DestDir: "{app}"; DestName: "SDIRIndexService.exe"; Flags: ignoreversion

[Code]
const
  Name = '{#ServiceName}';
  RegistryPath = 'SOFTWARE\SDIR\IndexService';
  ImageRegistryPath = 'SYSTEM\CurrentControlSet\Services\SDIRIndex';
  PathMissing = $FFFFFFFF;
  FileDirectory = $10;
  FileReparsePoint = $400;
  ServiceAbsent = 1060;
  ServiceDeletePending = 1072;
  ServiceNotActive = 1062;
  StateStopped = 1;

type
  TServiceStatus = record
    ServiceType, State, Controls, ExitCode, SpecificExitCode, CheckPoint, WaitHint: Cardinal;
  end;

var
  OwnerPage: TInputQueryWizardPage;
  OwnerSid, OwnerArgument: String;
  UpgradeDisabled, UpgradingService: Boolean;
  FreshRegistration: Boolean;
  Committed: Boolean;

function GetFileAttributesW(FileName: String): Cardinal;
  external 'GetFileAttributesW@kernel32.dll stdcall';
function OpenSCManagerW(MachineName, DatabaseName: Integer; Access: Cardinal): Integer;
  external 'OpenSCManagerW@advapi32.dll stdcall';
function OpenServiceW(Manager: Integer; ServiceName: String; Access: Cardinal): Integer;
  external 'OpenServiceW@advapi32.dll stdcall';
function CreateServiceW(Manager: Integer; ServiceName, DisplayName: String;
  Access, ServiceType, StartType, ErrorControl: Cardinal; ImagePath: String;
  LoadOrderGroup, TagId, Dependencies, ServiceAccount, Password: Integer): Integer;
  external 'CreateServiceW@advapi32.dll stdcall';
function CloseServiceHandle(Handle: Integer): Boolean;
  external 'CloseServiceHandle@advapi32.dll stdcall';
function QueryServiceStatus(Handle: Integer; var Status: TServiceStatus): Boolean;
  external 'QueryServiceStatus@advapi32.dll stdcall';
function ControlService(Handle: Integer; Control: Cardinal; var Status: TServiceStatus): Boolean;
  external 'ControlService@advapi32.dll stdcall';
function DeleteService(Handle: Integer): Boolean;
  external 'DeleteService@advapi32.dll stdcall';
function ChangeServiceConfigW(Handle: Integer; ServiceType, StartType, ErrorControl: Cardinal;
  Image, Group, Tag, Dependencies, Account, Password, DisplayName: Integer): Boolean;
  external 'ChangeServiceConfigW@advapi32.dll stdcall';
function ConvertStringSecurityDescriptorToSecurityDescriptorW(Text: String; Revision: Cardinal;
  var Data: Integer; Size: Integer): Boolean;
  external 'ConvertStringSecurityDescriptorToSecurityDescriptorW@advapi32.dll stdcall';
function SetServiceObjectSecurity(Handle: Integer; Information: Cardinal; Descriptor: Integer): Boolean;
  external 'SetServiceObjectSecurity@advapi32.dll stdcall';
function LocalFree(Memory: Integer): Integer;
  external 'LocalFree@kernel32.dll stdcall';

function ExpectedRoot(): String;
begin
  Result := ExpandConstant('{commonpf64}\SDIR Index Service');
end;

function ExpectedImage(): String;
begin
  Result := ExpectedRoot() + '\SDIRIndexService.exe';
end;

function IsAccountSid(Value: String): Boolean;
var
  Index, Separators, Digits: Integer;
begin
  Result := False;
  if (Copy(Value, 1, 9) <> 'S-1-5-21-') or (Length(Value) > 54) then Exit;
  Separators := 0;
  Digits := 0;
  for Index := 10 to Length(Value) do begin
    if Value[Index] = '-' then begin
      if Digits = 0 then Exit;
      Separators := Separators + 1;
      Digits := 0;
    end else begin
      if (Value[Index] < '0') or (Value[Index] > '9') then Exit;
      Digits := Digits + 1;
      if Digits > 10 then Exit;
    end;
  end;
  Result := (Separators = 3) and (Digits > 0);
end;

function DefaultOwnerSid(): String;
var
  Code, Start: Integer;
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
  if IsAccountSid(Line) then Result := Line;
end;

procedure InitializeWizard();
begin
  OwnerPage := CreateInputQueryPage(wpWelcome, '색인 서비스 사용 계정',
    '파일 이름과 경로를 검색할 Windows 계정을 선택합니다.',
    '이 SID에는 전체 로컬 NTFS 색인 결과의 파일 이름과 경로를 조회할 권한이 부여됩니다.' + #13#10 + '다른 관리자 계정으로 승인했다면, 실제 SDIR을 사용할 계정에서 whoami /user로 확인한 SID를 입력하세요.');
  OwnerPage.Add('Windows 사용자 SID', False);
  OwnerArgument := ExpandConstant('{param:SDIR-OWNER-SID|}');
  if OwnerArgument <> '' then OwnerPage.Values[0] := OwnerArgument
  else OwnerPage.Values[0] := DefaultOwnerSid();
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := (PageID = OwnerPage.ID) and (OwnerArgument <> '');
end;

function UpgradeBackup(): String;
begin
  Result := ExpectedRoot() + '\SDIRIndexService.upgrade-backup';
end;

function PrepareExistingService(const ExpectedSid: String; var Reason: String): Boolean;
var
  Image, Sid, Account: String;
  Attrs, StartMode, WinType: Cardinal;
  Manager, Service: Integer;
  Status: TServiceStatus;
begin
  Result := False;
  Reason := '기존 색인 서비스가 지원되는 이전 설치와 일치하지 않습니다.';
  if '{#PriorServiceSha256}' = '' then Exit;
  Attrs := GetFileAttributesW(ExpectedImage());
  if (Attrs = PathMissing) or ((Attrs and (FileDirectory or FileReparsePoint)) <> 0) then Exit;
  if GetFileAttributesW(UpgradeBackup()) <> PathMissing then Exit;
  if CompareText(GetSHA256OfFile(ExpectedImage()), '{#PriorServiceSha256}') <> 0 then Exit;
  if not RegQueryStringValue(HKLM64, RegistryPath, 'OwnerSid', Sid) or (Sid <> ExpectedSid) then Exit;
  if not RegQueryStringValue(HKLM64, ImageRegistryPath, 'ImagePath', Image) or
    (CompareText(Image, '"' + ExpectedImage() + '"') <> 0) then Exit;
  if not RegQueryStringValue(HKLM64, ImageRegistryPath, 'ObjectName', Account) or
    (CompareText(Account, 'LocalSystem') <> 0) then Exit;
  if not RegQueryDWordValue(HKLM64, ImageRegistryPath, 'Start', StartMode) or (StartMode <> 3) then Exit;
  if not RegQueryDWordValue(HKLM64, ImageRegistryPath, 'Type', WinType) or (WinType <> 16) then Exit;
  if not RegKeyExists(HKLM64, 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\SDIR-Index-Service-BF7D31D4-84CC-482A-8AE5-45C9BEA4100F_is1') then Exit;
  Reason := '색인 서비스를 갱신하려면 SDIR 창을 정상 종료하세요.';
  Manager := OpenSCManagerW(0, 0, 1);
  if Manager = 0 then Exit;
  try
    Service := OpenServiceW(Manager, Name, 6);
    if Service = 0 then Exit;
    try
      if not QueryServiceStatus(Service, Status) or (Status.State <> StateStopped) then Exit;
      if not ChangeServiceConfigW(Service, $FFFFFFFF, 4, $FFFFFFFF, 0, 0, 0, 0, 0, 0, 0) then Exit;
      UpgradeDisabled := True;
      if not QueryServiceStatus(Service, Status) or (Status.State <> StateStopped) then Exit;
    finally
      CloseServiceHandle(Service);
    end;
  finally
    CloseServiceHandle(Manager);
  end;
  Reason := '기존 색인 서비스 파일을 백업하지 못했습니다.';
  if not FileCopy(ExpectedImage(), UpgradeBackup(), True) then Exit;
  if CompareText(GetSHA256OfFile(UpgradeBackup()), '{#PriorServiceSha256}') <> 0 then Exit;
  UpgradingService := True;
  Result := True;
  Reason := '';
end;

procedure RestoreDemandStart();
var
  Manager, Service: Integer;
begin
  if not UpgradeDisabled then Exit;
  Manager := OpenSCManagerW(0, 0, 1);
  if Manager = 0 then RaiseException('색인 서비스 관리자에 연결할 수 없습니다.');
  try
    Service := OpenServiceW(Manager, Name, 2);
    if Service = 0 then RaiseException('기존 색인 서비스 등록을 열 수 없습니다.');
    try
      if not ChangeServiceConfigW(Service, $FFFFFFFF, 3, $FFFFFFFF, 0, 0, 0, 0, 0, 0, 0) then
        RaiseException('색인 서비스를 요청 시작으로 되돌리지 못했습니다.');
      UpgradeDisabled := False;
    finally
      CloseServiceHandle(Service);
    end;
  finally
    CloseServiceHandle(Manager);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ParentAttrs, RootAttrs: Cardinal;
  Manager, Existing: Integer;
  ServiceError: Cardinal;
begin
  Result := '색인 서비스의 보호된 설치 위치를 확인할 수 없습니다.';
  NeedsRestart := False;
  if CompareText(ExpandConstant('{app}'), ExpectedRoot()) <> 0 then Exit;
  ParentAttrs := GetFileAttributesW(ExpandConstant('{commonpf64}'));
  if (ParentAttrs = PathMissing) or ((ParentAttrs and FileDirectory) = 0) or
      ((ParentAttrs and FileReparsePoint) <> 0) then Exit;
  if OwnerArgument <> '' then OwnerSid := OwnerArgument
  else OwnerSid := Trim(OwnerPage.Values[0]);
  if not IsAccountSid(OwnerSid) then begin
    Result := '서비스를 사용할 Windows 계정의 SID를 확인하세요.';
    Exit;
  end;
  RootAttrs := GetFileAttributesW(ExpectedRoot());
  if RootAttrs <> PathMissing then begin
    if ((RootAttrs and FileDirectory) = 0) or ((RootAttrs and FileReparsePoint) <> 0) then Exit;
    if UpgradingService then begin
      Result := '';
      Exit;
    end;
    if not PrepareExistingService(OwnerSid, Result) then RestoreDemandStart();
    Exit;
  end;
  if RegKeyExists(HKLM64, RegistryPath) then Exit;
  Result := '기존 색인 서비스 등록을 확인할 수 없습니다.';
  Manager := OpenSCManagerW(0, 0, 1);
  if Manager = 0 then Exit;
  try
    Existing := OpenServiceW(Manager, Name, 4);
    ServiceError := DLLGetLastError;
    if Existing <> 0 then begin
      CloseServiceHandle(Existing);
      Exit;
    end;
    if ServiceError <> ServiceAbsent then Exit;
  finally
    CloseServiceHandle(Manager);
  end;
  Result := '';
end;

function VerifyBinary(): Boolean;
var
  Attrs: Cardinal;
begin
  Attrs := GetFileAttributesW(ExpectedImage());
  Result := (Attrs <> PathMissing) and
    ((Attrs and (FileDirectory or FileReparsePoint)) = 0) and
    (CompareText(GetSHA256OfFile(ExpectedImage()), '{#ServiceSha256}') = 0);
end;

function OwnRegistration(): Boolean;
var
  Image, Sid: String;
begin
  Result := RegQueryStringValue(HKLM64, ImageRegistryPath, 'ImagePath', Image) and
    (CompareText(Image, '"' + ExpectedImage() + '"') = 0) and
    RegQueryStringValue(HKLM64, RegistryPath, 'OwnerSid', Sid) and IsAccountSid(Sid) and
    VerifyBinary();
end;

function CacheHexGuid(const Value: String): Boolean;
var
  Index: Integer;
begin
  Result := False;
  if Length(Value) <> 36 then Exit;
  for Index := 1 to 36 do begin
    if (Index = 9) or (Index = 14) or (Index = 19) or (Index = 24) then begin
      if Value[Index] <> '-' then Exit;
    end else if Pos(Value[Index], '0123456789abcdef') = 0 then Exit;
  end;
  Result := True;
end;

function CacheDecimal(const Value: String): Boolean;
var
  Index: Integer;
begin
  Result := False;
  if (Length(Value) = 0) or (Length(Value) > 20) then Exit;
  for Index := 1 to Length(Value) do begin
    if Pos(Value[Index], '0123456789') = 0 then Exit;
  end;
  Result := True;
end;

function CacheFileName(const Value: String): Boolean;
var
  Tail: String;
  Separator: Integer;
begin
  Result := False;
  if (Length(Value) = 41) and (Copy(Value, 37, 5) = '.sidx') then begin
    Result := CacheHexGuid(Copy(Value, 1, 36));
    Exit;
  end;
  if (Length(Value) < 45) or (Copy(Value, 1, 5) <> '.tmp-') or
     (Value[42] <> '-') or not CacheHexGuid(Copy(Value, 6, 36)) then Exit;
  Tail := Copy(Value, 43, Length(Value) - 42);
  Separator := Pos('-', Tail);
  if Separator = 0 then Exit;
  Result := CacheDecimal(Copy(Tail, 1, Separator - 1)) and
    CacheDecimal(Copy(Tail, Separator + 1, Length(Tail) - Separator));
end;

function CheckResumeCache(const Directory: String; Level: Integer; Remove: Boolean): Boolean;
var
  Entry: TFindRec;
  Path: String;
  Attrs: Cardinal;
  IsDir, Allowed: Boolean;
begin
  Result := False;
  Attrs := GetFileAttributesW(Directory);
  if Attrs = PathMissing then begin
    Result := Level = 0;
    Exit;
  end;
  if ((Attrs and FileDirectory) = 0) or ((Attrs and FileReparsePoint) <> 0) then Exit;
  if FindFirst(Directory + '\*', Entry) then begin
    try
      repeat
        if (Entry.Name <> '.') and (Entry.Name <> '..') then begin
          Path := Directory + '\' + Entry.Name;
          Attrs := GetFileAttributesW(Path);
          if (Attrs = PathMissing) or ((Attrs and FileReparsePoint) <> 0) then Exit;
          IsDir := (Attrs and FileDirectory) <> 0;
          Allowed := False;
          case Level of
            0: Allowed := IsDir and IsAccountSid(Entry.Name);
            1: Allowed := IsDir and (Entry.Name = 'index-cache');
            2: Allowed := (IsDir and (Entry.Name = 'v1')) or
                          ((not IsDir) and (Entry.Name = '.profile.lock'));
            3: Allowed := (not IsDir) and CacheFileName(Entry.Name);
          end;
          if not Allowed then Exit;
          if IsDir then begin
            if not CheckResumeCache(Path, Level + 1, Remove) then Exit;
          end else if Remove then begin
            if not DeleteFile(Path) then Exit;
          end;
        end;
      until not FindNext(Entry);
    finally
      FindClose(Entry);
    end;
  end;
  if Remove then Result := RemoveDir(Directory) else Result := True;
end;

procedure RemoveRegistration(CleanCache: Boolean);
var
  Manager, Service, Countdown: Integer;
  Code: Cardinal;
  Status: TServiceStatus;
  DisableSucceeded, DeleteRequested: Boolean;
begin
  DisableSucceeded := False;
  DeleteRequested := False;
  Manager := OpenSCManagerW(0, 0, 1);
  if Manager = 0 then RaiseException('서비스 관리자에 연결할 수 없습니다.');
  try
    Service := OpenServiceW(Manager, Name, $10026);
    if Service = 0 then RaiseException('색인 서비스 등록을 열 수 없습니다.');
    try
      DisableSucceeded := ChangeServiceConfigW(Service, $FFFFFFFF, 4, $FFFFFFFF, 0, 0, 0, 0, 0, 0, 0);
      if not DisableSucceeded then RaiseException('제거 중에 색인 서비스가 다시 시작되는 것을 차단할 수 없습니다.');
      if not QueryServiceStatus(Service, Status) then RaiseException('색인 상태를 읽을 수 없습니다.');
      if Status.State <> StateStopped then begin
        if not ControlService(Service, 1, Status) then begin
          Code := DLLGetLastError;
          if Code <> ServiceNotActive then RaiseException('사용 중인 색인 서비스를 중지할 수 없습니다.');
        end;
        Countdown := 150;
        repeat
          Sleep(100);
          Countdown := Countdown - 1;
          if not QueryServiceStatus(Service, Status) then RaiseException('색인 상태 조회에 실패했습니다.');
        until (Status.State = StateStopped) or (Countdown = 0);
        if Status.State <> StateStopped then RaiseException('색인 종료 시간이 초과됐습니다.');
      end;
      if CleanCache and not CheckResumeCache(ExpectedRoot() + '\ResumeCache', 0, True) then
        RaiseException('색인 캐시를 정리하지 못했습니다. 사용 중인 파일을 닫고 다시 제거하세요.');
      DeleteRequested := DeleteService(Service);
      if not DeleteRequested then RaiseException('색인 서비스 등록을 지울 수 없습니다.');
    finally
      if DisableSucceeded and not DeleteRequested then begin
        if not ChangeServiceConfigW(Service, $FFFFFFFF, 3, $FFFFFFFF, 0, 0, 0, 0, 0, 0, 0) then
          Log('SDIR_INDEX_SERVICE_RECOVERY_FAILED');
      end;
      CloseServiceHandle(Service);
    end;
    Countdown := 100;
    repeat
      Service := OpenServiceW(Manager, Name, 4);
      Code := DLLGetLastError;
      if Service <> 0 then CloseServiceHandle(Service)
      else if Code = ServiceAbsent then begin
        RegDeleteValue(HKLM64, RegistryPath, 'OwnerSid');
        RegDeleteKeyIfEmpty(HKLM64, RegistryPath);
        Exit;
      end else if Code <> ServiceDeletePending then RaiseException('서비스 제거 상태를 확인할 수 없습니다.');
      Sleep(100);
      Countdown := Countdown - 1;
    until Countdown = 0;
    RaiseException('색인 서비스 제거 완료를 확인할 수 없습니다.');
  finally
    CloseServiceHandle(Manager);
  end;
end;

procedure CurStepChanged(Step: TSetupStep);
var
  Manager, Service, Descriptor, Code: Integer;
begin
  if Step <> ssPostInstall then Exit;
  if not VerifyBinary() then RaiseException('설치한 색인 파일이 배포본과 일치하지 않습니다.');
  if UpgradingService then begin
    if not OwnRegistration() then RaiseException('갱신 후 색인 서비스 등록을 확인하지 못했습니다.');
    RestoreDemandStart();
    Committed := True;
    Exit;
  end;
  if not Exec(ExpandConstant('{sys}\icacls.exe'),
    '"' + ExpectedRoot() + '" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F *S-1-5-32-545:(OI)(CI)RX',
    ExpandConstant('{sys}'), SW_HIDE, ewWaitUntilTerminated, Code) then RaiseException(' Program Files 보호 설정을 실행할 수 없습니다.');
  if Code <> 0 then RaiseException('색인 서비스 설치 폴더의 권한을 설정할 수 없습니다.');
  if not Exec(ExpandConstant('{sys}\icacls.exe'),
    '"' + ExpectedRoot() + '" /setowner *S-1-5-32-544 /T',
    ExpandConstant('{sys}'), SW_HIDE, ewWaitUntilTerminated, Code) then RaiseException('보호 폴더 소유권을 설정할 수 없습니다.');
  if Code <> 0 then RaiseException('관리자 그룹 소유권을 설정할 수 없습니다.');
  if not VerifyBinary() then RaiseException('권한 설정 후 색인 파일 검증에 실패했습니다.');
  Manager := OpenSCManagerW(0, 0, 3);
  if Manager = 0 then RaiseException('서비스 관리자에 연결할 수 없습니다.');
  try
    if not RegWriteStringValue(HKLM64, RegistryPath, 'OwnerSid', OwnerSid) then
      RaiseException('승인한 계정을 저장할 수 없습니다.');
    Service := CreateServiceW(Manager, Name, 'SDIR 색인 서비스', $40000,
      16, 3, 1, '"' + ExpectedImage() + '"', 0, 0, 0, 0, 0);
    if Service = 0 then begin
      RegDeleteValue(HKLM64, RegistryPath, 'OwnerSid');
      RegDeleteKeyIfEmpty(HKLM64, RegistryPath);
      RaiseException('색인 서비스를 등록할 수 없습니다.');
    end;
    FreshRegistration := True;
    try
      Descriptor := 0;
      if not ConvertStringSecurityDescriptorToSecurityDescriptorW(
        'D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;LCRP;;;' + OwnerSid + ')', 1, Descriptor, 0) then
        RaiseException('서비스 접근 규칙을 만들 수 없습니다.');
      try
        if not SetServiceObjectSecurity(Service, 4, Descriptor) then
          RaiseException('승인한 계정의 서비스 권한을 설정할 수 없습니다.');
      finally
        LocalFree(Descriptor);
      end;
    finally
      CloseServiceHandle(Service);
    end;
  finally
    CloseServiceHandle(Manager);
  end;
  Committed := True;
end;

procedure DeinitializeSetup();
begin
  if UpgradingService then begin
    if not Committed then begin
      if not FileCopy(UpgradeBackup(), ExpectedImage(), False) or
        (CompareText(GetSHA256OfFile(ExpectedImage()), '{#PriorServiceSha256}') <> 0) then
        RaiseException('기존 색인 서비스 파일을 복구하지 못했습니다. 관리자용 백업을 유지했습니다.');
    end;
    RestoreDemandStart();
    if not DeleteFile(UpgradeBackup()) then Log('SDIR_SERVICE_UPGRADE_BACKUP_RETAINED');
  end else if UpgradeDisabled then RestoreDemandStart();
  if FreshRegistration and not Committed then RemoveRegistration(False);
end;

function OwnUninstallRequest(): Boolean;
var
  ExpectedSid, InstalledSid: String;
begin
  Result := OwnRegistration();
  if not Result then Exit;
  ExpectedSid := ExpandConstant('{param:SDIR-EXPECTED-OWNER|}');
  if ExpectedSid = '' then Exit;
  Result := IsAccountSid(ExpectedSid) and
    RegQueryStringValue(HKLM64, RegistryPath, 'OwnerSid', InstalledSid) and
    (InstalledSid = ExpectedSid);
end;

function InitializeUninstall(): Boolean;
begin
  Result := OwnUninstallRequest();
  if not Result then
    SuppressibleMsgBox('설치 기록과 서비스가 일치하지 않아 제거를 중단했습니다.', mbError, MB_OK, IDOK);
end;

procedure CurUninstallStepChanged(Step: TUninstallStep);
begin
  if Step <> usUninstall then Exit;
  if not OwnUninstallRequest() then RaiseException('제거할 색인 서비스의 소유권이 달라졌습니다.');
  if not CheckResumeCache(ExpectedRoot() + '\ResumeCache', 0, False) then
    RaiseException('색인 캐시에서 알 수 없는 파일이나 연결을 발견해 제거를 중단했습니다.');
  RemoveRegistration(True);
end;
