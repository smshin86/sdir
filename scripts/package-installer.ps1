[CmdletBinding()]
param(
    [string]$OutputPath,
    [switch]$TestBuild,
    [string]$TestRootName,
    [switch]$TestBaseline,
    [switch]$TestMissingRuntime,
    [ValidateSet('None', 'Backup', 'Partial', 'Late')]
    [string]$TestFault = 'None',
    [string]$TestProfileDirectory,
    [ValidateRange(0, 65535)]
    [int]$TestDebugPort = 0
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-FileSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-BytesSha256 {
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()][byte[]]$Bytes)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-', '').ToLowerInvariant()
    }
    finally {
        $sha.Dispose()
    }
}

function Initialize-CompilerCaptureHelper {
    if (-not ('Sdir.Installer.CompilerCapture' -as [type])) {
        Add-Type -Language CSharp -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Threading;
using System.Threading.Tasks;

namespace Sdir.Installer
{
    public sealed class CompilerChunk
    {
        public long Sequence;
        public long StreamSequence;
        public string StreamName;
        public byte[] Data;
    }

    public sealed class CompilerCaptureResult
    {
        public int ProcessId;
        public int ExitCode = -1;
        public bool HasExitCode;
        public string ProcessWaitError;
        public string DrainWaitError;
        public string StandardOutputDrainError;
        public string StandardErrorDrainError;
        public byte[] StandardOutput;
        public byte[] StandardError;
        public List<CompilerChunk> Chunks = new List<CompilerChunk>();
        public long StandardOutputChunkCount;
        public long StandardErrorChunkCount;
    }

    public sealed class CompilerLaunchException : Exception
    {
        public CompilerLaunchException(string message, Exception inner)
            : base(message, inner)
        {
        }
    }

    public static class CompilerCapture
    {
        private static void Drain(
            Stream stream,
            MemoryStream raw,
            string streamName,
            CompilerCaptureResult result,
            object journalLock)
        {
            byte[] buffer = new byte[8192];
            int read;
            while ((read = stream.Read(buffer, 0, buffer.Length)) > 0)
            {
                byte[] data = new byte[read];
                Buffer.BlockCopy(buffer, 0, data, 0, read);
                raw.Write(data, 0, data.Length);
                lock (journalLock)
                {
                    CompilerChunk chunk = new CompilerChunk();
                    chunk.Sequence = result.Chunks.Count;
                    chunk.StreamName = streamName;
                    if (streamName == "stdout")
                    {
                        chunk.StreamSequence = result.StandardOutputChunkCount++;
                    }
                    else
                    {
                        chunk.StreamSequence = result.StandardErrorChunkCount++;
                    }
                    chunk.Data = data;
                    result.Chunks.Add(chunk);
                }
            }
        }

        public static CompilerCaptureResult Run(
            string executable,
            string arguments,
            string workingDirectory)
        {
            ProcessStartInfo startInfo = new ProcessStartInfo();
            startInfo.FileName = executable;
            startInfo.Arguments = arguments;
            startInfo.WorkingDirectory = workingDirectory;
            startInfo.UseShellExecute = false;
            startInfo.CreateNoWindow = true;
            startInfo.RedirectStandardOutput = true;
            startInfo.RedirectStandardError = true;

            Process process = new Process();
            process.StartInfo = startInfo;
            try
            {
                if (!process.Start())
                {
                    throw new InvalidOperationException("Process.Start returned false.");
                }
            }
            catch (Exception ex)
            {
                process.Dispose();
                throw new CompilerLaunchException(
                    "Could not launch the native compiler: " + ex.ToString(), ex);
            }

            CompilerCaptureResult result = new CompilerCaptureResult();
            result.ProcessId = process.Id;
            MemoryStream stdout = new MemoryStream();
            MemoryStream stderr = new MemoryStream();
            object journalLock = new object();

            Task stdoutDrain = Task.Factory.StartNew(
                new Action(delegate
                {
                    try
                    {
                        Drain(process.StandardOutput.BaseStream, stdout, "stdout", result, journalLock);
                    }
                    catch (Exception ex)
                    {
                        lock (journalLock) { result.StandardOutputDrainError = ex.ToString(); }
                    }
                }),
                CancellationToken.None,
                TaskCreationOptions.LongRunning,
                TaskScheduler.Default);

            Task stderrDrain = Task.Factory.StartNew(
                new Action(delegate
                {
                    try
                    {
                        Drain(process.StandardError.BaseStream, stderr, "stderr", result, journalLock);
                    }
                    catch (Exception ex)
                    {
                        lock (journalLock) { result.StandardErrorDrainError = ex.ToString(); }
                    }
                }),
                CancellationToken.None,
                TaskCreationOptions.LongRunning,
                TaskScheduler.Default);

            try
            {
                process.WaitForExit();
            }
            catch (Exception ex)
            {
                result.ProcessWaitError = ex.ToString();
            }

            try
            {
                Task.WaitAll(new Task[] { stdoutDrain, stderrDrain });
            }
            catch (Exception ex)
            {
                result.DrainWaitError = ex.ToString();
            }

            try
            {
                result.ExitCode = process.ExitCode;
                result.HasExitCode = true;
            }
            catch (Exception ex)
            {
                result.ProcessWaitError = (result.ProcessWaitError ?? String.Empty) + ex.ToString();
            }

            result.StandardOutput = stdout.ToArray();
            result.StandardError = stderr.ToArray();
            stdout.Dispose();
            stderr.Dispose();
            process.Dispose();
            return result;
        }
    }
}
'@
    }
}

function ConvertTo-WindowsProcessArgument {
    param([AllowEmptyString()][string]$Argument)
    $builder = New-Object System.Text.StringBuilder
    [void]$builder.Append([char]34)
    $backslashCount = 0
    for ($index = 0; $index -lt $Argument.Length; $index++) {
        $character = $Argument[$index]
        if ($character -eq [char]92) {
            $backslashCount++
            continue
        }
        if ($character -eq [char]34) {
            for ($repeat = 0; $repeat -lt (($backslashCount * 2) + 1); $repeat++) {
                [void]$builder.Append([char]92)
            }
            [void]$builder.Append([char]34)
        }
        else {
            for ($repeat = 0; $repeat -lt $backslashCount; $repeat++) {
                [void]$builder.Append([char]92)
            }
            [void]$builder.Append($character)
        }
        $backslashCount = 0
    }
    for ($repeat = 0; $repeat -lt ($backslashCount * 2); $repeat++) {
        [void]$builder.Append([char]92)
    }
    [void]$builder.Append([char]34)
    return $builder.ToString()
}

