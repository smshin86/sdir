const
  ReceiptFileName = 'sdir.install-receipt';
  ReceiptHeader = 'SDIR-RECEIPT-V2';
  StartMenuShortcutName = '{#AppNameValue}.lnk';
  DesktopShortcutName = '{#AppNameValue}.lnk';
  QuarantineLeaf = '.sdir-installer-rollback-{#AppIdValue}';
  UninstallLogHeaderSize = 448;
  UninstallLogHeaderVersion = 1054;
  UninstallLogFlagAdmin = $1;
  UninstallLogFlagPowerUser = $40;
  UninstallLogFlagAdminMode = $80;
  UninstallLogFlagWin64 = $20;
  UninstallLogFlagRedirectionGuard = $2000;
  RuntimeGuid = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
#ifdef TEST_BUILD
  TestLauncherFileName = 'sdir-installer-test-launcher.cmd';
#endif
  MaxMovedFiles = 80;
  MaxDirectorySnapshots = 8;
  MaxRegistryValueBytes = 1048576;
  InvalidFileAttributes = $FFFFFFFF;
  InvalidHandleValue = -1;
  FileAttributeDirectory = $10;
  FileAttributeReparsePoint = $400;
  FileFlagBackupSemantics = $02000000;
  FileReadAttributes = $80;
  FileWriteAttributes = $100;
  GenericRead = $80000000;
  OpenExisting = 3;
  ShareRead = 1;
  ShareWrite = 2;
  ShareDelete = 4;
  ProcessSnapshotFlag = $2;
  ProcessQueryLimitedInformation = $1000;
  CreateNew = 1;
  GenericWrite = $40000000;
  WaitObject0 = 0;
  WaitAbandoned = $80;
  WaitTimeout = $102;
  ErrorFileNotFound = 2;
  ErrorMoreData = 234;
  ErrorNoMoreItems = 259;
  KeyQueryValue = $1;
  KeySetValue = $2;
  KeyWow64_64Key = $100;
  RegistryTypeNone = 0;
  RegistryTypeString = 1;
  RegistryTypeExpandString = 2;
  RegistryTypeBinary = 3;
  RegistryTypeDWord = 4;
  RegistryTypeDWordBigEndian = 5;
  RegistryTypeLink = 6;
  RegistryTypeMultiString = 7;
  RegistryTypeQWord = 11;
  ProcessEntry32ExpectedSize = 556;

type
  TProcessEntry32 = record
    dwSize: Cardinal;
    cntUsage: Cardinal;
    th32ProcessID: Cardinal;
    th32DefaultHeapID: Cardinal;
    th32ModuleID: Cardinal;
    cntThreads: Cardinal;
    th32ParentProcessID: Cardinal;
    pcPriClassBase: Integer;
    dwFlags: Cardinal;
    szExeFile: array[0..259] of WideChar;
  end;
  TInt64Array = array of Int64;
  TRegistryStringArray = TArrayOfString;

var
  TransactionStarted: Boolean;
  NativeInstallCommitted: Boolean;
  RollbackCompleted: Boolean;
  ReceiptAlreadyWritten: Boolean;
  CreatedReceipt: Boolean;
  CreatedReceiptTemp: Boolean;
  CreatedStartMenuShortcut: Boolean;
  CreatedDesktopShortcut: Boolean;
  RegistrySnapshotCaptured: Boolean;
  RegistryKeyExisted: Boolean;
  RegistryValueNames: TRegistryStringArray;
  RegistryValueTypes: TRegistryStringArray;
  RegistryValueByteLengths: TRegistryStringArray;
  RegistryValueData: TRegistryStringArray;
  PayloadPaths, PayloadHashes: TArrayOfString;
  AcceptedInventoryIds, AcceptedInventoryPaths, AcceptedInventoryHashes: TArrayOfString;
  PayloadBytes, AcceptedInventoryBytes: TInt64Array;
  ReceiptPayloadSeen: array[0..127] of Boolean;
  ReceiptPayloadHashes: array[0..127] of String;
  ReceiptPayloadRecordCount: Integer;
  PayloadFilesInstalled: Integer;
  ReceiptInventoryId: String;
  ReceiptStartMenuHash: String;
  ReceiptDesktopHash: String;
  ReceiptDesktopEnabled: Boolean;
  NewReceiptHash: String;
  NewReceiptTempHash: String;
  NewStartMenuHash: String;
  NewDesktopHash: String;
  MovedCount: Integer;
  MovedOriginalPaths: array[0..79] of String;
  MovedBackupPaths: array[0..79] of String;
  MovedExpectedHashes: array[0..79] of String;
  MovedExpectedBytes: array[0..79] of Int64;
  MovedVerified: array[0..79] of Boolean;
  MovedRestored: array[0..79] of Boolean;
  QuarantineDirCount: Integer;
  QuarantineDirs: array[0..2] of String;
  DirectorySnapshotCount: Integer;
  DirectorySnapshotPaths: array[0..7] of String;
  DirectorySnapshotExists: array[0..7] of Boolean;
  DirectoryCreatedByRun: array[0..7] of Boolean;
  DirectorySnapshotAttributes: array[0..7] of Cardinal;
  DirectorySnapshotCreation: array[0..7] of TFileTime;
  DirectorySnapshotAccess: array[0..7] of TFileTime;
  DirectorySnapshotWrite: array[0..7] of TFileTime;
  InstallerMutexHandle: Integer;
  InstallerMutexOwned: Boolean;

function GetFileAttributesW(FileName: String): Cardinal;
  external 'GetFileAttributesW@kernel32.dll stdcall';
function SetFileAttributesW(FileName: String; Attributes: Cardinal): Boolean;
  external 'SetFileAttributesW@kernel32.dll stdcall';
function MoveFileW(ExistingFileName, NewFileName: String): Boolean;
  external 'MoveFileW@kernel32.dll stdcall';
function CreateFileW(FileName: String; DesiredAccess, ShareMode: Cardinal;
  SecurityAttributes: Integer; CreationDisposition, FlagsAndAttributes: Cardinal;
  TemplateFile: Integer): Integer;
  external 'CreateFileW@kernel32.dll stdcall';
function CloseHandle(Handle: Integer): Boolean;
  external 'CloseHandle@kernel32.dll stdcall';
function CreateMutexW(SecurityAttributes: Integer; InitialOwner: Boolean;
  Name: String): Integer;
  external 'CreateMutexW@kernel32.dll stdcall';
function WaitForSingleObject(Handle: Integer; Milliseconds: Cardinal): Cardinal;
  external 'WaitForSingleObject@kernel32.dll stdcall';
function ReleaseMutex(Handle: Integer): Boolean;
  external 'ReleaseMutex@kernel32.dll stdcall';
function WriteFile(Handle: Integer; Buffer: String; BytesToWrite: Cardinal;
  var BytesWritten: Cardinal; Overlapped: Integer): Boolean;
  external 'WriteFile@kernel32.dll stdcall';
function FlushFileBuffers(Handle: Integer): Boolean;
  external 'FlushFileBuffers@kernel32.dll stdcall';
function RegOpenKeyExW(Key: Integer; SubKey: String; Options, DesiredAccess: Cardinal;
  var ResultKey: Integer): Cardinal;
  external 'RegOpenKeyExW@advapi32.dll stdcall';
function RegCreateKeyExW(Key: Integer; SubKey: String; Reserved: Cardinal;
  ClassName: String; Options, DesiredAccess: Cardinal; SecurityAttributes: Integer;
  var ResultKey: Integer; var Disposition: Cardinal): Cardinal;
  external 'RegCreateKeyExW@advapi32.dll stdcall';
function RegCloseKey(Key: Integer): Cardinal;
  external 'RegCloseKey@advapi32.dll stdcall';
function RegQueryValueExW(Key: Integer; ValueName: String; Reserved: Integer;
  var ValueType: Cardinal; Data: String; var DataSize: Cardinal): Cardinal;
  external 'RegQueryValueExW@advapi32.dll stdcall';
function RegSetValueExW(Key: Integer; ValueName: String; Reserved: Cardinal;
  ValueType: Cardinal; Data: String; DataSize: Cardinal): Cardinal;
  external 'RegSetValueExW@advapi32.dll stdcall';
function RegDeleteKeyExW(Key: Integer; SubKey: String; SamDesired,
  Reserved: Cardinal): Cardinal;
  external 'RegDeleteKeyExW@advapi32.dll stdcall';
function GetFileTime(Handle: Integer; var CreationTime, LastAccessTime,
  LastWriteTime: TFileTime): Boolean;
  external 'GetFileTime@kernel32.dll stdcall';
function SetFileTime(Handle: Integer; var CreationTime, LastAccessTime,
  LastWriteTime: TFileTime): Boolean;
  external 'SetFileTime@kernel32.dll stdcall';
function CreateToolhelp32Snapshot(Flags, ProcessId: Cardinal): Integer;
  external 'CreateToolhelp32Snapshot@kernel32.dll stdcall';
function Process32FirstW(Snapshot: Integer; var Entry: TProcessEntry32): Boolean;
  external 'Process32FirstW@kernel32.dll stdcall';
function Process32NextW(Snapshot: Integer; var Entry: TProcessEntry32): Boolean;
  external 'Process32NextW@kernel32.dll stdcall';
function OpenProcess(DesiredAccess: Cardinal; InheritHandle: Boolean;
  ProcessId: Cardinal): Integer;
  external 'OpenProcess@kernel32.dll stdcall';
function QueryFullProcessImageNameW(Process: Integer; Flags: Cardinal;
  ImageName: String; var Size: Cardinal): Boolean;
  external 'QueryFullProcessImageNameW@kernel32.dll stdcall';
function GetCurrentProcessId: Cardinal;
  external 'GetCurrentProcessId@kernel32.dll stdcall';

procedure InitializePayloadManifest;
begin
  SetArrayLength(PayloadPaths, PayloadCount);
  SetArrayLength(PayloadHashes, PayloadCount);
  SetArrayLength(PayloadBytes, PayloadCount);
  SetArrayLength(AcceptedInventoryIds, InventoryRowCount);
  SetArrayLength(AcceptedInventoryPaths, InventoryRowCount);
  SetArrayLength(AcceptedInventoryHashes, InventoryRowCount);
  SetArrayLength(AcceptedInventoryBytes, InventoryRowCount);
#include "payload-initialize.iss"
end;

function RuntimeVersionIsValid(const Value: String): Boolean;
var
  Remainder, Part: String;
  Dot, Index, Number, NonZeroCount, DigitIndex: Integer;
  Ch: Char;
begin
  Result := False;
  Remainder := Value;
  NonZeroCount := 0;
  for Index := 0 to 3 do begin
    Dot := Pos('.', Remainder);
    if Index < 3 then begin
      if Dot = 0 then Exit;
      Part := Copy(Remainder, 1, Dot - 1);
      Delete(Remainder, 1, Dot);
    end else begin
      if Dot <> 0 then Exit;
      Part := Remainder;
    end;
    if Part = '' then Exit;
    for DigitIndex := 1 to Length(Part) do begin
      Ch := Part[DigitIndex];
      if (Ch < '0') or (Ch > '9') then Exit;
    end;
    if (StrToIntDef(Part, -1) < 0) or
       (StrToIntDef(Part, -1) > 65535) then Exit;
    Number := StrToIntDef(Part, -1);
    if Number > 0 then Inc(NonZeroCount);
  end;
  Result := NonZeroCount > 0;
end;

function RuntimeAtRoot(const Root: Integer; const KeyPath: String): Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(Root, KeyPath, 'pv', Version) and
    RuntimeVersionIsValid(Version);
end;

function InstallerMutexName: String;
var
  LocalAppDataPath: String;
  Index: Integer;
  Ch: Char;