function Get-CompilerDecodedText {
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()][byte[]]$Bytes)
    $offset = 0
    $encodingName = ''
    $encoding = $null
    if ($Bytes.Length -ge 4 -and $Bytes[0] -eq 0 -and $Bytes[1] -eq 0 -and
        $Bytes[2] -eq 254 -and $Bytes[3] -eq 255) {
        $encodingName = 'utf-32be'
        $encoding = New-Object System.Text.UTF32Encoding -ArgumentList $true, $true, $true
        $offset = 4
    }
    elseif ($Bytes.Length -ge 4 -and $Bytes[0] -eq 255 -and $Bytes[1] -eq 254 -and
        $Bytes[2] -eq 0 -and $Bytes[3] -eq 0) {
        $encodingName = 'utf-32le'
        $encoding = New-Object System.Text.UTF32Encoding -ArgumentList $false, $true, $true
        $offset = 4
    }
    elseif ($Bytes.Length -ge 3 -and $Bytes[0] -eq 239 -and $Bytes[1] -eq 187 -and $Bytes[2] -eq 191) {
        $encodingName = 'utf-8-bom'
        $encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false, $true
        $offset = 3
    }
    elseif ($Bytes.Length -ge 2 -and $Bytes[0] -eq 254 -and $Bytes[1] -eq 255) {
        $encodingName = 'utf-16be'
        $encoding = New-Object System.Text.UnicodeEncoding -ArgumentList $true, $true, $true
        $offset = 2
    }
    elseif ($Bytes.Length -ge 2 -and $Bytes[0] -eq 255 -and $Bytes[1] -eq 254) {
        $encodingName = 'utf-16le-bom'
        $encoding = New-Object System.Text.UnicodeEncoding -ArgumentList $false, $true, $true
        $offset = 2
    }
    else {
        try {
            $encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false, $true
            [void]$encoding.GetString($Bytes)
            $encodingName = 'utf-8'
        }
        catch [System.Text.DecoderFallbackException] {
            $encoding = [System.Text.Encoding]::Default
            $encodingName = 'windows-codepage-' + $encoding.CodePage
        }
    }
    return [pscustomobject]@{
        Encoding = $encodingName
        Text = $encoding.GetString($Bytes, $offset, $Bytes.Length - $offset)
        SerializedAs = 'utf-8-no-bom'
    }
}

function Write-CompilerCaptureEvidence {
    param(
        [Parameter(Mandatory = $true)][string]$Directory,
        [Parameter(Mandatory = $true)]$Capture
    )
    $stdout = [byte[]]$Capture.StandardOutput
    $stderr = [byte[]]$Capture.StandardError
    [System.IO.File]::WriteAllBytes((Join-Path $Directory 'compiler-stdout.raw'), $stdout)
    [System.IO.File]::WriteAllBytes((Join-Path $Directory 'compiler-stderr.raw'), $stderr)

    $stdoutDecoded = Get-CompilerDecodedText -Bytes $stdout
    $stderrDecoded = Get-CompilerDecodedText -Bytes $stderr
    [System.IO.File]::WriteAllText(
        (Join-Path $Directory ('compiler-stdout.decoded-' + $stdoutDecoded.Encoding + '.txt')),
        [string]$stdoutDecoded.Text,
        (New-Object System.Text.UTF8Encoding -ArgumentList $false))
    [System.IO.File]::WriteAllText(
        (Join-Path $Directory ('compiler-stderr.decoded-' + $stderrDecoded.Encoding + '.txt')),
        [string]$stderrDecoded.Text,
        (New-Object System.Text.UTF8Encoding -ArgumentList $false))

    $journalLines = New-Object System.Collections.Generic.List[string]
    foreach ($chunk in $Capture.Chunks) {
        $journalRecord = [ordered]@{
            collectorObservedSequence = [long]$chunk.Sequence
            stream = [string]$chunk.StreamName
            streamSequence = [long]$chunk.StreamSequence
            byteCount = [int]$chunk.Data.Length
            bytesBase64 = [Convert]::ToBase64String([byte[]]$chunk.Data)
        }
        [void]$journalLines.Add(($journalRecord | ConvertTo-Json -Compress))
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $Directory 'received-chunk-journal.jsonl'),
        (($journalLines.ToArray() -join [Environment]::NewLine) + [Environment]::NewLine),
        (New-Object System.Text.UTF8Encoding -ArgumentList $false))

    return [ordered]@{
        standardOutputEncoding = [string]$stdoutDecoded.Encoding
        standardErrorEncoding = [string]$stderrDecoded.Encoding
        decodedTextSerialization = 'utf-8-no-bom'
        standardOutputBytes = $stdout.Length
        standardErrorBytes = $stderr.Length
        standardOutputChunks = [long]$Capture.StandardOutputChunkCount
        standardErrorChunks = [long]$Capture.StandardErrorChunkCount
        processId = [long]$Capture.ProcessId
        crossStreamOrdering = 'collector-observed lock acquisition sequence only; producer ordering is not asserted'
        stdoutDrainError = $Capture.StandardOutputDrainError
        stderrDrainError = $Capture.StandardErrorDrainError
        drainWaitError = $Capture.DrainWaitError
        processWaitError = $Capture.ProcessWaitError
        processExitCode = if ($Capture.HasExitCode) { [int]$Capture.ExitCode } else { $null }
    }
}

function Resolve-FullPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location).Path $Path))
}

function Assert-OwnedStage {
    param([string]$Path, [string]$StageId)
    $markerPath = Join-Path $Path '.sdir-installer-owned-stage.json'
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        throw 'The staging directory has no ownership marker; it was preserved.'
    }
    $marker = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
    if ($marker.stageId -ne $StageId -or
        [System.IO.Path]::GetFullPath($marker.stagePath) -ne [System.IO.Path]::GetFullPath($Path) -or
        [System.IO.Path]::GetFullPath($marker.ownerScript) -ne [System.IO.Path]::GetFullPath($PSCommandPath)) {
        throw 'The staging directory ownership marker did not match; it was preserved.'
    }
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$installerRoot = Join-Path $repoRoot 'packaging\windows-installer'
$manifestPath = Join-Path $installerRoot 'payload.json'
$readmePath = Join-Path $installerRoot 'README-ko.txt'
$setupSourcePath = Join-Path $installerRoot 'SDIR.iss'
$rollbackSourcePath = Join-Path $installerRoot 'rollback.iss'
$compilerMetadataPath = Join-Path $repoRoot 'docs\task-id\windows-installer\compiler.json'
$portablePath = Join-Path $repoRoot 'dist\SDIR-windows-x64-portable.zip'
$stableExePath = Join-Path $repoRoot 'dist\SDIR.exe'
$baselineExePath = Join-Path $repoRoot 'docs\task-id\performance-optimize\baseline\SDIR-baseline.exe'
$profileMarkerName = '.sdir-installer-test-profile'
$profileMarkerValue = 'SDIR-INSTALLER-UAT-PROFILE-V1'
$testAppId = 'SDIR-UAT-20261117-9E39A20D-72E4-4D1D-AC7E-E65F4E2F1861'
$testAppName = 'SDIR UAT'
$testGroupName = 'SDIR UAT'
$stableAppHash = '766bf2aaf832eba9812b0d97f7dd207c0278ae9bd8c332de054548cbf6719740'
$stablePortableHash = '6aa32159140ce9ed0ff1bb06a486651290cb0e74df7e7dd37104ae720fcea4d5'
$baselineAppHash = 'b158955e2b1c66883c7240ad1d7d77341d7934c1b881100c3eb726101313f8fb'
$stableAppBytes = [long]9326592
$baselineAppBytes = [long]9055744
$legacyTestFileName = 'legacy-test-only.txt'
$legacyTestFileBytes = [System.Text.Encoding]::ASCII.GetBytes("SDIR TEST BASELINE B158`r`n")
$legacyTestFileHash = 'ea35e4c2c9646c4b489140707c1b87963d7f7912c85a6fc2e914b490fa818607'
$productionInventoryId = 'production-stable766-v1'
$testStableInventoryId = 'test-stable766-v1'
$testBaselineInventoryId = 'test-baselineb158-v1'
$testOptionNames = @('TestRootName', 'TestBaseline', 'TestMissingRuntime',
    'TestFault', 'TestProfileDirectory', 'TestDebugPort')
$testOptionSpecified = $false
foreach ($testOptionName in $testOptionNames) {
    if ($PSBoundParameters.ContainsKey($testOptionName)) {
        $testOptionSpecified = $true
        break
    }
}

if (-not $TestBuild -and $testOptionSpecified) {
    throw 'TestRootName, test hooks, baseline payload, and isolated profile options require -TestBuild.'
}
if ($TestBuild) {
    if (-not $TestRootName -or -not $TestProfileDirectory) {
        throw '-TestBuild requires -TestRootName and a marked -TestProfileDirectory.'
    }
    if ($TestRootName -notmatch '^SDIR-UAT-[A-Za-z0-9][A-Za-z0-9._-]{0,63}$' -or
        $TestRootName.EndsWith('.') -or $TestRootName.EndsWith(' ')) {
        throw 'TestRootName must be a unique valid leaf beginning with SDIR-UAT-.'
    }
    if (($TestDebugPort -ne 0) -and ($TestDebugPort -lt 1024)) {
        throw 'TestDebugPort must be 0 or a port in the range 1024 through 65535.'
    }
    $profileFullPath = Resolve-FullPath $TestProfileDirectory
    if (-not [System.IO.Path]::IsPathRooted($profileFullPath) -or
        $profileFullPath -notmatch '^[A-Za-z]:\\[A-Za-z0-9 _.-]+(?:\\[A-Za-z0-9 _.-]+)*$') {
        throw 'TestProfileDirectory must be an absolute local path using characters safe for the generated test launcher.'
    }
    if (-not (Test-Path -LiteralPath $profileFullPath -PathType Container)) {
        throw 'TestProfileDirectory must already exist and belong to the test harness.'
    }
    $profileMarkerPath = Join-Path $profileFullPath $profileMarkerName
    if (-not (Test-Path -LiteralPath $profileMarkerPath -PathType Leaf)) {
        throw ('TestProfileDirectory is missing the test-harness marker {0}.' -f $profileMarkerName)
    }
    $profileItem = Get-Item -LiteralPath $profileFullPath -Force
    $markerItem = Get-Item -LiteralPath $profileMarkerPath -Force
    if (($profileItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        ($markerItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'TestProfileDirectory and its ownership marker must not be reparse points.'
    }
    $markerText = [System.IO.File]::ReadAllText($profileMarkerPath).Trim()
    if ($markerText -ne $profileMarkerValue) {
        throw ('TestProfileDirectory marker must contain exactly {0}.' -f $profileMarkerValue)
    }
}
else {
    $profileFullPath = ''
}

if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw ('Payload manifest not found: {0}' -f $manifestPath)
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.schemaVersion -ne 1 -or $manifest.appSha256 -ne $stableAppHash -or
    $manifest.portableSha256 -ne $stablePortableHash -or $manifest.files.Count -ne 18) {
    throw 'payload.json does not describe the reviewed stable 18-file payload.'
}
if ((Get-Item -LiteralPath $stableExePath).Length -ne $stableAppBytes -or
    (Get-FileSha256 $stableExePath) -ne $stableAppHash) {
    throw 'The stable dist/SDIR.exe hash changed; the installer candidate was not built.'
}
if ((Get-FileSha256 $portablePath) -ne $stablePortableHash) {
    throw 'The stable portable ZIP hash changed; the installer candidate was not built.'
}
if ($TestBaseline) {
    if (-not (Test-Path -LiteralPath $baselineExePath -PathType Leaf) -or
        (Get-Item -LiteralPath $baselineExePath).Length -ne $baselineAppBytes -or
        (Get-FileSha256 $baselineExePath) -ne $baselineAppHash) {
        throw 'TestBaseline requires the exact reviewed historical test executable.'
    }
}

$destinationNames = @{}
foreach ($entry in $manifest.files) {
    if (-not $entry.source.StartsWith('dist/', [System.StringComparison]::Ordinal) -or
        $entry.source -notmatch '^dist/[A-Za-z0-9._/-]+$' -or
        $entry.source.Contains('//') -or
        $entry.source -match '(^|/)\.\.(/|$)' -or
        $entry.destination -notmatch '^[A-Za-z0-9._/-]+$' -or
        $entry.destination.Contains('//') -or $entry.destination.EndsWith('/') -or
        $entry.destination -match '(^|/)\.(/|$)' -or
        $entry.destination -match '(^|[\\/])\.\.([\\/]|$)' -or
        $entry.destination -match '^[\\/]' -or
        $entry.destination -match '[<>:"|?*]') {
        throw ('Unsafe payload descriptor path: {0}' -f $entry.source)
    }
    if ($destinationNames.ContainsKey([string]$entry.destination)) {
        throw ('Duplicate payload destination: {0}' -f $entry.destination)
    }
    $destinationNames[[string]$entry.destination] = $true
    if ($entry.destination -match '(^|/)portable\.flag$|(^|/)data(/|$)|search-settings\.md') {
        throw ('A private or unsupported path appeared in the payload descriptor: {0}' -f $entry.destination)
    }
    $sourcePath = Join-Path $repoRoot ($entry.source -replace '/', '\')
    if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
        throw ('Payload source is missing: {0}' -f $entry.source)
    }
    if (-not $TestBaseline -or $entry.destination -ne 'SDIR.exe') {
        $sourceInfo = Get-Item -LiteralPath $sourcePath
        if ($sourceInfo.Length -ne [long]$entry.bytes -or
            (Get-FileSha256 $sourcePath) -ne $entry.sha256) {
            throw ('Stable payload source changed: {0}' -f $entry.source)
        }
    }
}

$compilerMetadata = Get-Content -LiteralPath $compilerMetadataPath -Raw | ConvertFrom-Json
if ($compilerMetadata.version -ne '6.7.4' -or -not $compilerMetadata.installedPath) {
    throw 'compiler.json does not identify the installed Inno Setup 6.7.4 compiler.'
}
$compilerPath = Join-Path $compilerMetadata.installedPath 'ISCC.exe'
if (-not [System.IO.Path]::IsPathRooted($compilerPath) -or
    -not (Test-Path -LiteralPath $compilerPath -PathType Leaf)) {
    throw ('The installed ISCC.exe from compiler.json is unavailable: {0}' -f $compilerPath)
}
$compilerHash = Get-FileSha256 $compilerPath

if (-not $OutputPath) {
    $candidateName = 'SDIR-windows-x64-setup.candidate-{0}.exe' -f [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
    if ($TestBuild) {
        $candidateName = 'SDIR-windows-x64-setup-test-{0}-{1}.exe' -f $TestRootName, [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
    }
    $OutputPath = Join-Path (Join-Path $repoRoot 'dist\installer-candidates') $candidateName
}
$OutputPath = Resolve-FullPath $OutputPath
$outputDirectory = Split-Path -Parent $OutputPath
$productionInstallerPath = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'dist\SDIR-windows-x64-setup.exe'))
$protectedOutputPaths = @(
    $productionInstallerPath,
    [System.IO.Path]::GetFullPath($stableExePath),
    [System.IO.Path]::GetFullPath($portablePath)
)
foreach ($protectedPath in $protectedOutputPaths) {
    if ([System.IO.Path]::GetFullPath($OutputPath) -eq $protectedPath) {
        throw ('OutputPath may not overwrite a production or stable distribution artifact: {0}' -f $OutputPath)
    }
}
if ([System.IO.Path]::GetExtension($OutputPath) -ne '.exe') {
    throw 'OutputPath must end in .exe.'
}
if (Test-Path -LiteralPath $OutputPath) {
    throw ('OutputPath already exists and will not be overwritten: {0}' -f $OutputPath)
}
$receiptPath = $OutputPath + '.build.json'
if (Test-Path -LiteralPath $receiptPath) {
    throw ('The build receipt already exists and will not be overwritten: {0}' -f $receiptPath)
}
if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) {
    New-Item -ItemType Directory -Path $outputDirectory | Out-Null
}
$compilerEvidenceDirectory = Join-Path $outputDirectory (
    [System.IO.Path]::GetFileNameWithoutExtension($OutputPath) + '.compiler-evidence')