begin
  LocalAppDataPath := ExpandConstant('{localappdata}');
  Result := 'Global\SDIR-Installer-' + AppIdText + '-';
  for Index := 1 to Length(LocalAppDataPath) do begin
    Ch := LocalAppDataPath[Index];
    if (Ch = '\') or (Ch = '/') or (Ch = ':') then
      Result := Result + '_'
    else
      Result := Result + Ch;
  end;
end;

function AcquireInstallerMutex(var Reason: String): Boolean;
var
  WaitResult: Cardinal;
begin
  Result := False;
  Reason := '';
  if InstallerMutexOwned then begin
    Result := True;
    Exit;
  end;
  InstallerMutexHandle := CreateMutexW(0, False, InstallerMutexName);
  if InstallerMutexHandle = 0 then begin
    Reason := 'Could not create the per-user installer transaction mutex.';
    Exit;
  end;
  WaitResult := WaitForSingleObject(InstallerMutexHandle, 0);
  if (WaitResult = WaitObject0) or (WaitResult = WaitAbandoned) then begin
    InstallerMutexOwned := True;
    Result := True;
    Exit;
  end;
  CloseHandle(InstallerMutexHandle);
  InstallerMutexHandle := 0;
  if WaitResult = WaitTimeout then
    Reason := 'Another SDIR setup or uninstall operation is already running.'
  else
    Reason := 'Could not acquire the per-user installer transaction mutex.';
end;

procedure ReleaseInstallerMutex;
begin
  if InstallerMutexHandle <> 0 then begin
    if InstallerMutexOwned then ReleaseMutex(InstallerMutexHandle);
    CloseHandle(InstallerMutexHandle);
  end;
  InstallerMutexHandle := 0;
  InstallerMutexOwned := False;
end;

function HasWebViewRuntime: Boolean;
var
  RuntimeKey: String;
begin
#ifdef TEST_MISSING_RUNTIME
  Result := False;
#else
  RuntimeKey := 'Software\Microsoft\EdgeUpdate\Clients\' + RuntimeGuid;
  Result := RuntimeAtRoot(HKCU32, RuntimeKey) or
    RuntimeAtRoot(HKLM32, RuntimeKey);
  if not Result and IsWin64 then begin
    Result := RuntimeAtRoot(HKCU64, RuntimeKey) or
      RuntimeAtRoot(HKLM64, RuntimeKey);
  end;
#endif
end;

function TrimTrailingBackslash(const Value: String): String;
begin
  Result := Value;
  while (Length(Result) > 3) and (Result[Length(Result)] = '\') do
    Delete(Result, Length(Result), 1);
end;

function ExpectedInstallRoot: String;
begin
  Result := AddBackslash(ExpandConstant('{localappdata}\Programs')) +
    '{#RootNameValue}';
  Result := TrimTrailingBackslash(Result);
end;

function NormalizeWindowsPath(const Value: String): String;
begin
  Result := TrimTrailingBackslash(Value);
  if Copy(Result, 1, 4) = '\\?\' then
    Delete(Result, 1, 4);
end;

function NativePayloadPath(const Value: String): String;
begin
  Result := Value;
  StringChange(Result, '/', '\');
end;

function HasForbiddenCommandLineOverride: Boolean;
var
  Tail: String;
begin
  Tail := Uppercase(GetCmdTail);
  Result :=
    (Pos('/DIR=', Tail) > 0) or
    (Pos('/CLOSEAPPLICATIONS', Tail) > 0) or
    (Pos('/RESTARTAPPLICATIONS', Tail) > 0) or
    (Pos('/RESTART=', Tail) > 0) or
    (Pos('/RESTARTEXITCODE', Tail) > 0);
end;

function HasReparseComponent(const Path: String; var Reason: String): Boolean;
var
  CurrentPath, ParentPath: String;
  Attributes: Cardinal;
begin
  Result := True;
  CurrentPath := TrimTrailingBackslash(Path);
  while CurrentPath <> '' do begin
    Attributes := GetFileAttributesW(CurrentPath);
    if (Attributes <> InvalidFileAttributes) and
       ((Attributes and FileAttributeReparsePoint) <> 0) then begin
      Reason := 'Reparse point found in owned path: ' + CurrentPath;
      Exit;
    end;
    ParentPath := ExtractFileDir(CurrentPath);
    if (ParentPath = CurrentPath) or (ParentPath = '') then Break;
    CurrentPath := ParentPath;
  end;
  Result := False;
end;

function GetUninstallRegistryKey: String;
begin
  Result := 'Software\Microsoft\Windows\CurrentVersion\Uninstall\' +
    AppIdText + '_is1';
end;

function ReceiptPath(const Root: String): String;
begin
  Result := AddBackslash(Root) + ReceiptFileName;
end;

function StartMenuDirectory: String;
begin
  Result := AddBackslash(ExpandConstant('{userprograms}')) + GroupNameText;
end;

function StartMenuShortcutPath: String;
begin
  Result := AddBackslash(StartMenuDirectory) + StartMenuShortcutName;
end;

function DesktopShortcutPath: String;
begin
  Result := AddBackslash(ExpandConstant('{userdesktop}')) + DesktopShortcutName;
end;

function PayloadIndex(const RelativePath: String): Integer;
var
  Index: Integer;
begin
  Result := -1;
  for Index := 0 to PayloadCount - 1 do begin
    if CompareText(PayloadPaths[Index], RelativePath) = 0 then begin
      Result := Index;
      Exit;
    end;
  end;
end;

function InventoryRowIndex(const InventoryId, RelativePath: String): Integer;
var
  Index: Integer;
begin
  Result := -1;
  for Index := 0 to InventoryRowCount - 1 do begin
    if (AcceptedInventoryIds[Index] = InventoryId) and
       (AcceptedInventoryPaths[Index] = RelativePath) then begin
      Result := Index;
      Exit;
    end;
  end;
end;

function InventoryRowsForId(const InventoryId: String): Integer;
var
  Index: Integer;
begin
  Result := 0;
  for Index := 0 to InventoryRowCount - 1 do
    if AcceptedInventoryIds[Index] = InventoryId then
      Inc(Result);
end;

function AcceptedPathIndex(const RelativePath: String): Integer;
var
  Index: Integer;
begin
  Result := -1;
  for Index := 0 to InventoryRowCount - 1 do begin
    if CompareText(AcceptedInventoryPaths[Index], RelativePath) = 0 then begin
      Result := Index;
      Exit;
    end;
  end;
end;

function OwnedFilePath(const Root, RelativePath: String): String;
begin
  if AcceptedPathIndex(RelativePath) >= 0 then
    Result := AddBackslash(Root) + NativePayloadPath(RelativePath)
  else
    Result := '';
end;

function ValidSha256(const Value: String): Boolean;
var
  Index: Integer;
  Ch: Char;
  UpperValue: String;
begin
  Result := False;
  if Length(Value) <> 64 then Exit;
  UpperValue := Uppercase(Value);
  for Index := 1 to Length(Value) do begin
    Ch := UpperValue[Index];
    if not (((Ch >= '0') and (Ch <= '9')) or
      ((Ch >= 'A') and (Ch <= 'F'))) then Exit;
  end;
  Result := True;
end;

function IsUninstallerMetadata(const RelativePath: String): Boolean;
begin
  Result := (CompareText(RelativePath, 'unins000.exe') = 0) or
    (CompareText(RelativePath, 'unins000.dat') = 0);
end;

function IsRegularNonReparseFile(const Path: String): Boolean;
var
  Attributes: Cardinal;
  ReparseReason: String;
begin
  Result := False;
  Attributes := GetFileAttributesW(Path);
  if (Attributes = InvalidFileAttributes) or
     ((Attributes and FileAttributeDirectory) <> 0) or
     ((Attributes and FileAttributeReparsePoint) <> 0) then Exit;
  if HasReparseComponent(Path, ReparseReason) then Exit;
  Result := True;
end;

function FileSizeMatches(const Path: String; ExpectedBytes: Int64): Boolean;
var
  ActualBytes: Int64;
begin
  Result := False;
  if not FileSize64(Path, ActualBytes) then Exit;
  Result := ActualBytes = ExpectedBytes;
end;

function SavedFileMatches(const Path, ExpectedHash: String;
  ExpectedBytes: Int64): Boolean;
begin
  Result := False;
  if not IsRegularNonReparseFile(Path) then Exit;
  if not FileSizeMatches(Path, ExpectedBytes) then Exit;
  if CompareText(GetSHA256OfFile(Path), ExpectedHash) <> 0 then Exit;
  Result := True;
end;

function ReadUInt32LE(const Buffer: AnsiString; Offset: Integer): Cardinal;
begin
  Result := Cardinal(Ord(Buffer[Offset + 1])) or
    (Cardinal(Ord(Buffer[Offset + 2])) shl 8) or
    (Cardinal(Ord(Buffer[Offset + 3])) shl 16) or
    (Cardinal(Ord(Buffer[Offset + 4])) shl 24);
end;

function HeaderFieldMatches(const Buffer: AnsiString; Offset, FieldLength: Integer;
  const Expected: String): Boolean;
var
  Index: Integer;
begin
  Result := False;
  if Length(Expected) > FieldLength then Exit;
  for Index := 0 to FieldLength - 1 do begin
    if Index < Length(Expected) then begin
      if Buffer[Offset + Index + 1] <> Expected[Index + 1] then Exit;
    end else if Buffer[Offset + Index + 1] <> #0 then Exit;
  end;
  Result := True;
end;

function ComputeCrc32(const Buffer: AnsiString; ByteCount: Integer): Cardinal;
var
  Index, BitIndex: Integer;
begin
  Result := $FFFFFFFF;
  for Index := 1 to ByteCount do begin
    Result := Result xor Cardinal(Ord(Buffer[Index]));
    for BitIndex := 0 to 7 do begin
      if (Result and 1) <> 0 then
        Result := (Result shr 1) xor $EDB88320
      else
        Result := Result shr 1;
    end;
  end;
  Result := Result xor $FFFFFFFF;
end;

function ValidateUninstallerExecutableVersion(const Root: String;
  var Reason: String): Boolean;
var
  VersionMS, VersionLS: Cardinal;
  Path: String;
begin
  Result := False;
  Path := AddBackslash(Root) + 'unins000.exe';
  if not IsRegularNonReparseFile(Path) or
     not GetVersionNumbers(Path, VersionMS, VersionLS) then begin
    Reason := 'The fixed native uninstaller executable is missing or has no readable version resource.';
    Exit;
  end;
  if ((VersionMS shr 16) <> 51) or ((VersionMS and $FFFF) <> 1054) or
     (VersionLS <> 0) then begin
    Reason := 'The native uninstaller executable is not Inno Setup engine version 51.1054.0.0.';
    Exit;
  end;
  Result := True;
end;

function ValidateUninstallLogHeader(const Root: String;
  var Reason: String): Boolean;
var
  Path: String;
  Stream: TFileStream;
  Header: AnsiString;
  Version, RecordCount, Flags, StoredCrc: Cardinal;
  FileBytes: Int64;
  EndOffset: Int64;
begin
  Result := False;
  Path := AddBackslash(Root) + 'unins000.dat';
  if not IsRegularNonReparseFile(Path) then begin
    Reason := 'The fixed native uninstall log is missing, truncated, or not a regular file.';
    Exit;
  end;
  if not FileSize64(Path, FileBytes) then begin
    Reason := 'The fixed native uninstall log is missing, truncated, or not a regular file.';
    Exit;
  end;
  if FileBytes < UninstallLogHeaderSize then begin
    Reason := 'The fixed native uninstall log is missing, truncated, or not a regular file.';
    Exit;
  end;
  try
    Stream := TFileStream.Create(Path, fmOpenRead or fmShareDenyNone);
  except
    Reason := 'The native uninstall log header could not be opened for setup preflight.';
    Exit;
  end;
  try
    try
      SetLength(Header, UninstallLogHeaderSize);
      Stream.ReadBuffer(Header, UninstallLogHeaderSize);
      Version := ReadUInt32LE(Header, 320);
      RecordCount := ReadUInt32LE(Header, 324);
      EndOffset := Int64(ReadUInt32LE(Header, 328));
      Flags := ReadUInt32LE(Header, 332);
      StoredCrc := ReadUInt32LE(Header, 444);
      if not HeaderFieldMatches(Header, 0, 64,
        'Inno Setup Uninstall Log (b) 64-bit') or
         not HeaderFieldMatches(Header, 64, 128, AppIdText) or
         not HeaderFieldMatches(Header, 192, 128, AppNameText) then begin
        Reason := 'The native uninstall log header identity does not match this installation.';
        Exit;
      end;
      if (Version <> UninstallLogHeaderVersion) or (RecordCount = 0) or
         not FileSizeMatches(Path, EndOffset) then begin
        Reason := 'The native uninstall log header version, record count, or end offset is invalid.';
        Exit;
      end;
      { Inno 6.7.4 sets Win64 and may set RedirectionGuard on supported Windows.
        All privilege, CRC-suppression, obsolete, and unknown flags remain rejected. }
      if (Flags and (UninstallLogFlagAdmin or UninstallLogFlagPowerUser or
        UninstallLogFlagAdminMode)) <> 0 then begin
        Reason := 'The native uninstall log records an admin or power-user installation.';
        Exit;
      end;
      if (Flags and UninstallLogFlagWin64) = 0 then begin
        Reason := 'The native uninstall log does not record a 64-bit installation.';
        Exit;
      end;
      if (Flags <> UninstallLogFlagWin64) and
         (Flags <> (UninstallLogFlagWin64 or UninstallLogFlagRedirectionGuard)) then begin
        Reason := 'The native uninstall log contains unsupported or CRC-suppression flags.';
        Exit;
      end;
      if StoredCrc = $11111111 then begin
        Reason := 'The native uninstall log uses the debug CRC bypass sentinel.';
        Exit;
      end;
      if StoredCrc <> ComputeCrc32(Header, 444) then begin
        Reason := 'The native uninstall log header CRC is invalid.';
        Exit;
      end;
    except
      Reason := 'The native uninstall log header could not be read or decoded safely.';
      Exit;
    end;
  finally
    Stream.Free;
  end;
  Result := True;
end;

function ValidateRegisteredNativeMetadata(const Root: String;
  var Reason: String): Boolean;
var
  RegistryName, InstallLocation, AppPath, SetupVersion, UninstallString: String;
  QuietUninstallString, DisplayName, DisplayVersion, DisplayIcon: String;
  ExpectedExe, ExpectedUninstall, ExpectedQuiet, ExpectedIcon: String;
  CommaIndex: Integer;
begin
  Result := False;
  Reason := '';
  RegistryName := GetUninstallRegistryKey;
  ExpectedExe := AddBackslash(Root) + 'unins000.exe';
  ExpectedUninstall := #34 + ExpectedExe + #34;
  ExpectedQuiet := ExpectedUninstall + ' /SILENT';
  ExpectedIcon := AddBackslash(Root) + 'SDIR.exe';
  if not RegQueryStringValue(HKEY_CURRENT_USER, RegistryName, 'InstallLocation',
       InstallLocation) or
     not RegQueryStringValue(HKEY_CURRENT_USER, RegistryName,
       'Inno Setup: App Path', AppPath) or
     not RegQueryStringValue(HKEY_CURRENT_USER, RegistryName,
       'Inno Setup: Setup Version', SetupVersion) or
     not RegQueryStringValue(HKEY_CURRENT_USER, RegistryName,
       'UninstallString', UninstallString) or
     not RegQueryStringValue(HKEY_CURRENT_USER, RegistryName,
       'QuietUninstallString', QuietUninstallString) or
     not RegQueryStringValue(HKEY_CURRENT_USER, RegistryName,
       'DisplayName', DisplayName) or
     not RegQueryStringValue(HKEY_CURRENT_USER, RegistryName,
       'DisplayVersion', DisplayVersion) or
     not RegQueryStringValue(HKEY_CURRENT_USER, RegistryName,
       'DisplayIcon', DisplayIcon) then begin
    Reason := 'The fixed native uninstall registration is incomplete.';
    Exit;
  end;
  if (CompareText(NormalizeWindowsPath(InstallLocation),
        NormalizeWindowsPath(Root)) <> 0) or
     (CompareText(NormalizeWindowsPath(AppPath), NormalizeWindowsPath(Root)) <> 0) or
     (SetupVersion <> '6.7.4') or
     (UninstallString <> ExpectedUninstall) or
     (QuietUninstallString <> ExpectedQuiet) or
     (DisplayName <> AppNameText) or (DisplayVersion <> '0.1') then begin
    Reason := 'The native uninstall registration does not match the fixed root, identity, or Inno 6.7.4 commands.';
    Exit;
  end;
  CommaIndex := Pos(',', DisplayIcon);
  if CommaIndex > 0 then begin
    if Copy(DisplayIcon, CommaIndex, Length(DisplayIcon)) <> ',0' then begin
      Reason := 'The native uninstall display icon does not identify the SDIR executable.';
      Exit;
    end;
    Delete(DisplayIcon, CommaIndex, Length(DisplayIcon));
  end;
  if (Length(DisplayIcon) >= 2) and (DisplayIcon[1] = #34) and
     (DisplayIcon[Length(DisplayIcon)] = #34) then begin
    Delete(DisplayIcon, Length(DisplayIcon), 1);
    Delete(DisplayIcon, 1, 1);
  end;
  if CompareText(NormalizeWindowsPath(DisplayIcon),
       NormalizeWindowsPath(ExpectedIcon)) <> 0 then begin
    Reason := 'The native uninstall display icon does not identify the SDIR executable.';
    Exit;
  end;
  if not IsRegularNonReparseFile(ExpectedExe) or
     not IsRegularNonReparseFile(AddBackslash(Root) + 'unins000.dat') then begin
    Reason := 'The native uninstall executable and data file must both be present as regular files.';
    Exit;
  end;
  if not ValidateUninstallerExecutableVersion(Root, Reason) then Exit;
  if not IsUninstaller and not ValidateUninstallLogHeader(Root, Reason) then Exit;
  Result := True;
end;

function ReadReceipt(const Root: String; var Reason: String): Boolean;
var
  Lines: TArrayOfString;
  Line, Kind, Remainder, Field1, Field2, Field3, Path, Hash, FullPath: String;
  FirstSeparator, SecondSeparator: Integer;
  Index, InventoryIndexValue, PayloadRecords, SdirIndex: Integer;
  AppIdSeen, DesktopSeen, InventorySeen, StartMenuSeen: Boolean;
  DesktopValue: String;
begin
  Result := False;
  Reason := '';
  for Index := 0 to InventoryRowCount - 1 do begin
    ReceiptPayloadSeen[Index] := False;
    ReceiptPayloadHashes[Index] := '';
  end;
  ReceiptPayloadRecordCount := 0;
  ReceiptInventoryId := '';
  ReceiptStartMenuHash := '';
  ReceiptDesktopHash := '';
  ReceiptDesktopEnabled := False;
  AppIdSeen := False;
  DesktopSeen := False;
  InventorySeen := False;
  StartMenuSeen := False;
  PayloadRecords := 0;

  if not LoadStringsFromFile(ReceiptPath(Root), Lines) then begin
    Reason := 'Install receipt is missing or unreadable.';
    Exit;
  end;
  if (Length(Lines) = 0) or (Lines[0] <> ReceiptHeader) then begin
    Reason := 'Install receipt header is invalid.';
    Exit;
  end;

  for Index := 1 to Length(Lines) - 1 do begin
    Line := Lines[Index];
    FirstSeparator := Pos('|', Line);
    if FirstSeparator = 0 then begin
      Reason := 'Install receipt contains an invalid record.';
      Exit;
    end;
    Kind := Copy(Line, 1, FirstSeparator - 1);
    Remainder := Copy(Line, FirstSeparator + 1, Length(Line));
    if CompareText(Kind, 'AppId') = 0 then begin
      if AppIdSeen or (Remainder <> AppIdText) then begin
        Reason := 'Install receipt application identity is invalid.';
        Exit;
      end;
      AppIdSeen := True;
    end else if CompareText(Kind, 'Desktop') = 0 then begin
      if DesktopSeen or ((Remainder <> '0') and (Remainder <> '1')) then begin
        Reason := 'Install receipt desktop selection is invalid.';
        Exit;
      end;
      DesktopSeen := True;
      DesktopValue := Remainder;
      ReceiptDesktopEnabled := DesktopValue = '1';
    end else if CompareText(Kind, 'Inventory') = 0 then begin
      if InventorySeen or (InventoryRowsForId(Remainder) = 0) then begin
        Reason := 'Install receipt inventory selector is missing, duplicated, or unsupported.';
        Exit;
      end;
      InventorySeen := True;
      ReceiptInventoryId := Remainder;
    end else if CompareText(Kind, 'Payload') = 0 then begin
      SecondSeparator := Pos('|', Remainder);
      if SecondSeparator = 0 then begin
        Reason := 'Install receipt payload record is malformed.';
        Exit;
      end;
      Path := Copy(Remainder, 1, SecondSeparator - 1);
      Hash := Copy(Remainder, SecondSeparator + 1, Length(Remainder));
      if not InventorySeen then begin
        Reason := 'Install receipt payload record appears before its inventory selector.';
        Exit;
      end;
      if not ValidSha256(Hash) then begin
        Reason := 'Install receipt contains an invalid payload hash.';
        Exit;
      end;
      InventoryIndexValue := InventoryRowIndex(ReceiptInventoryId, Path);
      if InventoryIndexValue < 0 then begin
        Reason := 'Install receipt names a path outside the selected compiled inventory.';
        Exit;
      end;
      if ReceiptPayloadSeen[InventoryIndexValue] then begin
        Reason := 'Install receipt contains a duplicate payload path.';
        Exit;
      end;
      if CompareText(Hash, AcceptedInventoryHashes[InventoryIndexValue]) <> 0 then begin
        Reason := 'Install receipt payload hash differs from the selected compiled inventory.';
        Exit;
      end;
      ReceiptPayloadSeen[InventoryIndexValue] := True;
      ReceiptPayloadHashes[InventoryIndexValue] := Hash;
      FullPath := OwnedFilePath(Root, Path);
      if (FullPath = '') or HasReparseComponent(FullPath, Reason) then begin
        if Reason = '' then Reason := 'Receipt path is not a compiled owned file.';
        Exit;
      end;
      if FileExists(FullPath) then begin
        if DirExists(FullPath) or
           not FileSizeMatches(FullPath, AcceptedInventoryBytes[InventoryIndexValue]) or
           (CompareText(GetSHA256OfFile(FullPath), AcceptedInventoryHashes[InventoryIndexValue]) <> 0) then begin
          Reason := 'An owned file size or hash differs from the selected compiled inventory: ' + Path;
          Exit;
        end;
      end;
      Inc(PayloadRecords);
      Inc(ReceiptPayloadRecordCount);
    end else if CompareText(Kind, 'Shortcut') = 0 then begin
      SecondSeparator := Pos('|', Remainder);
      if SecondSeparator = 0 then begin
        Reason := 'Install receipt shortcut record is malformed.';
        Exit;
      end;
      Field1 := Copy(Remainder, 1, SecondSeparator - 1);
      Field2 := Copy(Remainder, SecondSeparator + 1, Length(Remainder));
      if not ValidSha256(Field2) then begin
        Reason := 'Install receipt contains an invalid shortcut hash.';
        Exit;
      end;
      if CompareText(Field1, 'StartMenu') = 0 then begin
        if StartMenuSeen then begin
          Reason := 'Install receipt contains a duplicate Start Menu shortcut.';
          Exit;
        end;
        StartMenuSeen := True;
        ReceiptStartMenuHash := Field2;
      end else if CompareText(Field1, 'Desktop') = 0 then begin
        if ReceiptDesktopHash <> '' then begin
          Reason := 'Install receipt contains a duplicate desktop shortcut.';
          Exit;
        end;
        ReceiptDesktopHash := Field2;
      end else begin
        Reason := 'Install receipt contains an unknown shortcut kind.';
        Exit;
      end;
    end else begin
      Reason := 'Install receipt contains an unknown record type.';
      Exit;
    end;
  end;

  SdirIndex := InventoryRowIndex(ReceiptInventoryId, 'SDIR.exe');
  if not AppIdSeen or not DesktopSeen or not InventorySeen or
     (InventoryRowsForId(ReceiptInventoryId) = 0) or
     (PayloadRecords <> InventoryRowsForId(ReceiptInventoryId)) or
     (SdirIndex < 0) or not ReceiptPayloadSeen[SdirIndex] or
     not StartMenuSeen then begin
    Reason := 'Install receipt is incomplete.';
    Exit;
  end;
  for Index := 0 to InventoryRowCount - 1 do begin
    if (AcceptedInventoryIds[Index] = ReceiptInventoryId) and
       not ReceiptPayloadSeen[Index] then begin
      Reason := 'Install receipt does not contain the complete selected compiled inventory.';
      Exit;
    end;
  end;
  if ReceiptDesktopEnabled <> (ReceiptDesktopHash <> '') then begin
    Reason := 'Install receipt desktop shortcut state is inconsistent.';
    Exit;
  end;

  FullPath := StartMenuShortcutPath;
  if HasReparseComponent(FullPath, Reason) then Exit;
  if DirExists(FullPath) then begin
    Reason := 'A directory occupies the SDIR Start Menu shortcut path.';
    Exit;
  end;
  if FileExists(FullPath) and
     (CompareText(GetSHA256OfFile(FullPath), ReceiptStartMenuHash) <> 0) then begin
    Reason := 'The Start Menu shortcut was modified.';
    Exit;
  end;
  FullPath := DesktopShortcutPath;
  if HasReparseComponent(FullPath, Reason) then Exit;
  if DirExists(FullPath) then begin
    Reason := 'A directory occupies the SDIR desktop shortcut path.';
    Exit;
  end;
  if FileExists(FullPath) then begin
    if ReceiptDesktopHash = '' then begin
      Reason := 'An unowned desktop shortcut collides with SDIR.';
      Exit;
    end;
    if CompareText(GetSHA256OfFile(FullPath), ReceiptDesktopHash) <> 0 then begin
      Reason := 'The desktop shortcut was modified.';
      Exit;
    end;
  end;
  if (ReceiptDesktopHash <> '') and not FileExists(FullPath) then
    Log('SDIR_OWNED_DESKTOP_SHORTCUT_MISSING: it can be repaired');

  for Index := 0 to InventoryRowCount - 1 do begin
    if AcceptedPathIndex(AcceptedInventoryPaths[Index]) = Index then begin
      FullPath := OwnedFilePath(Root, AcceptedInventoryPaths[Index]);
      if HasReparseComponent(FullPath, Reason) then Exit;
      if DirExists(FullPath) then begin
        Reason := 'A directory collides with an accepted SDIR inventory path: ' +
          AcceptedInventoryPaths[Index];
        Exit;
      end;
      if FileExists(FullPath) then begin
        InventoryIndexValue := InventoryRowIndex(ReceiptInventoryId,
          AcceptedInventoryPaths[Index]);
        if InventoryIndexValue < 0 then begin
          Reason := 'An unowned file collides with an accepted SDIR inventory path: ' +
            AcceptedInventoryPaths[Index];
          Exit;
        end;
        if not FileSizeMatches(FullPath, AcceptedInventoryBytes[InventoryIndexValue]) or
           (CompareText(GetSHA256OfFile(FullPath),
             AcceptedInventoryHashes[InventoryIndexValue]) <> 0) then begin
          Reason := 'An owned file changed during receipt validation: ' +
            AcceptedInventoryPaths[Index];
          Exit;
        end;
      end;
    end;
  end;

  Result := True;
end;

function ValidateRegistrationFreeRetainedRoot(const Root: String;
  var Reason: String): Boolean;
var
  Index: Integer;
  Path: String;
begin
  Result := False;
  Reason := '';
  for Index := 0 to InventoryRowCount - 1 do begin
    if AcceptedPathIndex(AcceptedInventoryPaths[Index]) = Index then begin
      Path := OwnedFilePath(Root, AcceptedInventoryPaths[Index]);
      if FileExists(Path) or DirExists(Path) then begin
        Reason := 'A registration-free retained root still contains a compiled-owned payload path: ' +
          AcceptedInventoryPaths[Index];
        Exit;
      end;
    end;
  end;
  if FileExists(AddBackslash(Root) + 'unins000.exe') or
     DirExists(AddBackslash(Root) + 'unins000.exe') or
     FileExists(AddBackslash(Root) + 'unins000.dat') or
     DirExists(AddBackslash(Root) + 'unins000.dat') or
     FileExists(StartMenuShortcutPath) or DirExists(StartMenuShortcutPath) or
     FileExists(DesktopShortcutPath) or DirExists(DesktopShortcutPath) then begin
    Reason := 'A registration-free retained root still contains native metadata or an SDIR shortcut.';
    Exit;
  end;
  Result := True;
end;

function ProgramQuarantineRoot(const Root: String): String; forward;

function ValidateRootAndOwnership(const Root: String; var Reason: String): Boolean;
var
  ExpectedRoot, RegistryName: String;
  RootExists, RegistryExists: Boolean;
begin
  Result := False;
  Reason := '';
#ifdef TEST_BUILD
  if '{#RootNameValue}' = '{#ProductionRootName}' then begin
    Reason := 'The compiled installation root must be distinct from production.';
    Exit;
  end;
  if Copy(ExtractFileName(Root), 1, 9) <> 'SDIR-UAT-' then begin
    Reason := 'A test build only accepts its compiled SDIR-UAT- installation root.';
    Exit;
  end;
#endif
  ExpectedRoot := ExpectedInstallRoot;
  if CompareText(NormalizeWindowsPath(Root), NormalizeWindowsPath(ExpectedRoot)) <> 0 then begin
    Reason := 'Only the fixed per-user installation directory is supported.';
    Exit;
  end;
  if HasReparseComponent(Root, Reason) then Exit;
  if DirExists(Root) and FileExists(AddBackslash(Root) + 'portable.flag') then begin
    Reason := 'A portable.flag destination is never modified by the installer.';
    Exit;
  end;

  RootExists := DirExists(Root);
  RegistryName := GetUninstallRegistryKey;
  RegistryExists := RegKeyExists(HKEY_CURRENT_USER, RegistryName);
  if HasReparseComponent(ProgramQuarantineRoot(Root), Reason) or
     HasReparseComponent(AddBackslash(StartMenuDirectory) + QuarantineLeaf, Reason) or
     HasReparseComponent(AddBackslash(ExtractFileDir(DesktopShortcutPath)) +
       QuarantineLeaf + '-Desktop', Reason) or
     HasReparseComponent(ReceiptPath(Root), Reason) or
     HasReparseComponent(AddBackslash(Root) + 'unins000.exe', Reason) or
     HasReparseComponent(AddBackslash(Root) + 'unins000.dat', Reason) or
     HasReparseComponent(StartMenuShortcutPath, Reason) or
     HasReparseComponent(DesktopShortcutPath, Reason) then Exit;
  if FileExists(ReceiptPath(Root) + '.new') or
     DirExists(ReceiptPath(Root) + '.new') then begin
    Reason := 'An unexplained receipt sidecar already exists.';
    Exit;
  end;
  if DirExists(ProgramQuarantineRoot(Root)) or
     FileExists(ProgramQuarantineRoot(Root)) or
     DirExists(AddBackslash(StartMenuDirectory) + QuarantineLeaf) or
     FileExists(AddBackslash(StartMenuDirectory) + QuarantineLeaf) or
     DirExists(AddBackslash(ExtractFileDir(DesktopShortcutPath)) +
       QuarantineLeaf + '-Desktop') or
     FileExists(AddBackslash(ExtractFileDir(DesktopShortcutPath)) +
       QuarantineLeaf + '-Desktop') then begin
    Reason := 'An unexplained rollback directory exists; no files were changed.';
    Exit;
  end;
  if not RootExists and RegistryExists then begin
    Reason := 'An owned HKCU registration exists without its installation directory.';
    Exit;
  end;
  if not RootExists then begin
    if FileExists(StartMenuShortcutPath) or FileExists(DesktopShortcutPath) then begin
      Reason := 'An unowned shortcut already uses an SDIR shortcut path.';
      Exit;
    end;
    Result := True;
    Exit;
  end;
  if not ReadReceipt(Root, Reason) then Exit;
  if RegistryExists then begin
    if not ValidateRegisteredNativeMetadata(Root, Reason) then Exit;
  end else begin
    if not ValidateRegistrationFreeRetainedRoot(Root, Reason) then Exit;
    Log('SDIR_FORMER_OWNER_RECEIPT_ACCEPTED');
  end;
  Result := True;
end;

function WideProcessName(const Entry: TProcessEntry32): String;
var
  Index: Integer;
begin
  Result := '';
  for Index := 0 to 259 do begin
    if Entry.szExeFile[Index] = #0 then Break;
    Result := Result + Entry.szExeFile[Index];
  end;
end;

function IsInstalledImageRunning(const Root: String; var Reason: String): Boolean;
var
  Snapshot, Process, CurrentProcess: Integer;
  Entry: TProcessEntry32;
  ProcessName, ImageName, ExpectedImage: String;
  BufferSize: Cardinal;
  Found, QuerySucceeded: Boolean;
begin
  Result := False;
  Reason := '';
  Snapshot := CreateToolhelp32Snapshot(ProcessSnapshotFlag, 0);
  if Snapshot = InvalidHandleValue then begin
    Result := True;
    Reason := 'Could not inspect running process images safely.';
    Exit;
  end;
  CurrentProcess := GetCurrentProcessId;
  ExpectedImage := NormalizeWindowsPath(AddBackslash(Root) + 'SDIR.exe');
  if SizeOf(Entry) <> ProcessEntry32ExpectedSize then begin
    CloseHandle(Snapshot);
    Result := True;
    Reason := 'The process enumeration structure has an unexpected 32-bit size.';
    Exit;
  end;
  Entry.dwSize := SizeOf(Entry);
  Found := Process32FirstW(Snapshot, Entry);
  while Found do begin
    ProcessName := WideProcessName(Entry);
    if (Entry.th32ProcessID <> CurrentProcess) and
       (CompareText(ProcessName, 'SDIR.exe') = 0) then begin
      Process := OpenProcess(ProcessQueryLimitedInformation, False,
        Entry.th32ProcessID);
      if Process = 0 then begin
        CloseHandle(Snapshot);
        Result := True;
        Reason := 'Could not verify an SDIR.exe process image; close it or retry.';
        Exit;
      end;
      SetLength(ImageName, 32767);
      BufferSize := 32767;
      QuerySucceeded := QueryFullProcessImageNameW(Process, 0, ImageName, BufferSize);
      CloseHandle(Process);
      if not QuerySucceeded then begin
        CloseHandle(Snapshot);
        Result := True;
        Reason := 'Could not verify an SDIR.exe process image; close it or retry.';
        Exit;
      end;
      SetLength(ImageName, BufferSize);
      if CompareText(NormalizeWindowsPath(ImageName), ExpectedImage) = 0 then begin
        CloseHandle(Snapshot);
        Result := True;
        Reason := 'SDIR.exe or its worker is still running.';
        Exit;
      end;
    end;
    Found := Process32NextW(Snapshot, Entry);
  end;
  CloseHandle(Snapshot);
end;

function TryExclusiveProbe(const Path: String): Boolean;
var
  Handle: Integer;
begin
  Result := True;
  if not FileExists(Path) then Exit;
  Handle := CreateFileW(Path, GenericRead, 0, 0, OpenExisting, 0, 0);
  if Handle = InvalidHandleValue then begin
    Result := False;
    Exit;
  end;
  CloseHandle(Handle);
end;

function TryUninstallerExecutableReadProbe(const Path: String): Boolean;
var
  Handle: Integer;
begin
  Result := True;
  if not FileExists(Path) then Exit;
  Handle := CreateFileW(Path, GenericRead, ShareRead or ShareWrite or ShareDelete,
    0, OpenExisting, 0, 0);
  if Handle = InvalidHandleValue then begin
    Result := False;
    Exit;
  end;
  CloseHandle(Handle);
end;

function ProbeExternalLocks(const Root: String; var Reason: String): Boolean;
var
  Index: Integer;
  Path: String;
begin
  Result := False;
  Reason := '';
  if not DirExists(Root) then begin
    Result := True;
    Exit;
  end;
  if IsInstalledImageRunning(Root, Reason) then Exit;
  for Index := 0 to InventoryRowCount - 1 do begin
    if AcceptedPathIndex(AcceptedInventoryPaths[Index]) = Index then begin
      Path := OwnedFilePath(Root, AcceptedInventoryPaths[Index]);
      if not TryExclusiveProbe(Path) then begin
        Reason := 'An accepted owned program file is locked by another process: ' +
          AcceptedInventoryPaths[Index];
        Exit;
      end;
    end;
  end;
  if not TryExclusiveProbe(ReceiptPath(Root)) then begin
    Reason := 'The SDIR install receipt is locked by another process.';
    Exit;
  end;
  if not TryExclusiveProbe(StartMenuShortcutPath) then begin
    Reason := 'The SDIR Start Menu shortcut is locked by another process.';
    Exit;
  end;
  if not TryExclusiveProbe(DesktopShortcutPath) then begin
    Reason := 'The SDIR desktop shortcut is locked by another process.';
    Exit;
  end;
  if IsUninstaller then begin
    Path := AddBackslash(Root) + 'unins000.exe';
    if CompareText(NormalizeWindowsPath(ExpandConstant('{uninstallexe}')),
      NormalizeWindowsPath(Path)) <> 0 then begin
      Reason := 'The native uninstaller is not running from the corroborated original executable path.';
      Exit;
    end;
    if not TryUninstallerExecutableReadProbe(Path) then begin
      Reason := 'The native uninstaller executable is denied read access by another process.';
      Exit;
    end;
  end else begin
    if not TryExclusiveProbe(AddBackslash(Root) + 'unins000.exe') or
       not TryExclusiveProbe(AddBackslash(Root) + 'unins000.dat') then begin
      Reason := 'Native uninstall metadata is locked by another process.';
      Exit;
    end;
  end;
  Result := True;
end;

function IsSupportedRegistryType(ValueType: Cardinal): Boolean;
begin
  Result := (ValueType = RegistryTypeNone) or
    (ValueType = RegistryTypeString) or
    (ValueType = RegistryTypeExpandString) or
    (ValueType = RegistryTypeBinary) or
    (ValueType = RegistryTypeDWord) or
    (ValueType = RegistryTypeDWordBigEndian) or
    (ValueType = RegistryTypeLink) or
    (ValueType = RegistryTypeMultiString) or
    (ValueType = RegistryTypeQWord);
end;

function RegistryValueWasSnapshotted(const Name: String): Boolean;
var
  Index: Integer;
begin
  Result := False;
  for Index := 0 to GetArrayLength(RegistryValueNames) - 1 do
    if CompareText(RegistryValueNames[Index], Name) = 0 then begin
      Result := True;
      Exit;
    end;
end;

function IsNativeUninstallValueName(const Name: String): Boolean;
begin
  Result := (Copy(Uppercase(Name), 1, 12) = 'INNO SETUP: ') or
    (CompareText(Name, 'DisplayName') = 0) or
    (CompareText(Name, 'DisplayVersion') = 0) or
    (CompareText(Name, 'Publisher') = 0) or
    (CompareText(Name, 'UninstallString') = 0) or
    (CompareText(Name, 'QuietUninstallString') = 0) or
    (CompareText(Name, 'InstallLocation') = 0) or
    (CompareText(Name, 'DisplayIcon') = 0) or
    (CompareText(Name, 'URLInfoAbout') = 0) or
    (CompareText(Name, 'HelpLink') = 0) or
    (CompareText(Name, 'URLUpdateInfo') = 0) or
    (CompareText(Name, 'Inno Setup: App Path') = 0) or
    (CompareText(Name, 'Inno Setup: Icon Group') = 0) or
    (CompareText(Name, 'Inno Setup: User') = 0) or
    (CompareText(Name, 'Inno Setup: Setup Version') = 0) or
    (CompareText(Name, 'Inno Setup: Language') = 0) or
    (CompareText(Name, 'Inno Setup: Selected Tasks') = 0) or
    (CompareText(Name, 'Inno Setup: Deselected Tasks') = 0) or
    (CompareText(Name, 'Inno Setup: Components') = 0) or
    (CompareText(Name, 'Inno Setup: Setup Type') = 0) or
    (CompareText(Name, 'InstallDate') = 0) or
    (CompareText(Name, 'NoModify') = 0) or
    (CompareText(Name, 'NoRepair') = 0) or
    (CompareText(Name, 'EstimatedSize') = 0);
end;

function SnapshotUninstallRegistry(var Reason: String): Boolean;
var
  Index, KeyHandle, Attempt: Integer;
  KeyPath, ValueName, DataBuffer: String;
  ValueNames, SubkeyNames: TArrayOfString;
  ValueType, DataLength, Status: Cardinal;
begin
  Result := False;
  Reason := '';
  RegistrySnapshotCaptured := False;
  SetArrayLength(RegistryValueNames, 0);
  SetArrayLength(RegistryValueTypes, 0);
  SetArrayLength(RegistryValueByteLengths, 0);
  SetArrayLength(RegistryValueData, 0);
  KeyPath := GetUninstallRegistryKey;
  RegistryKeyExisted := RegKeyExists(HKEY_CURRENT_USER, KeyPath);
  if not RegistryKeyExisted then begin
    RegistrySnapshotCaptured := True;
    Result := True;
    Exit;
  end;
  if not RegGetSubkeyNames(HKEY_CURRENT_USER, KeyPath, SubkeyNames) then begin
    Reason := 'Could not enumerate subkeys under the owned uninstall key.';
    Exit;
  end;
  if GetArrayLength(SubkeyNames) <> 0 then begin
    Reason := 'The owned uninstall key contains subkeys; no files were changed.';
    Exit;
  end;
  if not RegGetValueNames(HKEY_CURRENT_USER, KeyPath, ValueNames) then begin
    Reason := 'Could not enumerate every value under the owned uninstall key.';
    Exit;
  end;
  Status := RegOpenKeyExW(HKEY_CURRENT_USER, KeyPath, 0,
    KeyQueryValue or KeyWow64_64Key, KeyHandle);
  if Status <> 0 then begin
    Reason := 'Could not open the owned uninstall key for a complete snapshot.';
    Exit;
  end;
  SetArrayLength(RegistryValueNames, GetArrayLength(ValueNames));
  SetArrayLength(RegistryValueTypes, GetArrayLength(ValueNames));
  SetArrayLength(RegistryValueByteLengths, GetArrayLength(ValueNames));
  SetArrayLength(RegistryValueData, GetArrayLength(ValueNames));
  for Index := 0 to GetArrayLength(ValueNames) - 1 do begin
    ValueName := ValueNames[Index];
    DataLength := 0;
    ValueType := 0;
    Status := RegQueryValueExW(KeyHandle, ValueName, 0, ValueType, '', DataLength);
    if (Status <> 0) and (Status <> ErrorMoreData) then begin
      RegCloseKey(KeyHandle);
      Reason := 'Could not query an uninstall registry value: ' + ValueName;
      Exit;
    end;
    DataBuffer := '';
    Attempt := 0;
    while Status = ErrorMoreData do begin
      if DataLength > MaxRegistryValueBytes then begin
        RegCloseKey(KeyHandle);
        Reason := 'An uninstall registry value exceeds the supported snapshot size: ' +
          ValueName;
        Exit;
      end;
      if Attempt >= 4 then begin
        RegCloseKey(KeyHandle);
        Reason := 'An uninstall registry value changed repeatedly during snapshot: ' +
          ValueName;
        Exit;
      end;
      SetLength(DataBuffer, (DataLength + 1) div 2);
      Status := RegQueryValueExW(KeyHandle, ValueName, 0, ValueType,
        DataBuffer, DataLength);
      Inc(Attempt);
    end;
    if Status <> 0 then begin
      RegCloseKey(KeyHandle);
      Reason := 'Could not read the complete uninstall registry value: ' + ValueName;
      Exit;
    end;
    if DataLength > MaxRegistryValueBytes then begin
      RegCloseKey(KeyHandle);
      Reason := 'An uninstall registry value exceeds the supported snapshot size: ' +
        ValueName;
      Exit;
    end;
    if not IsSupportedRegistryType(ValueType) then begin
      RegCloseKey(KeyHandle);
      Reason := 'An uninstall registry value has an unsupported type: ' + ValueName;
      Exit;
    end;
    SetLength(DataBuffer, (DataLength + 1) div 2);
    RegistryValueNames[Index] := ValueName;
    RegistryValueTypes[Index] := IntToStr(ValueType);
    RegistryValueByteLengths[Index] := IntToStr(DataLength);
    RegistryValueData[Index] := DataBuffer;
  end;
  RegCloseKey(KeyHandle);
  RegistrySnapshotCaptured := True;
  Result := True;
end;

function RestoreUninstallRegistry(var Reason: String): Boolean;
var
  Index, KeyHandle: Integer;
  KeyPath, ValueName: String;
  CurrentValueNames, CurrentSubkeyNames: TArrayOfString;
  Status, ValueType, DataLength, Disposition: Cardinal;
  UnexpectedValueFound: Boolean;
begin
  Result := True;
  Reason := '';
  if not RegistrySnapshotCaptured then Exit;
  KeyPath := GetUninstallRegistryKey;
  if not RegKeyExists(HKEY_CURRENT_USER, KeyPath) then begin
    if not RegistryKeyExisted then Exit;
    Status := RegCreateKeyExW(HKEY_CURRENT_USER, KeyPath, 0, '', 0,
      KeySetValue or KeyQueryValue or KeyWow64_64Key, 0, KeyHandle, Disposition);
  end else
    Status := RegOpenKeyExW(HKEY_CURRENT_USER, KeyPath, 0,
      KeySetValue or KeyQueryValue or KeyWow64_64Key, KeyHandle);
  if Status <> 0 then begin
    Result := False;
    Reason := 'Could not open the uninstall key to restore its prior values.';
    Exit;
  end;

  UnexpectedValueFound := False;
  if not RegGetValueNames(HKEY_CURRENT_USER, KeyPath, CurrentValueNames) then begin
    RegCloseKey(KeyHandle);
    Result := False;
    Reason := 'Could not enumerate uninstall values during compensation.';
    Exit;
  end;
  for Index := 0 to GetArrayLength(CurrentValueNames) - 1 do begin
    ValueName := CurrentValueNames[Index];
    if not RegistryValueWasSnapshotted(ValueName) then begin
      if IsNativeUninstallValueName(ValueName) then begin
        if not RegDeleteValue(HKEY_CURRENT_USER, KeyPath, ValueName) then begin
          RegCloseKey(KeyHandle);
          Result := False;
          Reason := 'Could not remove a new native uninstall value: ' + ValueName;
          Exit;
        end;
      end else
        UnexpectedValueFound := True;
    end;
  end;

  for Index := 0 to GetArrayLength(RegistryValueNames) - 1 do begin
    ValueType := StrToIntDef(RegistryValueTypes[Index], -1);
    DataLength := StrToIntDef(RegistryValueByteLengths[Index], -1);
    if (ValueType = $FFFFFFFF) or (DataLength = $FFFFFFFF) or
       (DataLength > MaxRegistryValueBytes) then begin
      RegCloseKey(KeyHandle);
      Result := False;
      Reason := 'The saved uninstall registry snapshot is invalid.';
      Exit;
    end;
    Status := RegSetValueExW(KeyHandle, RegistryValueNames[Index], 0,
      ValueType, RegistryValueData[Index], DataLength);
    if Status <> 0 then begin
      RegCloseKey(KeyHandle);
      Result := False;
      Reason := 'Could not restore an uninstall registry value: ' +
        RegistryValueNames[Index];
      Exit;
    end;
  end;
  RegCloseKey(KeyHandle);

  if not RegGetSubkeyNames(HKEY_CURRENT_USER, KeyPath, CurrentSubkeyNames) then begin
    Result := False;
    Reason := 'Could not verify uninstall subkeys after compensation.';
    Exit;
  end;
  if GetArrayLength(CurrentSubkeyNames) <> 0 then begin
    Result := False;
    Reason := 'A new uninstall subkey was preserved during compensation.';
    Exit;
  end;
  if not RegistryKeyExisted then begin
    if not RegGetValueNames(HKEY_CURRENT_USER, KeyPath, CurrentValueNames) then begin
      Result := False;
      Reason := 'Could not verify newly created uninstall values after compensation.';
      Exit;
    end;
    if GetArrayLength(CurrentValueNames) <> 0 then begin
      Result := False;
      Reason := 'Unrecognized uninstall values were preserved during compensation.';
      Exit;
    end;
    if RegKeyExists(HKEY_CURRENT_USER, KeyPath) then begin
      Status := RegDeleteKeyExW(HKEY_CURRENT_USER, KeyPath, KeyWow64_64Key, 0);
      if Status <> 0 then begin
        Result := False;
        Reason := 'Could not remove the empty uninstall key created by this install.';
      end;
    end;
  end else if UnexpectedValueFound then begin
    Result := False;
    Reason := 'Unrecognized uninstall values were preserved during compensation.';
  end;
end;

function SnapshotDirectory(const Path: String; var Reason: String): Boolean;
var
  Index, Handle: Integer;
  Attributes: Cardinal;
  CreationTime, AccessTime, WriteTime: TFileTime;
begin
  Result := False;
  for Index := 0 to DirectorySnapshotCount - 1 do
    if CompareText(DirectorySnapshotPaths[Index], Path) = 0 then begin
      Result := True;
      Exit;
    end;
  if DirectorySnapshotCount >= MaxDirectorySnapshots then begin
    Reason := 'Directory metadata snapshot capacity exceeded.';
    Exit;
  end;
  Attributes := GetFileAttributesW(Path);
  DirectorySnapshotPaths[DirectorySnapshotCount] := Path;
  DirectoryCreatedByRun[DirectorySnapshotCount] := False;
  if Attributes = InvalidFileAttributes then begin
    DirectorySnapshotExists[DirectorySnapshotCount] := False;
    Inc(DirectorySnapshotCount);
    Result := True;
    Exit;
  end;
  if (Attributes and FileAttributeDirectory) = 0 then begin
    Reason := 'A required directory path is occupied by a file: ' + Path;
    Exit;
  end;
  Handle := CreateFileW(Path, FileReadAttributes, ShareRead or ShareWrite or ShareDelete,
    0, OpenExisting, FileFlagBackupSemantics, 0);
  if Handle = InvalidHandleValue then begin
    Reason := 'Could not snapshot directory metadata: ' + Path;
    Exit;
  end;
  if not GetFileTime(Handle, CreationTime, AccessTime, WriteTime) then begin
    CloseHandle(Handle);
    Reason := 'Could not read directory timestamps: ' + Path;
    Exit;
  end;
  CloseHandle(Handle);
  DirectorySnapshotExists[DirectorySnapshotCount] := True;
  DirectorySnapshotAttributes[DirectorySnapshotCount] := Attributes;
  DirectorySnapshotCreation[DirectorySnapshotCount] := CreationTime;
  DirectorySnapshotAccess[DirectorySnapshotCount] := AccessTime;
  DirectorySnapshotWrite[DirectorySnapshotCount] := WriteTime;
  Inc(DirectorySnapshotCount);
  Result := True;
end;

function EnsureDirectory(const Path: String): Boolean; forward;

procedure MarkDirectoryCreatedByRun(const Path: String);
var
  Index: Integer;
begin
  for Index := 0 to DirectorySnapshotCount - 1 do
    if CompareText(DirectorySnapshotPaths[Index], Path) = 0 then begin
      if not DirectorySnapshotExists[Index] then
        DirectoryCreatedByRun[Index] := True;
      Exit;
    end;
end;

procedure MarkObservedInstallerDirectories(const Root: String);
var
  ProgramsPath, DocsPath: String;
begin
  ProgramsPath := ExtractFileDir(Root);
  DocsPath := AddBackslash(Root) + 'docs';
  if DirExists(ProgramsPath) then MarkDirectoryCreatedByRun(ProgramsPath);
  if DirExists(Root) then MarkDirectoryCreatedByRun(Root);
  if DirExists(DocsPath) then MarkDirectoryCreatedByRun(DocsPath);
end;

function RestoreDirectoryMetadata(var Reason: String): Boolean;
var
  Index, Handle: Integer;
  Path: String;
  CreationTime, AccessTime, WriteTime: TFileTime;
begin
  Result := True;
  Reason := '';
  for Index := DirectorySnapshotCount - 1 downto 0 do begin
    Path := DirectorySnapshotPaths[Index];
    if HasReparseComponent(Path, Reason) then begin
      Result := False;
      Continue;
    end;
    if not DirectorySnapshotExists[Index] then begin
      if DirectoryCreatedByRun[Index] and DirExists(Path) and
         not RemoveDir(Path) then begin
        Result := False;
        Reason := 'A newly created empty directory could not be removed: ' + Path;
      end;
      if not DirExists(Path) then
        DirectoryCreatedByRun[Index] := False;
    end else begin
      if not DirExists(Path) then begin
        if not EnsureDirectory(Path) then begin
          Result := False;
          Reason := 'A previous directory could not be restored: ' + Path;
          Continue;
        end;
      end;
      Handle := CreateFileW(Path, FileWriteAttributes,
        ShareRead or ShareWrite or ShareDelete, 0, OpenExisting,
        FileFlagBackupSemantics, 0);
      if Handle = InvalidHandleValue then begin
        Result := False;
        Reason := 'Could not restore directory metadata: ' + Path;
        Continue;
      end;
      CreationTime := DirectorySnapshotCreation[Index];
      AccessTime := DirectorySnapshotAccess[Index];
      WriteTime := DirectorySnapshotWrite[Index];
      if not SetFileTime(Handle, CreationTime, AccessTime, WriteTime) then begin
        Result := False;
        Reason := 'Could not restore directory timestamps: ' + Path;
      end;
      CloseHandle(Handle);
      if not SetFileAttributesW(Path, DirectorySnapshotAttributes[Index]) then begin
        Result := False;
        Reason := 'Could not restore directory attributes: ' + Path;
      end;
    end;
  end;
end;

function EnsureDirectory(const Path: String): Boolean;
var
  ParentPath: String;
begin
  Result := DirExists(Path);
  if Result then Exit;
  ParentPath := ExtractFileDir(Path);
  if (ParentPath = '') or (ParentPath = Path) then Exit;
  if not DirExists(ParentPath) and not EnsureDirectory(ParentPath) then Exit;
  Result := CreateDir(Path);
  if Result then
    MarkDirectoryCreatedByRun(Path)
  else
    Result := DirExists(Path);
end;

function CapturePreInstallState(const Root: String; var Reason: String): Boolean;
var
  ProgramsPath, UserProgramsPath, StartMenuPath, DesktopPath: String;
begin
  Result := False;
  Reason := '';
  if not ValidateRootAndOwnership(Root, Reason) then Exit;
  if not ProbeExternalLocks(Root, Reason) then Exit;
  ProgramsPath := ExtractFileDir(Root);
  StartMenuPath := StartMenuDirectory;
  UserProgramsPath := ExtractFileDir(StartMenuPath);
  DesktopPath := ExtractFileDir(DesktopShortcutPath);
  if not SnapshotDirectory(ProgramsPath, Reason) or
     not SnapshotDirectory(Root, Reason) or
     not SnapshotDirectory(AddBackslash(Root) + 'docs', Reason) or
     not SnapshotDirectory(UserProgramsPath, Reason) or
     not SnapshotDirectory(StartMenuPath, Reason) or
     not SnapshotDirectory(DesktopPath, Reason) then Exit;
  if not SnapshotUninstallRegistry(Reason) then Exit;
  Result := True;
end;

function AddQuarantineDirectory(const Path: String; var Reason: String): Boolean;
var
  Index: Integer;
begin
  Result := False;
  for Index := 0 to QuarantineDirCount - 1 do
    if CompareText(QuarantineDirs[Index], Path) = 0 then begin
      Result := True;
      Exit;
    end;
  if HasReparseComponent(Path, Reason) then Exit;
  if DirExists(Path) or FileExists(Path) then begin
    Reason := 'An unexplained rollback directory already exists: ' + Path;
    Exit;
  end;
  if QuarantineDirCount >= 3 then begin
    Reason := 'Rollback directory capacity exceeded.';
    Exit;
  end;
  if not CreateDir(Path) then begin
    Reason := 'Could not create same-volume rollback directory: ' + Path;
    Exit;
  end;
  QuarantineDirs[QuarantineDirCount] := Path;
  Inc(QuarantineDirCount);
  Result := True;
end;

function QuarantineFile(const OriginalPath, QuarantineRoot,
  RelativePath: String; var Reason: String): Boolean;
var
  BackupPath, ExpectedHash: String;
  ExpectedBytes: Int64;
  MoveIndex: Integer;
begin
  Result := False;
  if not FileExists(OriginalPath) then begin
    if DirExists(OriginalPath) then begin
      Reason := 'A directory occupies an owned file path: ' + RelativePath;
      Exit;
    end;
    Result := True;
    Exit;
  end;
  if not IsRegularNonReparseFile(OriginalPath) then begin
    Reason := 'An owned file is not a regular non-reparse file: ' + RelativePath;
    Exit;
  end;
  ExpectedHash := GetSHA256OfFile(OriginalPath);
  if ExpectedHash = '' then begin
    Reason := 'An owned file could not be snapshotted before quarantine: ' + RelativePath;
    Exit;
  end;
  if not FileSize64(OriginalPath, ExpectedBytes) then begin
    Reason := 'An owned file could not be snapshotted before quarantine: ' + RelativePath;
    Exit;
  end;
  if MovedCount >= MaxMovedFiles then begin
    Reason := 'Rollback file capacity exceeded.';
    Exit;
  end;
  if not AddQuarantineDirectory(QuarantineRoot, Reason) then Exit;
  BackupPath := AddBackslash(QuarantineRoot) + IntToStr(MovedCount) + '.bak';
  if not MoveFileW(OriginalPath, BackupPath) then begin
    Reason := 'A known owned file could not be moved into quarantine: ' +
      RelativePath;
    Exit;
  end;
  MoveIndex := MovedCount;
  MovedOriginalPaths[MoveIndex] := OriginalPath;
  MovedBackupPaths[MoveIndex] := BackupPath;
  MovedExpectedHashes[MoveIndex] := ExpectedHash;
  MovedExpectedBytes[MoveIndex] := ExpectedBytes;
  MovedVerified[MoveIndex] := False;
  MovedRestored[MoveIndex] := False;
  Inc(MovedCount);
  Log('SDIR_QUARANTINED: ' + RelativePath);
  if FileExists(OriginalPath) or DirExists(OriginalPath) then begin
    Reason := 'The original path still exists after a quarantine move: ' + RelativePath;
    Exit;
  end;
  if not SavedFileMatches(BackupPath, ExpectedHash, ExpectedBytes) then begin
    Reason := 'The quarantined backup failed its immediate size and hash verification and was retained: ' +
      BackupPath;
    Exit;
  end;
  MovedVerified[MoveIndex] := True;
#ifdef TEST_FAULT_BACKUP
  if MovedCount = 1 then begin
    Log('SDIR_TEST_FAULT_BACKUP_AFTER_FIRST_MOVE');
    RaiseException('TEST ONLY: injected backup failure immediately after the first quarantine move.');
  end;
#endif
  Result := True;
end;

function ProgramQuarantineRoot(const Root: String): String;
begin
  Result := AddBackslash(Root) + QuarantineLeaf;
end;

function QuarantineOwnedState(const Root: String; var Reason: String): Boolean;
var
  Index: Integer;
  RelativePath, Path, StartMenuQ, DesktopQ: String;
begin
  Result := False;
  Reason := '';
  for Index := 0 to InventoryRowCount - 1 do begin
    RelativePath := AcceptedInventoryPaths[Index];
    if AcceptedPathIndex(RelativePath) = Index then begin
      Path := OwnedFilePath(Root, RelativePath);
      if not QuarantineFile(Path, ProgramQuarantineRoot(Root),
        'program\' + RelativePath, Reason) then Exit;
    end;
  end;
  if not QuarantineFile(ReceiptPath(Root), ProgramQuarantineRoot(Root),
    'program\' + ReceiptFileName, Reason) then Exit;
  if not QuarantineFile(AddBackslash(Root) + 'unins000.exe',
    ProgramQuarantineRoot(Root), 'program\unins000.exe', Reason) then Exit;
  if not QuarantineFile(AddBackslash(Root) + 'unins000.dat',
    ProgramQuarantineRoot(Root), 'program\unins000.dat', Reason) then Exit;

  StartMenuQ := AddBackslash(StartMenuDirectory) + QuarantineLeaf;
  if not QuarantineFile(StartMenuShortcutPath, StartMenuQ,
    'StartMenu\' + StartMenuShortcutName, Reason) then Exit;
  DesktopQ := AddBackslash(ExtractFileDir(DesktopShortcutPath)) +
    QuarantineLeaf + '-Desktop';
  if not QuarantineFile(DesktopShortcutPath, DesktopQ,
    'Desktop\' + DesktopShortcutName, Reason) then Exit;
  Result := True;
end;

function CreateOwnedShortcuts(const Root: String; var Reason: String): Boolean;
var
  StartMenuDir, StartMenuPath, DesktopPath, LinkPath, LinkTarget: String;
begin
  Result := False;
  Reason := '';
  StartMenuDir := StartMenuDirectory;
  StartMenuPath := StartMenuShortcutPath;
  DesktopPath := DesktopShortcutPath;
  if not EnsureDirectory(StartMenuDir) then begin
    Reason := 'Could not create the per-user Start Menu folder.';
    Exit;
  end;
#ifdef TEST_BUILD
  LinkTarget := AddBackslash(Root) + TestLauncherFileName;
#else
  LinkTarget := AddBackslash(Root) + 'SDIR.exe';
#endif
  try
    LinkPath := CreateShellLink(StartMenuPath, AppNameText, LinkTarget,
      '', Root, AddBackslash(Root) + 'SDIR.exe', 0, 1);
  except
    Reason := 'Could not create the Start Menu shortcut: ' + GetExceptionMessage;
    Exit;
  end;
  CreatedStartMenuShortcut := True;
  if LinkPath = '' then LinkPath := StartMenuPath;
  NewStartMenuHash := GetSHA256OfFile(LinkPath);
  if NewStartMenuHash = '' then begin
    Reason := 'Could not hash the newly created Start Menu shortcut.';
    Exit;
  end;

  if WizardIsTaskSelected('desktopicon') then begin
    try
      LinkPath := CreateShellLink(DesktopPath, AppNameText, LinkTarget,
        '', Root, AddBackslash(Root) + 'SDIR.exe', 0, 1);
    except
      Reason := 'Could not create the optional desktop shortcut: ' +
        GetExceptionMessage;
      Exit;
    end;
    CreatedDesktopShortcut := True;
    if LinkPath = '' then LinkPath := DesktopPath;
    NewDesktopHash := GetSHA256OfFile(LinkPath);
    if NewDesktopHash = '' then begin
      Reason := 'Could not hash the newly created desktop shortcut.';
      Exit;
    end;
  end;
  Result := True;
end;

function DeleteOwnedReceiptTemp(const Root: String): Boolean;
var
  TempPath: String;
begin
  Result := True;
  if not CreatedReceiptTemp then Exit;
  TempPath := ReceiptPath(Root) + '.new';
  if FileExists(TempPath) then begin
    if (NewReceiptTempHash = '') or
       (CompareText(GetSHA256OfFile(TempPath), NewReceiptTempHash) <> 0) then begin
      Result := False;
      Exit;
    end;
    if not DeleteFile(TempPath) then begin
      Result := False;
      Exit;
    end;
  end;
  CreatedReceiptTemp := False;
  NewReceiptTempHash := '';
end;

function WriteReceiptLinesCreateNew(const Path, OwnerRoot: String;
  const Lines: TArrayOfString; TrackTemp: Boolean; var Reason: String): Boolean;
var
  Index, LineIndex, Handle: Integer;
  Contents, OneCharacter: String;
  Written: Cardinal;
  CharacterValue: Char;
begin
  Result := False;
  Reason := '';
  Contents := '';
  for LineIndex := 0 to Length(Lines) - 1 do
    Contents := Contents + Lines[LineIndex] + #13#10;
  for Index := 1 to Length(Contents) do begin
    CharacterValue := Contents[Index];
    if Ord(CharacterValue) > 127 then begin
      Reason := 'The install receipt contains a character outside its ASCII format.';
      Exit;
    end;
  end;
  Handle := CreateFileW(Path, GenericWrite, 0, 0, CreateNew, 0, 0);
  if Handle = InvalidHandleValue then begin
    Reason := 'The receipt sidecar already exists or could not be reserved with CREATE_NEW.';
    Exit;
  end;
  if TrackTemp then begin
    CreatedReceiptTemp := True;
    NewReceiptTempHash := '';
  end;
  for Index := 1 to Length(Contents) do begin
    OneCharacter := Copy(Contents, Index, 1);
    Written := 0;
    if not WriteFile(Handle, OneCharacter, 1, Written, 0) or (Written <> 1) then begin
      CloseHandle(Handle);
      if TrackTemp then begin
        NewReceiptTempHash := GetSHA256OfFile(Path);
        DeleteOwnedReceiptTemp(OwnerRoot);
      end else
        DeleteFile(Path);
      Reason := 'Could not write the complete install receipt sidecar.';
      Exit;
    end;
  end;
  if not FlushFileBuffers(Handle) then begin
    CloseHandle(Handle);
    if TrackTemp then begin
      NewReceiptTempHash := GetSHA256OfFile(Path);
      DeleteOwnedReceiptTemp(OwnerRoot);
    end else
      DeleteFile(Path);
    Reason := 'Could not flush the install receipt sidecar.';
    Exit;
  end;
  CloseHandle(Handle);
  if TrackTemp then NewReceiptTempHash := GetSHA256OfFile(Path);
  if GetSHA256OfFile(Path) = '' then begin
    Reason := 'Could not verify the install receipt sidecar.';
    Exit;
  end;
  Result := True;
end;

function BuildAndWriteReceipt(const Root: String; var Reason: String): Boolean;
var
  Lines: TArrayOfString;
  Index, Count: Integer;
  Path, TempPath, RelativePath: String;
begin
  Result := False;
  Reason := '';
  SetArrayLength(Lines, PayloadCount + 6);
  Count := 0;
  Lines[Count] := ReceiptHeader;
  Inc(Count);
  Lines[Count] := 'AppId|' + AppIdText;
  Inc(Count);
  Lines[Count] := 'Desktop|0';
  if CreatedDesktopShortcut then Lines[Count] := 'Desktop|1';
  Inc(Count);
  Lines[Count] := 'Inventory|' + ActiveInventoryId;
  Inc(Count);

  for Index := 0 to PayloadCount - 1 do begin
    RelativePath := PayloadPaths[Index];
    Path := AddBackslash(Root) + NativePayloadPath(RelativePath);
    if not FileExists(Path) then begin
      Reason := 'An installed payload file is missing before commit: ' + RelativePath;
      Exit;
    end;
    if not FileSizeMatches(Path, PayloadBytes[Index]) or
       (CompareText(GetSHA256OfFile(Path), PayloadHashes[Index]) <> 0) then begin
      Reason := 'An installed payload size or hash does not match: ' + RelativePath;
      Exit;
    end;
    Lines[Count] := 'Payload|' + RelativePath + '|' + PayloadHashes[Index];
    Inc(Count);
  end;

  if not FileExists(StartMenuShortcutPath) then begin
    Reason := 'The Start Menu shortcut is missing before commit.';
    Exit;
  end;
  Lines[Count] := 'Shortcut|StartMenu|' + GetSHA256OfFile(StartMenuShortcutPath);
  Inc(Count);
  if CreatedDesktopShortcut then begin
    if not FileExists(DesktopShortcutPath) then begin
      Reason := 'The desktop shortcut is missing before commit.';
      Exit;
    end;
    Lines[Count] := 'Shortcut|Desktop|' + GetSHA256OfFile(DesktopShortcutPath);
    Inc(Count);
  end;
  SetArrayLength(Lines, Count);

  TempPath := ReceiptPath(Root) + '.new';
  if FileExists(TempPath) or DirExists(TempPath) then begin
    Reason := 'An unexplained receipt sidecar blocks receipt creation.';
    Exit;
  end;
  if not WriteReceiptLinesCreateNew(TempPath, Root, Lines, True, Reason) then begin
    if Reason = '' then Reason := 'Could not write the install receipt.';
    Exit;
  end;
  if FileExists(ReceiptPath(Root)) or
     not MoveFileW(TempPath, ReceiptPath(Root)) then begin
    DeleteOwnedReceiptTemp(Root);
    Reason := 'Could not atomically place the install receipt.';
    Exit;
  end;
  CreatedReceiptTemp := False;
  NewReceiptTempHash := '';
  CreatedReceipt := True;
  NewReceiptHash := GetSHA256OfFile(ReceiptPath(Root));
  if NewReceiptHash = '' then begin
    Reason := 'Could not hash the newly written install receipt.';
    Exit;
  end;
  ReceiptAlreadyWritten := True;
  Result := True;
end;

#ifdef TEST_FAULT_PARTIAL
procedure InjectPartialInstallFailure;
begin
  Log('SDIR_TEST_FAULT_PARTIAL_AFTER_FIRST_PAYLOAD');
  RaiseException('TEST ONLY: injected failure after the first payload file.');
end;
#endif

procedure PayloadFileInstalled;
begin
  Inc(PayloadFilesInstalled);
  MarkObservedInstallerDirectories(ExpandConstant('{app}'));
#ifdef TEST_FAULT_PARTIAL
  if PayloadFilesInstalled = 1 then InjectPartialInstallFailure;
#endif
end;

procedure FinishPayloadInstall;
var
  Reason: String;
begin
  PayloadFileInstalled;
  if PayloadFilesInstalled <> PayloadCount then
    RaiseException('설치된 페이로드 콜백 수가 컴파일된 페이로드 수와 일치하지 않습니다.');
  MarkObservedInstallerDirectories(ExpandConstant('{app}'));
  if not CreateOwnedShortcuts(ExpandConstant('{app}'), Reason) then
    RaiseException('바로 가기를 만들지 못했습니다: ' + Reason);
  if not BuildAndWriteReceipt(ExpandConstant('{app}'), Reason) then
    RaiseException('설치된 파일 검증 또는 영수증 작성에 실패했습니다: ' + Reason);
#ifdef TEST_FAULT_LATE
  Log('SDIR_TEST_FAULT_LATE_BEFORE_FINALIZATION');
  RaiseException('TEST ONLY: injected late pre-finalization failure.');
#endif
  Log('SDIR_PAYLOAD_HASHES_VERIFIED');
end;

function RestoreMovedFiles(var Reason: String): Boolean;
var
  Index: Integer;
  OriginalPath, BackupPath, RelativePath, AppRoot, ExpectedHash: String;
  PayloadIndexValue: Integer;
  ExpectedBytes: Int64;
begin
  Result := True;
  Reason := '';
  AppRoot := ExpandConstant('{app}');
  for Index := MovedCount - 1 downto 0 do begin
    if MovedRestored[Index] then Continue;
    OriginalPath := MovedOriginalPaths[Index];
    BackupPath := MovedBackupPaths[Index];
    if not MovedVerified[Index] then begin
      Result := False;
      Reason := 'An unverified quarantined backup was retained for manual recovery: ' +
        BackupPath;
      Continue;
    end;
    if not SavedFileMatches(BackupPath, MovedExpectedHashes[Index],
      MovedExpectedBytes[Index]) then begin
      Result := False;
      Reason := 'A quarantined backup is missing or changed and was retained: ' + BackupPath;
      Continue;
    end;
    if DirExists(OriginalPath) then begin
      Result := False;
      Reason := 'A directory blocks restoration of a quarantined file: ' + OriginalPath;
      Continue;
    end;
    if FileExists(OriginalPath) then begin
      ExpectedHash := '';
      ExpectedBytes := -1;
      if CompareText(OriginalPath, ReceiptPath(AppRoot)) = 0 then
        ExpectedHash := NewReceiptHash
      else if CompareText(OriginalPath, StartMenuShortcutPath) = 0 then
        ExpectedHash := NewStartMenuHash
      else if CompareText(OriginalPath, DesktopShortcutPath) = 0 then
        ExpectedHash := NewDesktopHash
      else if Pos(Uppercase(AddBackslash(AppRoot)),
        Uppercase(OriginalPath)) = 1 then begin
        RelativePath := Copy(OriginalPath, Length(AppRoot) + 2,
          Length(OriginalPath));
        PayloadIndexValue := PayloadIndex(RelativePath);
        if PayloadIndexValue >= 0 then begin
          ExpectedHash := PayloadHashes[PayloadIndexValue];
          ExpectedBytes := PayloadBytes[PayloadIndexValue];
        end;
      end;
      if (ExpectedHash = '') or
         ((ExpectedBytes >= 0) and
           (not FileSizeMatches(OriginalPath, ExpectedBytes))) or
         (CompareText(GetSHA256OfFile(OriginalPath), ExpectedHash) <> 0) then begin
        Result := False;
        Reason := 'An unexpected file blocks restoration of a quarantined path: ' +
          OriginalPath;
        Continue;
      end;
      if not DeleteFile(OriginalPath) then begin
        Result := False;
        Reason := 'A new installed file could not be removed before restore: ' +
          OriginalPath;
        Continue;
      end;
    end;
    if not EnsureDirectory(ExtractFileDir(OriginalPath)) then begin
      Result := False;
      Reason := 'Could not recreate an original file directory: ' + OriginalPath;
      Continue;
    end;
    if not MoveFileW(BackupPath, OriginalPath) then begin
      Result := False;
      Reason := 'Could not restore quarantined file: ' + OriginalPath;
      Continue;
    end;
    if not SavedFileMatches(OriginalPath, MovedExpectedHashes[Index],
      MovedExpectedBytes[Index]) then begin
      Result := False;
      Reason := 'A restored file failed its size and hash verification: ' + OriginalPath;
      Continue;
    end;
    if IsRegularNonReparseFile(OriginalPath) then
      MovedRestored[Index] := True;
  end;
end;

function DeleteNewShortcut(const Path, ExpectedHash: String): Boolean;
begin
  Result := True;
  if not FileExists(Path) then Exit;
  if (ExpectedHash = '') or
     (CompareText(GetSHA256OfFile(Path), ExpectedHash) <> 0) then begin
    Result := False;
    Exit;
  end;
  Result := DeleteFile(Path);
end;

procedure RestoreAfterFailure;
var
  RegistryReason, FileReason, DirectoryReason: String;
  RecoveryPath: String;
  FilesRestored, RegistryRestored, DirectoriesRestored, ShortcutsRemoved: Boolean;
  QuarantineDirsRemoved: Boolean;
  Index: Integer;
begin
  if RollbackCompleted then Exit;
  ShortcutsRemoved := True;
  if CreatedStartMenuShortcut then begin
    if DeleteNewShortcut(StartMenuShortcutPath, NewStartMenuHash) then
      CreatedStartMenuShortcut := False
    else
      ShortcutsRemoved := False;
  end;
  if CreatedDesktopShortcut then begin
    if DeleteNewShortcut(DesktopShortcutPath, NewDesktopHash) then
      CreatedDesktopShortcut := False
    else
      ShortcutsRemoved := False;
  end;
  if CreatedReceipt and FileExists(ReceiptPath(ExpandConstant('{app}'))) then begin
    if (NewReceiptHash <> '') and
       (CompareText(GetSHA256OfFile(ReceiptPath(ExpandConstant('{app}'))),
         NewReceiptHash) = 0) and
       DeleteFile(ReceiptPath(ExpandConstant('{app}'))) then
      CreatedReceipt := False
    else
      ShortcutsRemoved := False;
  end else if CreatedReceipt then
    CreatedReceipt := False;
  if CreatedReceiptTemp then begin
    if DeleteOwnedReceiptTemp(ExpandConstant('{app}')) then
      CreatedReceiptTemp := False
    else
      ShortcutsRemoved := False;
  end;

  FilesRestored := RestoreMovedFiles(FileReason);
  RegistryRestored := RestoreUninstallRegistry(RegistryReason);
  QuarantineDirsRemoved := True;
  if ShortcutsRemoved and FilesRestored and RegistryRestored then begin
    for Index := QuarantineDirCount - 1 downto 0 do
      if DirExists(QuarantineDirs[Index]) and
         not RemoveDir(QuarantineDirs[Index]) then begin
        QuarantineDirsRemoved := False;
        FileReason := 'A rollback directory still contains recovery material.';
      end;
  end else
    QuarantineDirsRemoved := False;

  DirectoriesRestored := RestoreDirectoryMetadata(DirectoryReason);
  if ShortcutsRemoved and FilesRestored and RegistryRestored and
     QuarantineDirsRemoved and DirectoriesRestored then begin
    RollbackCompleted := True;
    Exit;
  end;

  RecoveryPath := ProgramQuarantineRoot(ExpandConstant('{app}'));
  Log('SDIR_ROLLBACK_INCOMPLETE; recovery material retained at: ' + RecoveryPath);
  if FileReason = '' then FileReason := 'One or more owned items could not be restored.';
  if RegistryReason <> '' then FileReason := FileReason + ' ' + RegistryReason;
  if DirectoryReason <> '' then FileReason := FileReason + ' ' + DirectoryReason;
  SuppressibleMsgBox('설치 실패 뒤 이전 상태를 모두 복구하지 못했습니다.' + #13#10 +
    '복구 파일을 삭제하지 마세요. 보존 위치: ' + RecoveryPath + #13#10 + FileReason,
    mbError, MB_OK, IDOK);
end;

function DisposeCommittedBackups: Boolean;
var
  Index: Integer;
  CleanupOk: Boolean;
begin
  Result := False;
  CleanupOk := True;
  for Index := 0 to MovedCount - 1 do begin
    if not MovedVerified[Index] or
       not SavedFileMatches(MovedBackupPaths[Index], MovedExpectedHashes[Index],
         MovedExpectedBytes[Index]) then begin
      CleanupOk := False;
    end else if not DeleteFile(MovedBackupPaths[Index]) then
      CleanupOk := False;
  end;
  if CleanupOk then begin
    for Index := 0 to QuarantineDirCount - 1 do
      if DirExists(QuarantineDirs[Index]) and
         not RemoveDir(QuarantineDirs[Index]) then
        CleanupOk := False;
  end;
  Result := CleanupOk;
  if not CleanupOk then begin
    Log('SDIR_COMMITTED_BACKUPS_RETAINED');
    SuppressibleMsgBox('설치는 완료됐지만 이전 파일 일부를 정리하지 못했습니다.' + #13#10 +
      '다음 설치 전에 보존된 .sdir-installer-rollback 폴더를 확인하세요.',
      mbInformation, MB_OK, IDOK);
  end;
end;

procedure RemoveUninstallShortcuts(const Root: String; var Reason: String);
var
  ReceiptReason: String;
begin
  Reason := '';
  if not ReadReceipt(Root, ReceiptReason) then begin
    Reason := ReceiptReason;
    Exit;
  end;
  if FileExists(StartMenuShortcutPath) then begin
    if (ReceiptStartMenuHash = '') or
       (CompareText(GetSHA256OfFile(StartMenuShortcutPath),
         ReceiptStartMenuHash) <> 0) then begin
      Reason := 'Start Menu shortcut changed after uninstall preflight; left in place.';
    end else if not DeleteFile(StartMenuShortcutPath) then begin
      Reason := 'Start Menu shortcut could not be removed.';
    end;
  end;
  if FileExists(DesktopShortcutPath) then begin
    if (ReceiptDesktopHash = '') or
       (CompareText(GetSHA256OfFile(DesktopShortcutPath),
         ReceiptDesktopHash) <> 0) then begin
      Reason := Reason + ' Desktop shortcut changed after uninstall preflight; left in place.';
    end else if not DeleteFile(DesktopShortcutPath) then begin
      Reason := Reason + ' Desktop shortcut could not be removed.';
    end;
  end;
  RemoveDir(StartMenuDirectory);
end;

function DirectoryContainsOnlyReceipt(const Root: String): Boolean;
var
  FindRecord: TFindRec;
  EntryPath, ReparseReason: String;
begin
  Result := False;
  if not DirExists(Root) then begin
    Result := True;
    Exit;
  end;
  EntryPath := AddBackslash(Root) + '*';
  if not FindFirst(EntryPath, FindRecord) then Exit;
  try
    repeat
      if (FindRecord.Name <> '.') and (FindRecord.Name <> '..') and
         (CompareText(FindRecord.Name, ReceiptFileName) <> 0) then Exit;
    until not FindNext(FindRecord);
    if HasReparseComponent(Root, ReparseReason) then Exit;
    Result := True;
  finally
    FindClose(FindRecord);
  end;
end;

procedure FinalizeUninstallRoot(const Root: String; var Reason: String);
var
  ReceiptReason, RestoreReason: String;
  ReceiptLines: TArrayOfString;
  ReceiptFile: String;
begin
  Reason := '';
  RemoveUninstallShortcuts(Root, ReceiptReason);
  if ReceiptReason <> '' then
    Reason := ReceiptReason;
  if DirExists(AddBackslash(Root) + 'docs') then
    RemoveDir(AddBackslash(Root) + 'docs');
  if not DirExists(Root) then Exit;
  ReceiptFile := ReceiptPath(Root);
  if not FileExists(ReceiptFile) then begin
    Reason := Reason + ' The validated former-ownership receipt is missing.';
    Exit;
  end;
  if not DirectoryContainsOnlyReceipt(Root) then begin
    Log('SDIR_UNINSTALL_FORMER_OWNER_RECEIPT_RETAINED');
    if Reason = '' then
      Reason := 'User data or unknown files remain; the validated ownership receipt was retained.';
    Exit;
  end;
  if not LoadStringsFromFile(ReceiptFile, ReceiptLines) then begin
    Reason := Reason + ' The ownership receipt could not be read for final cleanup.';
    Exit;
  end;
  if not DeleteFile(ReceiptFile) then begin
    Reason := Reason + ' The empty installation directory receipt could not be removed.';
    Exit;
  end;
  if not RemoveDir(Root) then begin
    RestoreReason := '';
    if not WriteReceiptLinesCreateNew(ReceiptFile, Root, ReceiptLines, False,
      RestoreReason) then
      Reason := Reason + ' The remaining directory could not be removed and its receipt could not be restored: ' +
        RestoreReason
    else
      Reason := Reason + ' The remaining directory was preserved with its former-ownership receipt.';
  end;
end;