if (Test-Path -LiteralPath $compilerEvidenceDirectory) {
    throw ('Compiler evidence directory already exists and will not be overwritten: {0}' -f $compilerEvidenceDirectory)
}
New-Item -ItemType Directory -Path $compilerEvidenceDirectory | Out-Null

$stageId = [Guid]::NewGuid().ToString('N')
$stagePath = Join-Path $outputDirectory ('.sdir-installer-owned-stage-' + $stageId)
New-Item -ItemType Directory -Path $stagePath | Out-Null
$stageMarker = [ordered]@{
    stageId = $stageId
    stagePath = [System.IO.Path]::GetFullPath($stagePath)
    ownerScript = [System.IO.Path]::GetFullPath($PSCommandPath)
    createdUtc = [DateTime]::UtcNow.ToString('o')
}
$utf8NoBom = New-Object System.Text.UTF8Encoding -ArgumentList $false
[System.IO.File]::WriteAllText((Join-Path $stagePath '.sdir-installer-owned-stage.json'),
    ($stageMarker | ConvertTo-Json -Depth 5), $utf8NoBom)

$compileSucceeded = $false
try {
    $payloadStage = Join-Path $stagePath 'payload'
    New-Item -ItemType Directory -Path $payloadStage | Out-Null
    $inventory = New-Object System.Collections.ArrayList
    $allSourceHashes = [ordered]@{}
    $manifestSourceHash = Get-FileSha256 $manifestPath
    $allSourceHashes['packaging/windows-installer/payload.json'] = $manifestSourceHash
    $allSourceHashes['packaging/windows-installer/README-ko.txt'] = Get-FileSha256 $readmePath
    $allSourceHashes['packaging/windows-installer/SDIR.iss'] = Get-FileSha256 $setupSourcePath
    $allSourceHashes['packaging/windows-installer/rollback.iss'] = Get-FileSha256 $rollbackSourcePath
    $allSourceHashes['scripts/package-installer.ps1'] = Get-FileSha256 $PSCommandPath
    $allSourceHashes['docs/task-id/windows-installer/compiler.json'] = Get-FileSha256 $compilerMetadataPath
    $allSourceHashes['dist/SDIR-windows-x64-portable.zip'] = Get-FileSha256 $portablePath
    foreach ($entry in $manifest.files) {
        $stableSourcePath = Join-Path $repoRoot ([string]$entry.source -replace '/', '\')
        $allSourceHashes[[string]$entry.source] = Get-FileSha256 $stableSourcePath
    }

    foreach ($entry in $manifest.files) {
        $sourceRelative = $entry.source
        $sourcePath = Join-Path $repoRoot ($sourceRelative -replace '/', '\')
        $selectedPath = $sourcePath
        $expectedHash = [string]$entry.sha256
        if ($TestBaseline -and $entry.destination -eq 'SDIR.exe') {
            $selectedPath = $baselineExePath
            $sourceRelative = 'docs/task-id/performance-optimize/baseline/SDIR-baseline.exe'
            $expectedHash = $baselineAppHash
        }
        $destinationRelative = [string]$entry.destination -replace '/', '\'
        $destinationPath = Join-Path $payloadStage $destinationRelative
        $destinationParent = Split-Path -Parent $destinationPath
        if (-not (Test-Path -LiteralPath $destinationParent -PathType Container)) {
            New-Item -ItemType Directory -Path $destinationParent -Force | Out-Null
        }
        Copy-Item -LiteralPath $selectedPath -Destination $destinationPath
        $actualHash = Get-FileSha256 $destinationPath
        if ($actualHash -ne $expectedHash) {
            throw ('Staged payload hash mismatch: {0}' -f $entry.destination)
        }
        $fileInfo = Get-Item -LiteralPath $destinationPath
        [void]$inventory.Add([ordered]@{
            source = $sourceRelative
            destination = [string]$entry.destination
            bytes = [long]$fileInfo.Length
            sha256 = $actualHash
        })
        $allSourceHashes[$sourceRelative] = Get-FileSha256 $selectedPath
    }

    if ($TestBuild -and $TestBaseline) {
        if ($legacyTestFileBytes.Length -ne 25 -or
            (Get-BytesSha256 $legacyTestFileBytes) -ne $legacyTestFileHash) {
            throw 'The compiled legacy test fixture bytes do not match the reviewed 25-byte identity.'
        }
        $legacyPath = Join-Path $payloadStage $legacyTestFileName
        [System.IO.File]::WriteAllBytes($legacyPath, $legacyTestFileBytes)
        $legacyInfo = Get-Item -LiteralPath $legacyPath
        $legacyActualHash = Get-FileSha256 $legacyPath
        if ($legacyInfo.Length -ne 25 -or $legacyActualHash -ne $legacyTestFileHash) {
            throw 'The staged legacy test fixture does not match its reviewed size and hash.'
        }
        $legacySource = 'generated/test-baseline/' + $legacyTestFileName
        [void]$inventory.Add([ordered]@{
            source = $legacySource
            destination = $legacyTestFileName
            bytes = [long]$legacyInfo.Length
            sha256 = $legacyActualHash
        })
        $allSourceHashes[$legacySource] = $legacyActualHash
    }

    if ($TestBuild) {
        $launcherName = 'sdir-installer-test-launcher.cmd'
        $launcherPath = Join-Path $payloadStage $launcherName
        $launcherLines = New-Object System.Collections.Generic.List[string]
        [void]$launcherLines.Add('@echo off')
        [void]$launcherLines.Add('setlocal DisableDelayedExpansion')
        [void]$launcherLines.Add(('set "SDIR_PROFILE_DIR={0}"' -f $profileFullPath))
        if ($TestDebugPort -gt 0) {
            [void]$launcherLines.Add(('set "SDIR_DEBUG_PORT={0}"' -f $TestDebugPort))
        }
        [void]$launcherLines.Add('"%~dp0SDIR.exe" %*')
        [void]$launcherLines.Add('exit /b %errorlevel%')
        [System.IO.File]::WriteAllText($launcherPath, ($launcherLines -join "`r`n") + "`r`n", [System.Text.Encoding]::ASCII)
        $launcherHash = Get-FileSha256 $launcherPath
        $launcherInfo = Get-Item -LiteralPath $launcherPath
        [void]$inventory.Add([ordered]@{
            source = 'generated/test-build/sdir-installer-test-launcher.cmd'
            destination = $launcherName
            bytes = [long]$launcherInfo.Length
            sha256 = $launcherHash
        })
        $allSourceHashes['generated/test-build/sdir-installer-test-launcher.cmd'] = $launcherHash
    }

    $inventoryArray = @($inventory.ToArray())
    $acceptedInventoryRows = New-Object System.Collections.ArrayList
    if ($TestBuild) {
        $launcherEntry = $inventoryArray | Where-Object { $_.destination -eq 'sdir-installer-test-launcher.cmd' } | Select-Object -First 1
        if (-not $launcherEntry) {
            throw 'The TEST inventory is missing its compiled launcher row.'
        }
        foreach ($entry in $manifest.files) {
            $stableBytes = [long]$entry.bytes
            $stableHash = [string]$entry.sha256
            if ($entry.destination -eq 'SDIR.exe') {
                $stableBytes = $stableAppBytes
                $stableHash = $stableAppHash
            }
            [void]$acceptedInventoryRows.Add([ordered]@{
                InventoryId = $testStableInventoryId
                Destination = [string]$entry.destination
                Bytes = [long]$stableBytes
                SHA256 = $stableHash
            })
        }
        [void]$acceptedInventoryRows.Add([ordered]@{
            InventoryId = $testStableInventoryId
            Destination = [string]$launcherEntry.destination
            Bytes = [long]$launcherEntry.bytes
            SHA256 = [string]$launcherEntry.sha256
        })

        foreach ($entry in $manifest.files) {
            $baselineBytes = [long]$entry.bytes
            $baselineHash = [string]$entry.sha256
            if ($entry.destination -eq 'SDIR.exe') {
                $baselineBytes = $baselineAppBytes
                $baselineHash = $baselineAppHash
            }
            [void]$acceptedInventoryRows.Add([ordered]@{
                InventoryId = $testBaselineInventoryId
                Destination = [string]$entry.destination
                Bytes = [long]$baselineBytes
                SHA256 = $baselineHash
            })
        }
        [void]$acceptedInventoryRows.Add([ordered]@{
            InventoryId = $testBaselineInventoryId
            Destination = $legacyTestFileName
            Bytes = [long]$legacyTestFileBytes.Length
            SHA256 = $legacyTestFileHash
        })
        [void]$acceptedInventoryRows.Add([ordered]@{
            InventoryId = $testBaselineInventoryId
            Destination = [string]$launcherEntry.destination
            Bytes = [long]$launcherEntry.bytes
            SHA256 = [string]$launcherEntry.sha256
        })
        $activeInventoryId = if ($TestBaseline) { $testBaselineInventoryId } else { $testStableInventoryId }
    }
    else {
        foreach ($entry in $manifest.files) {
            [void]$acceptedInventoryRows.Add([ordered]@{
                InventoryId = $productionInventoryId
                Destination = [string]$entry.destination
                Bytes = [long]$entry.bytes
                SHA256 = [string]$entry.sha256
            })
        }
        $activeInventoryId = $productionInventoryId
    }
    $acceptedInventoryArray = @($acceptedInventoryRows.ToArray())
    $expectedActiveCount = if ($TestBuild -and $TestBaseline) { 20 } elseif ($TestBuild) { 19 } else { 18 }
    if ($inventoryArray.Count -ne $expectedActiveCount -or
        ($TestBuild -and $acceptedInventoryArray.Count -ne 39) -or
        (-not $TestBuild -and $acceptedInventoryArray.Count -ne 18)) {
        throw 'The extracted payload or accepted compiled inventory has an unexpected row count.'
    }
    $manifestStableEntry = $manifest.files | Where-Object { $_.destination -eq 'SDIR.exe' } | Select-Object -First 1
    $embeddedAppEntry = $inventoryArray | Where-Object { $_.destination -eq 'SDIR.exe' } | Select-Object -First 1
    if (-not $embeddedAppEntry -or
        (-not $manifestStableEntry -or [long]$manifestStableEntry.bytes -ne $stableAppBytes) -or
        ($TestBaseline -and ($embeddedAppEntry.sha256 -ne $baselineAppHash -or
            [long]$embeddedAppEntry.bytes -ne $baselineAppBytes)) -or
        (-not $TestBaseline -and ($embeddedAppEntry.sha256 -ne $stableAppHash -or $embeddedAppEntry.bytes -ne [long]$manifestStableEntry.bytes))) {
        throw 'The staged SDIR.exe does not match its declared stable or test-baseline identity.'
    }

    $manifestInventory = @($inventoryArray | Sort-Object destination | ForEach-Object {
        [ordered]@{ destination = $_.destination; bytes = $_.bytes; sha256 = $_.sha256 }
    })
    $inventoryJson = ConvertTo-Json -InputObject $manifestInventory -Depth 6 -Compress
    $inventoryHash = Get-BytesSha256 ($utf8NoBom.GetBytes($inventoryJson))
    $acceptedInventoryJson = ConvertTo-Json -InputObject $acceptedInventoryArray -Depth 6 -Compress
    $acceptedInventoryHash = Get-BytesSha256 ($utf8NoBom.GetBytes($acceptedInventoryJson))

    $sourceTextEncoding = New-Object System.Text.UTF8Encoding -ArgumentList $true
    $sourcePairs = @(
        [pscustomobject]@{ Source = $setupSourcePath; Name = 'SDIR.iss' },
        [pscustomobject]@{ Source = $rollbackSourcePath; Name = 'rollback.iss' }
    )
    foreach ($sourcePair in $sourcePairs) {
        $sourceFile = [string]$sourcePair.Source
        $stageFile = Join-Path $stagePath ([string]$sourcePair.Name)
        $sourceText = [System.IO.File]::ReadAllText($sourceFile)
        [System.IO.File]::WriteAllText($stageFile, $sourceText, $sourceTextEncoding)
    }
    $readmeText = [System.IO.File]::ReadAllText($readmePath)
    if ($TestBuild) {
        $readmeText = 'TEST BUILD: isolated SDIR-UAT installation at %LOCALAPPDATA%\Programs\' +
            $TestRootName + [Environment]::NewLine + [Environment]::NewLine +
            $readmeText.Replace(
                '%LOCALAPPDATA%\Programs\SDIR',
                '%LOCALAPPDATA%\Programs\' + $TestRootName)
    }
    [System.IO.File]::WriteAllText((Join-Path $stagePath 'README-ko.txt'), $readmeText, $sourceTextEncoding)

    $payloadPaths = @($inventoryArray | ForEach-Object { [string]$_.destination })
    $rootName = if ($TestBuild) { $TestRootName } else { 'SDIR' }
    $appId = if ($TestBuild) { $testAppId } else { 'SDIR-PerUser-4E7E0A18-79E0-41C0-97C5-65A10A79B3F4' }
    $appName = if ($TestBuild) { $testAppName } else { 'SDIR' }
    $groupName = if ($TestBuild) { $testGroupName } else { 'SDIR' }
    $defaultDirectory = '{localappdata}\Programs\' + $rootName
    $testProfileForCode = if ($TestBuild) { $profileFullPath } else { '' }
    $testDebugPortForCode = if ($TestBuild) { [string]$TestDebugPort } else { '0' }
    $payloadConstantLines = New-Object System.Collections.Generic.List[string]
    [void]$payloadConstantLines.Add('#ifdef PAYLOAD_CONSTANTS_CODE')
    [void]$payloadConstantLines.Add('const')
    [void]$payloadConstantLines.Add(('  PayloadCount = {0};' -f $payloadPaths.Count))
    [void]$payloadConstantLines.Add(('  InventoryRowCount = {0};' -f $acceptedInventoryArray.Count))
    [void]$payloadConstantLines.Add(("  ActiveInventoryId = '{0}';" -f $activeInventoryId))
    [void]$payloadConstantLines.Add(('  TestProfileDirectoryText = ''{0}'';' -f $testProfileForCode))
    [void]$payloadConstantLines.Add(('  TestDebugPortText = ''{0}'';' -f $testDebugPortForCode))
    [void]$payloadConstantLines.Add('#else')
    [void]$payloadConstantLines.Add(('#define TestAppIdGuardValue "{0}"' -f $appId))
    [void]$payloadConstantLines.Add(('#define TestAppNameGuardValue "{0}"' -f $appName))
    [void]$payloadConstantLines.Add(('#define TestGroupNameGuardValue "{0}"' -f $groupName))
    [void]$payloadConstantLines.Add(('#define TestRootNameGuardValue "{0}"' -f $rootName))
    [void]$payloadConstantLines.Add(('#define TestDefaultDirGuardValue "{0}"' -f $defaultDirectory))
    [void]$payloadConstantLines.Add(('#define AppIdValue "{0}"' -f $appId))
    [void]$payloadConstantLines.Add(('#define AppNameValue "{0}"' -f $appName))
    [void]$payloadConstantLines.Add(('#define GroupNameValue "{0}"' -f $groupName))
    [void]$payloadConstantLines.Add(('#define RootNameValue "{0}"' -f $rootName))
    [void]$payloadConstantLines.Add(('#define DefaultDirValue "{0}"' -f $defaultDirectory))
    [void]$payloadConstantLines.Add('#endif')
    [System.IO.File]::WriteAllLines((Join-Path $stagePath 'payload-constants.iss'), $payloadConstantLines.ToArray(), [System.Text.Encoding]::ASCII)

    $payloadInitializeLines = New-Object System.Collections.Generic.List[string]
    for ($index = 0; $index -lt $inventoryArray.Count; $index++) {
        $entry = $inventoryArray[$index]
        [void]$payloadInitializeLines.Add(
            ("  PayloadPaths[{0}] := '{1}';" -f $index, [string]$entry.destination))
        [void]$payloadInitializeLines.Add(
            ("  PayloadBytes[{0}] := {1};" -f $index, [long]$entry.bytes))
        [void]$payloadInitializeLines.Add(
            ("  PayloadHashes[{0}] := '{1}';" -f $index, [string]$entry.sha256))
    }
    for ($index = 0; $index -lt $acceptedInventoryArray.Count; $index++) {
        $entry = $acceptedInventoryArray[$index]
        [void]$payloadInitializeLines.Add(
            ("  AcceptedInventoryIds[{0}] := '{1}';" -f $index, [string]$entry.InventoryId))
        [void]$payloadInitializeLines.Add(
            ("  AcceptedInventoryPaths[{0}] := '{1}';" -f $index, [string]$entry.Destination))
        [void]$payloadInitializeLines.Add(
            ("  AcceptedInventoryBytes[{0}] := {1};" -f $index, [long]$entry.Bytes))
        [void]$payloadInitializeLines.Add(
            ("  AcceptedInventoryHashes[{0}] := '{1}';" -f $index, [string]$entry.SHA256))
    }
    [System.IO.File]::WriteAllLines(
        (Join-Path $stagePath 'payload-initialize.iss'),
        $payloadInitializeLines.ToArray(),
        [System.Text.Encoding]::ASCII)

    $payloadFileLines = New-Object System.Collections.Generic.List[string]
    for ($index = 0; $index -lt $inventoryArray.Count; $index++) {
        $relativeDestination = ([string]$inventoryArray[$index].destination) -replace '/', '\'
        $source = 'payload\' + $relativeDestination
        $relativeDirectory = Split-Path -Parent $relativeDestination
        $destDirectory = if ($relativeDirectory) { '{app}\' + $relativeDirectory } else { '{app}' }
        $destName = Split-Path -Leaf $relativeDestination
        $callback = if ($index -eq ($inventoryArray.Count - 1)) { 'FinishPayloadInstall' } else { 'PayloadFileInstalled' }
        [void]$payloadFileLines.Add(('Source: "{0}"; DestDir: "{1}"; DestName: "{2}"; Flags: ignoreversion; AfterInstall: {3}' -f $source, $destDirectory, $destName, $callback))
    }
    [System.IO.File]::WriteAllLines((Join-Path $stagePath 'payload-files.iss'), $payloadFileLines.ToArray(), [System.Text.Encoding]::ASCII)

    $generatedInputDirectory = Join-Path $compilerEvidenceDirectory 'generated-inputs'
    New-Item -ItemType Directory -Path $generatedInputDirectory | Out-Null
    $generatedInputNames = @(
        'SDIR.iss',
        'rollback.iss',
        'payload-constants.iss',
        'payload-initialize.iss',
        'payload-files.iss',
        'README-ko.txt'
    )
    foreach ($generatedInputName in $generatedInputNames) {
        $generatedInputPath = Join-Path $stagePath $generatedInputName
        $generatedInputHash = Get-FileSha256 $generatedInputPath
        Copy-Item -LiteralPath $generatedInputPath -Destination (Join-Path $generatedInputDirectory $generatedInputName)
        $allSourceHashes['staged/' + $generatedInputName] = $generatedInputHash
    }
    $sourceHashEvidence = [ordered]@{
        generatedUtc = [DateTime]::UtcNow.ToString('o')
        sourceHashes = $allSourceHashes
        payloadInventory = $inventoryArray
        payloadInventorySha256 = $inventoryHash
        activeInventoryId = $activeInventoryId
        acceptedCompiledInventories = $acceptedInventoryArray
        acceptedCompiledInventoriesSha256 = $acceptedInventoryHash
        stableAppSha256 = $stableAppHash
        stablePortableSha256 = $stablePortableHash
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $compilerEvidenceDirectory 'source-hashes.json'),
        ($sourceHashEvidence | ConvertTo-Json -Depth 12),
        $utf8NoBom)

    try {
        Initialize-CompilerCaptureHelper
    }
    catch {
        $helperCompilationEvidence = [ordered]@{
            completedUtc = [DateTime]::UtcNow.ToString('o')
            helperSourcePath = [System.IO.Path]::GetFullPath($PSCommandPath)
            helperSourceSha256 = Get-FileSha256 $PSCommandPath
            helperCompilationError = $_.Exception.ToString()
        }
        [System.IO.File]::WriteAllText(
            (Join-Path $compilerEvidenceDirectory 'compiler-capture-helper-error.json'),
            ($helperCompilationEvidence | ConvertTo-Json -Depth 5),
            $utf8NoBom)
        throw
    }

    $buildDefines = New-Object System.Collections.Generic.List[string]
    if ($TestBuild) {
        [void]$buildDefines.Add('/DTEST_BUILD')
        switch ($TestFault) {
            'Backup' { [void]$buildDefines.Add('/DTEST_FAULT_BACKUP') }
            'Partial' { [void]$buildDefines.Add('/DTEST_FAULT_PARTIAL') }
            'Late' { [void]$buildDefines.Add('/DTEST_FAULT_LATE') }
        }
        if ($TestMissingRuntime) {
            [void]$buildDefines.Add('/DTEST_MISSING_RUNTIME')
        }
    }

    $stageOutput = Join-Path $stagePath 'output'
    New-Item -ItemType Directory -Path $stageOutput | Out-Null
    $buildBaseName = 'SDIR-windows-x64-setup'
    if ($TestBuild) { $buildBaseName = 'SDIR-windows-x64-setup-test-' + $TestRootName }
    $compilerArguments = New-Object System.Collections.Generic.List[string]
    [void]$compilerArguments.Add('/O' + $stageOutput)
    [void]$compilerArguments.Add('/F' + $buildBaseName)
    foreach ($define in $buildDefines) { [void]$compilerArguments.Add($define) }
    [void]$compilerArguments.Add('SDIR.iss')

    $compilerArgumentArray = $compilerArguments.ToArray()
    $quotedCompilerArguments = @(
        foreach ($compilerArgument in $compilerArgumentArray) {
            ConvertTo-WindowsProcessArgument -Argument ([string]$compilerArgument)
        }
    )
    $compilerCommandLine = $quotedCompilerArguments -join ' '
    $compilerStartedUtc = [DateTime]::UtcNow.ToString('o')
    $compilerCapture = $null
    $compilerLaunchError = $null
    $compilerCaptureError = $null
    $compilerExitCode = $null
    try {
        $captureType = 'Sdir.Installer.CompilerCapture' -as [type]
        if (-not $captureType) {
            throw 'The native compiler capture helper type is unavailable.'
        }
        $compilerCapture = $captureType::Run(
            $compilerPath,
            $compilerCommandLine,
            $stagePath)
    }
    catch {
        $exception = $_.Exception
        $launchException = $false
        while ($exception) {
            if ($exception.GetType().FullName -eq 'Sdir.Installer.CompilerLaunchException') {
                $launchException = $true
                break
            }
            $exception = $exception.InnerException
        }
        if ($launchException) {
            $compilerLaunchError = $_.Exception.ToString()
        }
        else {
            $compilerCaptureError = $_.Exception.ToString()
        }
    }

    $captureEvidence = $null
    if ($compilerCapture) {
        if ($compilerCapture.HasExitCode) {
            $compilerExitCode = [int]$compilerCapture.ExitCode
        }
        if ($compilerCapture.ProcessWaitError -or $compilerCapture.DrainWaitError -or
            $compilerCapture.StandardOutputDrainError -or $compilerCapture.StandardErrorDrainError) {
            $drainCaptureError = @(
                $compilerCapture.ProcessWaitError,
                $compilerCapture.DrainWaitError,
                $compilerCapture.StandardOutputDrainError,
                $compilerCapture.StandardErrorDrainError
            ) -join [Environment]::NewLine
            if ($compilerCaptureError) {
                $compilerCaptureError += [Environment]::NewLine + $drainCaptureError
            }
            else {
                $compilerCaptureError = $drainCaptureError
            }
        }
    }

    $compilerRunEvidence = [ordered]@{
        startedUtc = $compilerStartedUtc
        completedUtc = [DateTime]::UtcNow.ToString('o')
        executable = $compilerPath
        workingDirectory = $stagePath
        processId = if ($compilerCapture) { [long]$compilerCapture.ProcessId } else { $null }
        arguments = @($compilerArgumentArray)
        commandLine = $compilerCommandLine
        launchError = $compilerLaunchError
        captureError = $compilerCaptureError
        nativeExitCode = $compilerExitCode
        outputCapture = $captureEvidence
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $compilerEvidenceDirectory 'compiler-invocation.json'),
        ($compilerRunEvidence | ConvertTo-Json -Depth 12),
        $utf8NoBom)

    if ($compilerCapture) {
        try {
            $captureEvidence = Write-CompilerCaptureEvidence -Directory $compilerEvidenceDirectory -Capture $compilerCapture
        }
        catch {
            $captureHelperError = $_.Exception.ToString()
            if ($compilerCaptureError) {
                $compilerCaptureError += [Environment]::NewLine + $captureHelperError
            }
            else {
                $compilerCaptureError = $captureHelperError
            }
        }
        $compilerRunEvidence.captureError = $compilerCaptureError
        $compilerRunEvidence.outputCapture = $captureEvidence
        [System.IO.File]::WriteAllText(
            (Join-Path $compilerEvidenceDirectory 'compiler-invocation.json'),
            ($compilerRunEvidence | ConvertTo-Json -Depth 12),
            $utf8NoBom)
    }

    if ($compilerLaunchError) {
        throw ('ISCC.exe launch failed; launch error retained in {0}: {1}' -f
            (Join-Path $compilerEvidenceDirectory 'compiler-invocation.json'),
            $compilerLaunchError)
    }
    if ($compilerCaptureError) {
        throw ('ISCC.exe output/process capture failed; diagnostics retained in {0}: {1}' -f
            (Join-Path $compilerEvidenceDirectory 'compiler-invocation.json'),
            $compilerCaptureError)
    }
    if ($null -eq $compilerExitCode) {
        throw ('ISCC.exe completed without a native exit code; diagnostics retained in {0}' -f
            (Join-Path $compilerEvidenceDirectory 'compiler-invocation.json'))
    }
    if ($compilerExitCode -ne 0) {
        throw ('ISCC.exe exited with code {0}; source and complete stream evidence retained at {1}' -f
            $compilerExitCode, $compilerEvidenceDirectory)
    }

    $compiledSetupPath = Join-Path $stageOutput ($buildBaseName + '.exe')
    if (-not (Test-Path -LiteralPath $compiledSetupPath -PathType Leaf)) {
        throw ('ISCC.exe returned success but produced no expected setup file: {0}' -f $compiledSetupPath)
    }
    foreach ($entry in $inventoryArray) {
        $stagedFile = Join-Path $payloadStage (([string]$entry.destination) -replace '/', '\')
        $stagedInfo = Get-Item -LiteralPath $stagedFile
        if ($stagedInfo.Length -ne [long]$entry.bytes -or (Get-FileSha256 $stagedFile) -ne $entry.sha256) {
            throw ('Staged payload changed before compile completion: {0}' -f $entry.destination)
        }
    }
    if ((Get-FileSha256 $stableExePath) -ne $stableAppHash -or
        (Get-FileSha256 $portablePath) -ne $stablePortableHash) {
        throw 'A stable source artifact changed during compiler execution.'
    }

    $setupHash = Get-FileSha256 $compiledSetupPath
    $setupInfo = Get-Item -LiteralPath $compiledSetupPath
    $sourceHashes = [ordered]@{}
    foreach ($key in $allSourceHashes.Keys) { $sourceHashes[$key] = $allSourceHashes[$key] }
    $sourceHashes['compiler/ISCC.exe'] = $compilerHash
    $buildReceipt = [ordered]@{
        schemaVersion = 1
        artifactKind = 'candidate-only'
        createdUtc = [DateTime]::UtcNow.ToString('o')
        outputPath = $OutputPath
        setupBytes = [long]$setupInfo.Length
        setupSha256 = $setupHash
        embeddedAppSha256 = [string]$embeddedAppEntry.sha256
        stableAppSha256 = $stableAppHash
        stablePortableSha256Before = $stablePortableHash
        stablePortableSha256After = Get-FileSha256 $portablePath
        payloadInventorySha256 = $inventoryHash
        payloadInventory = $inventoryArray
        activeInventoryId = $activeInventoryId
        acceptedCompiledInventoriesSha256 = $acceptedInventoryHash
        acceptedCompiledInventories = $acceptedInventoryArray
        sourceHashes = $sourceHashes
        stagedInstallerSources = [ordered]@{
            SDIR_iss = Get-FileSha256 (Join-Path $stagePath 'SDIR.iss')
            rollback_iss = Get-FileSha256 (Join-Path $stagePath 'rollback.iss')
            payload_constants_iss = Get-FileSha256 (Join-Path $stagePath 'payload-constants.iss')
            payload_initialize_iss = Get-FileSha256 (Join-Path $stagePath 'payload-initialize.iss')
            payload_files_iss = Get-FileSha256 (Join-Path $stagePath 'payload-files.iss')
            README_ko_txt = Get-FileSha256 (Join-Path $stagePath 'README-ko.txt')
        }
        compilerProvenance = [ordered]@{
            path = $compilerPath
            version = [string]$compilerMetadata.version
            installerSha256 = [string]$compilerMetadata.sha256
            installerBytes = [long]$compilerMetadata.bytes
            authenticode = [string]$compilerMetadata.authenticode
            publisher = [string]$compilerMetadata.publisher
            isccSha256 = $compilerHash
        }
        buildDefines = @($buildDefines.ToArray())
        testBuild = [bool]$TestBuild
        testRootName = if ($TestBuild) { $TestRootName } else { $null }
        testAppId = if ($TestBuild) { $testAppId } else { $null }
        testAppName = if ($TestBuild) { $testAppName } else { $null }
        testGroupName = if ($TestBuild) { $testGroupName } else { $null }
        testBaseline = [bool]$TestBaseline
        testBaselineSource = if ($TestBaseline) { 'docs/task-id/performance-optimize/baseline/SDIR-baseline.exe' } else { $null }
        testMissingRuntime = [bool]$TestMissingRuntime
        testFault = if ($TestBuild) { $TestFault } else { 'None' }
        testProfileDirectory = if ($TestBuild) { $profileFullPath } else { $null }
        testProfileMarker = if ($TestBuild) { $profileMarkerValue } else { $null }
        testDebugPort = if ($TestBuild) { $TestDebugPort } else { $null }
        testLauncher = if ($TestBuild) { 'sdir-installer-test-launcher.cmd' } else { $null }
        identity = [ordered]@{
            appId = $appId
            appName = $appName
            groupName = $groupName
            rootName = $rootName
            defaultDirectory = $defaultDirectory
        }
        compilerExitCode = $compilerExitCode
        compilerEvidenceDirectory = $compilerEvidenceDirectory
        compilerOutputCapture = $captureEvidence
        verification = 'compile-and-payload-static-checks-only; Windows UAT not claimed'
    }
    $stagedReceiptPath = Join-Path $stagePath 'build-receipt.json'
    [System.IO.File]::WriteAllText($stagedReceiptPath,
        ($buildReceipt | ConvertTo-Json -Depth 12), $utf8NoBom)

    if (Test-Path -LiteralPath $OutputPath) {
        throw ('OutputPath appeared before final move and will not be overwritten: {0}' -f $OutputPath)
    }
    if (Test-Path -LiteralPath $receiptPath) {
        throw ('Build receipt path appeared before final move and will not be overwritten: {0}' -f $receiptPath)
    }
    [System.IO.File]::Move($compiledSetupPath, $OutputPath)
    if ((Get-FileSha256 $OutputPath) -ne $setupHash) {
        throw 'The checked candidate move changed the setup hash; the candidate was retained for review.'
    }
    [System.IO.File]::Move($stagedReceiptPath, $receiptPath)
    if (-not (Test-Path -LiteralPath $receiptPath -PathType Leaf)) {
        throw 'The setup candidate moved but its build receipt did not.'
    }
    $compileSucceeded = $true
    Write-Output ('Candidate setup: {0}' -f $OutputPath)
    Write-Output ('Candidate SHA-256: {0}' -f $setupHash)
    Write-Output ('Build receipt: {0}' -f $receiptPath)
}
finally {
    if ($compileSucceeded) {
        try {
            Assert-OwnedStage -Path $stagePath -StageId $stageId
            Remove-Item -LiteralPath $stagePath -Recurse -Force
        }
        catch {
            [Console]::Error.WriteLine(('Owned stage cleanup failed and was retained: {0}' -f $_.Exception.Message))
        }
    }
}
