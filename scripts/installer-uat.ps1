[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$EvidenceDirectory,
    [switch]$Execute,
    [string[]]$Stage = @('all'),
    [string[]]$Cases
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-UatStandaloneCase {
    param([string[]]$Cases, [bool]$CasesBound, [bool]$StageBound, [bool]$Execute)
    if ($CasesBound -and $Execute -and -not $StageBound -and $null -ne $Cases -and $Cases.Count -eq 1) {
        $caseId = [string]$Cases[0]
        if (@('A6', 'A15', 'A16') -ccontains $caseId) { return $caseId }
    }
    return $null
}

$requestedCase = Get-UatStandaloneCase -Cases $Cases -CasesBound ($PSBoundParameters.ContainsKey('Cases')) -StageBound ($PSBoundParameters.ContainsKey('Stage')) -Execute ([bool]$Execute)
if ($null -eq $requestedCase) {
    [Console]::Error.WriteLine('Only explicit -Execute -Cases A6, A15, or A16 as one case is supported; Stage, default-all, mixed, duplicate, and unsupported selections are refused.')
    exit 2
}
$script:selectedCases = @($requestedCase)
$script:diagnosticCandidateKeys = @(if ($requestedCase -ceq 'A6') { 'portable', 'separate' }
    elseif ($requestedCase -ceq 'A15') { 'fresh' })

if ($env:OS -ne 'Windows_NT') {
    throw 'installer-uat.ps1 runs only on Windows. This coding run must not execute installer or app journeys.'
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$helperPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'installer-ui.ps1'))
$staticEvidenceRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'docs\task-id\windows-installer\uat-harness-static'))
$providedEvidencePath = [System.IO.Path]::GetFullPath($EvidenceDirectory)
$evidencePrefix = $staticEvidenceRoot.TrimEnd('\') + '\'
if (-not $providedEvidencePath.StartsWith($evidencePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw ('EvidenceDirectory must be a new run directory under {0}.' -f $staticEvidenceRoot)
}
if (-not $Execute) {
    throw 'No installer or UI operation was started. After code review, pass -Execute and a unique -EvidenceDirectory for the real Windows UAT.'
}
if (-not [Environment]::Is64BitProcess) {
    throw 'The full UAT must run in 64-bit Windows PowerShell.'
}
if (-not (Test-Path -LiteralPath $helperPath -PathType Leaf)) {
    throw ('Owned UI helper is missing: {0}' -f $helperPath)
}
. $helperPath

$script:runId = [Guid]::NewGuid().ToString('N')
$script:startedUtcAsDate = [DateTime]::UtcNow
$script:startedUtc = $script:startedUtcAsDate.ToString('o')
$script:runOwnerPath = Join-Path $providedEvidencePath 'run-owner.json'
$script:evidenceDirectoryCreated = $false
$script:runOwnerWritten = $false
$script:runOwner = $null
$script:preflightCalled = $false
$script:preflightResult = $null
$script:explicitDiagnostic = $true
$script:diagnosticCompleted = $false
$script:fullsuiteComplete = $false
$script:exitCode = 1
$script:mutexName = $null
$script:uatMutex = $null
$script:mutexAcquired = $false
$script:mutexWasAcquired = $false
$script:mutexAbandoned = $false
$script:originalErrorRecord = $null
$script:finalizationFailure = ''
$script:resultWriteAttempted = $false
$script:resultWriteFailed = $false
$script:resultWriteFailureText = ''
$script:diagnosticCandidateEvidencePath = $null
$script:diagnosticCandidateRecord = $null
$script:compilerReadiness = $null
$script:sequence = 0
$script:eventsPath = Join-Path $providedEvidencePath 'events.jsonl'
$script:commandsPath = Join-Path $providedEvidencePath 'commands.jsonl'
$script:resultPath = Join-Path $providedEvidencePath 'user-journey-result.json'
$script:commandDirectory = Join-Path $providedEvidencePath 'commands'
$script:activeProcesses = New-Object System.Collections.Generic.List[object]
$script:retainedRecoveryResources = New-Object System.Collections.Generic.List[object]
$script:commandRecords = New-Object System.Collections.Generic.List[object]
$script:candidateRecords = New-Object System.Collections.Generic.List[object]
$script:candidateProvenancePaths = New-Object System.Collections.Generic.List[string]
$script:initialTestRoot = $null
$script:initialTestRootName = $null
$script:a6Evidence = @()
$script:a15Evidence = @()
$script:productionSnapshotAfter = $null
$script:sourceGaps = New-Object System.Collections.Generic.List[string]
$script:a9CoreVerified = $false
$script:a9Evidence = @()
$script:failureText = ''
$script:completed = $false
$script:cleanupVerified = $false
$script:stableAppHashBefore = $null
$script:stableAppHashAfter = $null
$script:stablePortableHashBefore = $null
$script:stablePortableHashAfter = $null
$script:sourceBuildIdentityEquivalence = $null
$script:a4Evidence = @()
$script:skipNames = @()
$script:caseDefinitions = [ordered]@{
    A1 = 'Compiler provenance, explicit stable payload hashes, source/build identity equivalence, and unchanged stable app/portable artifacts.'
    A2 = 'Korean welcome/options/ready/install/finish journey, desktop option off/on, real cancellation input, and no elevation.'
    A3 = 'Fresh install exact payload bytes, ownership receipt, one correct test HKCU uninstall entry, and shortcut targets/working directories.'
    A4 = 'Wizard, direct executable, Start Menu, and desktop launches open the installed app, show fixture files, and persist profile changes after normal WM_CLOSE.'
    A5 = 'Setup preserves raw profile bytes; supported layout, tabs, groups, workspaces, theme, and folder state restore semantically in the UI.'
    A6 = 'Portable destination is refused intact; a separate install works; settings-only manual transfer preserves source and never overwrites existing settings.'
    A7 = 'A real B158 TestBuild upgrade removes the compiled historical test-owned path before stable replacement and preserves unknown/data sentinels.'
    A8 = 'A running app and a held idle worker block GUI/silent reinstall and uninstall while windows/processes and all install bytes remain unchanged.'
    A9 = 'Exclusive exe, document, obsolete-owned file, and native metadata locks refuse safely; release and retry are observed.'
    A10 = 'GUI cancellation before mutation and during partial extraction restore prior bytes, defined NTFS metadata, streams, owner/DACL, directories, receipt, registry, and shortcuts after exit.'
    A11 = 'Backup, partial-extraction, and late pre-finalization fault builds return expected nonzero results and restore the full prior snapshot.'
    A12 = 'Forced kill, power loss, and interrupted recovery are explicitly excluded by the reviewed plan.'
    A13 = 'Wrong /DIR, unowned/colliding root, portable flag, malformed receipt, reparse traversal, modified-owned file, and unknown collision refuse without damage.'
    A14 = 'The registered uninstaller confirmation can be cancelled and then completed; user settings, data, and unknown files remain.'
    A15 = 'Ordinary running/locked uninstall refuses; uninstall then reinstall restores the saved UI state.'
    A16 = 'Bounded silent fresh install, reinstall, and removal return actual exit codes and never launch, close, or reboot.'
    A17 = 'Actual WebView2 runtime detection and installed-app launch plus the TestBuild-only missing-runtime refusal branch; host runtime is untouched.'
    A18 = 'Production installer identity, actual SDIR profile, shortcuts, registration, unrelated sentinels, and stable distribution hashes remain untouched; only owned fixtures are removed after full pass.'
}
$script:caseResults = [ordered]@{}
foreach ($caseId in $script:caseDefinitions.Keys) {
    $initialStatus = if ($caseId -eq 'A12') { 'excluded' } else { 'pending' }
    $script:caseResults[$caseId] = [ordered]@{
        id = $caseId
        selected = [bool]($script:selectedCases -ccontains [string]$caseId)
        requirement = $script:caseDefinitions[$caseId]
        status = $initialStatus
        evidence = @()
        details = @()
    }
}

$script:stageMap = @{
    provenance = @('A1')
    upgrade = @('A7')
    wizard = @('A2', 'A3')
    launch = @('A4', 'A5', 'A15')
    portable = @('A6')
    locks = @('A8', 'A9')
    rollback = @('A10', 'A11')
    refusal = @('A13')
    uninstall = @('A14', 'A15')
    silent = @('A16')
    runtime = @('A17')
}
$script:skipNames = @($script:caseDefinitions.Keys | Where-Object { $_ -notin $script:selectedCases -and $_ -ne 'A12' })

function Write-UatJsonFile {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)]$Value)
    $json = ConvertTo-Json -InputObject $Value -Depth 30
    $encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false
    [System.IO.File]::WriteAllText($Path, $json, $encoding)
}

function Add-UatEvent {
    param(
        [Parameter(Mandatory = $true)][string]$Kind,
        [Parameter(Mandatory = $true)][string]$Name,
        [hashtable]$Data = @{}
    )
    $script:sequence++
    $record = [ordered]@{
        sequence = $script:sequence
        runId = $script:runId
        timestampUtc = [DateTime]::UtcNow.ToString('o')
        kind = $Kind
        name = $Name
        data = $Data
    }
    $line = ConvertTo-Json -InputObject $record -Depth 20 -Compress
    [System.IO.File]::AppendAllText($script:eventsPath, $line + "`r`n", (New-Object System.Text.UTF8Encoding -ArgumentList $false))
}

function Set-UatCase {
    param(
        [Parameter(Mandatory = $true)][string]$Id,
        [Parameter(Mandatory = $true)][ValidateSet('verified', 'pending', 'excluded')][string]$Status,
        [string[]]$Evidence = @(),
        [string[]]$Details = @(),
        [switch]$DeferResultWrite
    )
    Assert-Uat ($script:evidenceDirectoryCreated -and $script:runOwnerWritten) 'No case result can be recorded before the evidence directory and run owner are established.'
    Assert-Uat ($script:caseResults.Contains($Id)) ('Unknown case result ID: {0}' -f $Id)
    $selected = [bool]$script:caseResults[$Id].selected
    if ($Status -eq 'verified' -and -not $selected) {
        $Status = 'pending'
        $Details = @($Details) + @('This case was exercised as a prerequisite but was not selected for this diagnostic run; it remains unqualified.')
    }
    $evidencePaths = New-Object System.Collections.Generic.List[string]
    foreach ($path in @($Evidence)) {
        Assert-Uat (-not [string]::IsNullOrWhiteSpace([string]$path)) ('Case {0} received an empty evidence path.' -f $Id)
        $fullPath = [System.IO.Path]::GetFullPath([string]$path)
        Assert-Uat (Test-Path -LiteralPath $fullPath) ('Case {0} evidence path does not exist: {1}' -f $Id, $fullPath)
        [void]$evidencePaths.Add($fullPath)
    }
    $script:caseResults[$Id].selected = $selected
    $script:caseResults[$Id].status = $Status
    $script:caseResults[$Id].evidence = @($evidencePaths.ToArray() | Select-Object -Unique)
    $script:caseResults[$Id].details = @($Details)
    Add-UatEvent -Kind 'case-status' -Name $Id -Data @{ selected = $selected; status = $Status; evidence = $script:caseResults[$Id].evidence; details = $script:caseResults[$Id].details }
    if (-not $DeferResultWrite) { Write-UatResult }
}

function Write-UatResult {
    Assert-Uat ($script:evidenceDirectoryCreated -and $script:runOwnerWritten) 'No result can be written before run ownership is established.'
    if ($script:resultWriteFailed) {
        throw 'A previous UAT result write failed; the result writer will not retry.'
    }
    $script:resultWriteAttempted = $true
    try {
        $mandatory = @($script:caseResults.Values | Where-Object { $_.id -ne 'A12' })
        $allPassed = @($mandatory | Where-Object { $_.status -ne 'verified' }).Count -eq 0
        $outcome = if ($script:explicitDiagnostic) { 'not-qualified' } elseif ($allPassed) { 'qualified' } else { 'not-qualified' }
        $record = [ordered]@{
            schemaVersion = 1
            runId = $script:runId
            startedUtc = $script:startedUtc
            updatedUtc = [DateTime]::UtcNow.ToString('o')
            overall = $outcome
            execution = if ($script:completed) { 'completed' } elseif ($script:failureText) { 'stopped-on-unexpected-failure' } else { 'in-progress-or-not-started' }
            evidenceDirectory = $providedEvidencePath
            runOwnerPath = $script:runOwnerPath
            explicitDiagnostic = [bool]$script:explicitDiagnostic
            fullsuiteComplete = [bool]$script:fullsuiteComplete
            selectedCases = @($script:selectedCases)
            diagnosticCompleted = [bool]$script:diagnosticCompleted
            diagnosticExitCode = [int]$script:exitCode
            mutexAcquired = [bool]$script:mutexWasAcquired
            mutexAbandoned = [bool]$script:mutexAbandoned
            candidateProvenance = $script:diagnosticCandidateEvidencePath
            candidateProvenancePaths = @($script:candidateProvenancePaths.ToArray())
            resultWriteAttempted = [bool]$script:resultWriteAttempted
            resultWriteFailed = [bool]$script:resultWriteFailed
            resultWriteFailure = $script:resultWriteFailureText
            appExpectedSha256 = '766bf2aaf832eba9812b0d97f7dd207c0278ae9bd8c332de054548cbf6719740'
            portableExpectedSha256 = '6aa32159140ce9ed0ff1bb06a486651290cb0e74df7e7dd37104ae720fcea4d5'
            stableAppSha256Before = $script:stableAppHashBefore
            stableAppSha256After = $script:stableAppHashAfter
            stablePortableSha256Before = $script:stablePortableHashBefore
            stablePortableSha256After = $script:stablePortableHashAfter
            sourceBuildIdentityEquivalence = $script:sourceBuildIdentityEquivalence
            productionInstallAttempted = $false
            productionBuildPerformed = $false
            candidates = @($script:candidateRecords.ToArray())
            sourceGaps = @($script:sourceGaps.ToArray())
            commands = [System.IO.Path]::GetFullPath($script:commandsPath)
            eventLog = [System.IO.Path]::GetFullPath($script:eventsPath)
            failure = $script:failureText
            originalErrorRecord = if ($null -eq $script:originalErrorRecord) { $null } else { [ordered]@{
                exceptionType = $script:originalErrorRecord.Exception.GetType().FullName
                exception = $script:originalErrorRecord.Exception.ToString()
                fullyQualifiedErrorId = [string]$script:originalErrorRecord.FullyQualifiedErrorId
                category = [string]$script:originalErrorRecord.CategoryInfo
                scriptStackTrace = [string]$script:originalErrorRecord.ScriptStackTrace
            } }
            finalizationFailure = $script:finalizationFailure
            cases = @($script:caseResults.Values)
            cleanup = [ordered]@{ verified = $script:cleanupVerified; details = if ($script:cleanupVerified) { 'Owned test fixtures were removed after all mandatory cases passed and no child remained.' } else { 'No fixture cleanup is claimed.' } }
        }
        Write-UatJsonFile -Path $script:resultPath -Value $record
    }
    catch {
        $script:resultWriteFailed = $true
        $script:resultWriteFailureText = $_.Exception.ToString()
        throw
    }
}

function Assert-Uat {
    param([Parameter(Mandatory = $true)][bool]$Condition, [Parameter(Mandatory = $true)][string]$Message)
    if (-not $Condition) { throw $Message }
}

function Assert-UatEqual {
    param([AllowNull()]$Expected, [AllowNull()]$Actual, [Parameter(Mandatory = $true)][string]$Message)
    $left = ConvertTo-Json -InputObject $Expected -Depth 30 -Compress
    $right = ConvertTo-Json -InputObject $Actual -Depth 30 -Compress
    if ($left -cne $right) { throw ('{0} Expected={1}; Actual={2}' -f $Message, $left, $right) }
}

function Assert-NoReparseComponents {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    $current = $root
    $relative = $full.Substring($root.Length).Split([char[]]@('\', '/'), [System.StringSplitOptions]::RemoveEmptyEntries)
    foreach ($part in $relative) {
        $current = Join-Path $current $part
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw ('Refusing a reparse point in a fixture path: {0}' -f $current)
            }
        }
    }
}

function Get-UatStrictDecodedText {
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()][byte[]]$Bytes)
    $offset = 0
    $encodingName = 'utf-8'
    $encoding = [System.Text.UTF8Encoding]::new($false, $true)
    if ($Bytes.Length -ge 4 -and $Bytes[0] -eq 0 -and $Bytes[1] -eq 0 -and $Bytes[2] -eq 254 -and $Bytes[3] -eq 255) {
        $encodingName = 'utf-32be-bom'; $encoding = [System.Text.UTF32Encoding]::new($true, $true, $true); $offset = 4
    }
    elseif ($Bytes.Length -ge 4 -and $Bytes[0] -eq 255 -and $Bytes[1] -eq 254 -and $Bytes[2] -eq 0 -and $Bytes[3] -eq 0) {
        $encodingName = 'utf-32le-bom'; $encoding = [System.Text.UTF32Encoding]::new($false, $true, $true); $offset = 4
    }
    elseif ($Bytes.Length -ge 3 -and $Bytes[0] -eq 239 -and $Bytes[1] -eq 187 -and $Bytes[2] -eq 191) {
        $encodingName = 'utf-8-bom'; $encoding = [System.Text.UTF8Encoding]::new($false, $true); $offset = 3
    }
    elseif ($Bytes.Length -ge 2 -and $Bytes[0] -eq 254 -and $Bytes[1] -eq 255) {
        $encodingName = 'utf-16be-bom'; $encoding = [System.Text.UnicodeEncoding]::new($true, $true, $true); $offset = 2
    }
    elseif ($Bytes.Length -ge 2 -and $Bytes[0] -eq 255 -and $Bytes[1] -eq 254) {
        $encodingName = 'utf-16le-bom'; $encoding = [System.Text.UnicodeEncoding]::new($false, $true, $true); $offset = 2
    }
    try {
        return [pscustomobject]@{ text = $encoding.GetString($Bytes, $offset, $Bytes.Length - $offset); encoding = $encodingName }
    }
    catch [System.Text.DecoderFallbackException] {
        if ($offset -ne 0) { throw }
        $codePage = [Console]::OutputEncoding.CodePage
        $strict = [System.Text.Encoding]::GetEncoding($codePage,
            [System.Text.EncoderFallback]::ExceptionFallback,
            [System.Text.DecoderFallback]::ExceptionFallback)
        return [pscustomobject]@{ text = $strict.GetString($Bytes); encoding = ('windows-codepage-{0}' -f $codePage) }
    }
}

function ConvertTo-WindowsCommandArgument {
    param([AllowEmptyString()][string]$Value)
    $builder = New-Object System.Text.StringBuilder
    [void]$builder.Append([char]34)
    $backslashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq [char]92) { $backslashes++; continue }
        if ($character -eq [char]34) {
            for ($index = 0; $index -lt ($backslashes * 2 + 1); $index++) { [void]$builder.Append([char]92) }
            [void]$builder.Append([char]34)
        }
        else {
            for ($index = 0; $index -lt $backslashes; $index++) { [void]$builder.Append([char]92) }
            [void]$builder.Append($character)
        }
        $backslashes = 0
    }
    for ($index = 0; $index -lt ($backslashes * 2); $index++) { [void]$builder.Append([char]92) }
    [void]$builder.Append([char]34)
    return $builder.ToString()
}

function Start-UatProcess {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [string[]]$Argument = @(),
        [string]$WorkingDirectory = $repoRoot,
        [hashtable]$Environment = @{},
        [Parameter(Mandatory = $true)][string]$Name,
        [switch]$TrackNativeEngines,
        [string]$NativeAppRoot,
        $CompletionBudget = $null,
        [switch]$A6PortableRefusal,
        [string]$A6PortableDestinationPath
    )
    $fullExe = [System.IO.Path]::GetFullPath($Executable)
    $captureIntent = $null
    if ($A6PortableRefusal) {
        $captureIntent = Assert-UatA6PortableRefusalStart -Executable $fullExe -Arguments $Argument `
            -WorkingDirectory $WorkingDirectory -Environment $Environment -Name $Name `
            -TrackNativeEngines ([bool]$TrackNativeEngines) -NativeAppRoot $NativeAppRoot `
            -PortableDestinationPath $A6PortableDestinationPath
    }
    elseif (-not [string]::IsNullOrEmpty($A6PortableDestinationPath)) {
        throw 'The portable.flag destination argument is reserved for the A6 root-only refusal.'
    }
    if (-not (Test-Path -LiteralPath $fullExe -PathType Leaf)) { throw ('Command executable is missing: {0}' -f $fullExe) }
    $fullWorkingDirectory = [System.IO.Path]::GetFullPath($WorkingDirectory)
    $argumentLine = (@($Argument | ForEach-Object { ConvertTo-WindowsCommandArgument ([string]$_) }) -join ' ')
    $environmentPairs = @()
    foreach ($key in $Environment.Keys) { $environmentPairs += ('{0}={1}' -f $key, [string]$Environment[$key]) }
    $commandId = [Guid]::NewGuid().ToString('N')
    $productionProtectionBefore = $null
    if ($null -ne $captureIntent) {
        $productionRegistryKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $script:productionAppId + '_is1'
        $productionProtectionBefore = Get-UatProductionSnapshot -ProductionRegistryKey $productionRegistryKey
        Assert-NoReparseComponents -Path $captureIntent.logPath
        Assert-Uat (-not (Test-Path -LiteralPath $captureIntent.logPath)) 'A6 root-only capture requires a fresh log at launch.'
    }
    $child = [Sdir.Installer.Uat.OwnedProcess]::Start($fullExe, $argumentLine, $fullWorkingDirectory, [string[]]$environmentPairs)
    $entry = [pscustomobject]@{
        commandId = $commandId; sessionId = $script:runId; name = $Name; executable = $fullExe
        arguments = $argumentLine; workingDirectory = $fullWorkingDirectory; processId = [int]$child.Id
        startedUtc = $child.StartTimeUtc.ToString('o'); kind = 'owned-child'; child = $child; process = $null
        workerReadyEvidence = $null; workerWatchPath = $null; nativeTracking = [bool]$TrackNativeEngines
        nativeAppRoot = if ($TrackNativeEngines -and $NativeAppRoot) { [System.IO.Path]::GetFullPath($NativeAppRoot) } else { $null }
        nativeObservationStarted = $false; nativeObservationStartedUtc = $null; nativeObservationError = $null
        nativeObservationCount = 0; nativeLauncher = $null; nativeEngineChildren = New-Object System.Collections.Generic.List[object]
        completionBudget = $CompletionBudget; launchIntent = $null; lastCapture = $null; finalCapture = $null; completedResult = $null; finished = $false
        partialWriteFailed = $false
        captureIntent = $captureIntent; portableRefusalQualified = $false
        productionProtectionBefore = $productionProtectionBefore
    }
    if ($null -ne $captureIntent) {
        $captureIntent['commandId'] = $commandId
        $captureIntent['processId'] = [int]$child.Id
        $captureIntent['processStartedUtc'] = $entry.startedUtc
    }
    [void]$script:activeProcesses.Add($entry)
    $startRecord = [ordered]@{
        commandId = $commandId; sessionId = $script:runId; name = $Name; executable = $fullExe
        arguments = $argumentLine; workingDirectory = $fullWorkingDirectory; processId = [int]$child.Id
        startedUtc = $entry.startedUtc; phase = 'started'; nativeTracking = [bool]$TrackNativeEngines
        nativeAppRoot = $entry.nativeAppRoot
    }
    if ($null -ne $captureIntent) { $startRecord['captureIntent'] = $captureIntent }
    [System.IO.File]::AppendAllText($script:commandsPath,
        (ConvertTo-Json -InputObject $startRecord -Depth 12 -Compress) + "`r`n",
        (New-Object System.Text.UTF8Encoding -ArgumentList $false))
    Add-UatEvent -Kind 'process-started' -Name $Name -Data @{ commandId = $commandId; processId = [int]$child.Id; executable = $fullExe; nativeTracking = [bool]$TrackNativeEngines }
    if ($TrackNativeEngines) {
        try {
            $launcherProcess = [System.Diagnostics.Process]::GetProcessById([int]$child.Id)
            $launcherHandle = $launcherProcess.Handle
            if ($launcherHandle -eq [IntPtr]::Zero -or $launcherProcess.HasExited) { throw 'Native launcher was not live while its process handle was pinned.' }
            $launcherStart = $launcherProcess.StartTime.ToUniversalTime()
            $launcherTicks = [long]$launcherStart.Ticks
            $launcherImage = [System.IO.Path]::GetFullPath([Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath([int]$child.Id))
            if ($launcherTicks -ne [long]$child.StartTimeUtc.Ticks -or -not [string]::Equals($launcherImage, $fullExe, [System.StringComparison]::OrdinalIgnoreCase)) { throw 'Native launcher PID/startTicks/image did not match the started process entry.' }
            $launcherRow = Get-CimInstance -ClassName Win32_Process -Filter ('ProcessId = {0}' -f [int]$child.Id) -ErrorAction Stop
            if ($null -eq $launcherRow -or [int]$launcherRow.ProcessId -ne [int]$child.Id) { throw 'Native launcher disappeared from the CIM correlation snapshot.' }
            $launcherCreated = ConvertTo-UatCimCreationUtc -Value $launcherRow.CreationDate -ProcessId ([int]$child.Id)
            if ([Math]::Abs($launcherCreated.Ticks - $launcherTicks) -ge [TimeSpan]::TicksPerMillisecond) { throw 'Native launcher CIM/native correlation exceeded 1 ms.' }
            $parentId = [int]$launcherRow.ParentProcessId
            $parentProcess = [System.Diagnostics.Process]::GetProcessById($parentId)
            $parentHandle = $parentProcess.Handle
            if ($parentHandle -eq [IntPtr]::Zero -or $parentProcess.HasExited) { throw 'Native launcher parent was not live while its exact identity was pinned.' }
            $parentStart = $parentProcess.StartTime.ToUniversalTime()
            $parentTicks = [long]$parentStart.Ticks
            $parentImage = [System.IO.Path]::GetFullPath([Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath($parentId))
            $parentRow = Get-CimInstance -ClassName Win32_Process -Filter ('ProcessId = {0}' -f $parentId) -ErrorAction Stop
            if ($null -eq $parentRow -or [int]$parentRow.ProcessId -ne $parentId) { throw 'Native launcher parent disappeared from the CIM correlation snapshot.' }
            $parentCreated = ConvertTo-UatCimCreationUtc -Value $parentRow.CreationDate -ProcessId $parentId
            if ([Math]::Abs($parentCreated.Ticks - $parentTicks) -ge [TimeSpan]::TicksPerMillisecond -or $parentTicks -gt $launcherTicks) { throw 'Native launcher parent failed exact native/CIM identity correlation.' }
            if ($launcherProcess.HasExited -or $parentProcess.HasExited -or [long]$launcherProcess.StartTime.ToUniversalTime().Ticks -ne $launcherTicks -or [long]$parentProcess.StartTime.ToUniversalTime().Ticks -ne $parentTicks) { throw 'Native launcher or parent identity changed during initial pinning.' }
            $entry.process = $launcherProcess
            $entry.nativeLauncher = [pscustomobject]@{
                ownershipRoute = $null
                ownershipCommandId = $null
                commandLine = [string]$launcherRow.CommandLine; observedUtc = [DateTime]::UtcNow.ToString('o')
                processId = [int]$child.Id; startUtc = $launcherStart; startTicks = $launcherTicks; imagePath = $launcherImage
                parentProcessId = $parentId; parentStartTicks = $parentTicks; parentImagePath = $parentImage
                process = $launcherProcess; pinnedHandle = $launcherHandle; parentProcess = $parentProcess; parentPinnedHandle = $parentHandle
            }
            $entry.nativeObservationStarted = $true
            $entry.nativeObservationStartedUtc = [DateTime]::UtcNow.ToString('o')
            Update-UatNativeEngineChildren -Entry $entry
        }
        catch {
            $observationFailure = $_.Exception
            $entry.nativeObservationError = $observationFailure.ToString()
            Add-UatEvent -Kind 'native-observation-incomplete' -Name $Name -Data @{ commandId = $commandId; processId = [int]$child.Id; diagnostic = $entry.nativeObservationError; observedUtc = [DateTime]::UtcNow.ToString('o') }
            try { Save-UatProcessPartial -Entry $entry }
            catch { Add-UatEvent -Kind 'process-partial-save-failed' -Name $Name -Data @{ commandId = $commandId; processId = [int]$child.Id; diagnostic = $_.Exception.ToString() } }
            throw $observationFailure
        }
    }
    return $entry
}
function Write-UatCaptureFiles {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)]$Capture,
        [switch]$Partial
    )
    $folder = Join-Path $script:commandDirectory $Entry.commandId
    if (-not (Test-Path -LiteralPath $folder -PathType Container)) { New-Item -ItemType Directory -Path $folder | Out-Null }
    $suffix = if ($Partial) { '.partial' } else { '' }
    $stdoutPath = Join-Path $folder ('stdout{0}.bin' -f $suffix)
    $stderrPath = Join-Path $folder ('stderr{0}.bin' -f $suffix)
    $chunksPath = Join-Path $folder ('chunks{0}.json' -f $suffix)
    [System.IO.File]::WriteAllBytes($stdoutPath, [byte[]]$Capture.StandardOutput)
    [System.IO.File]::WriteAllBytes($stderrPath, [byte[]]$Capture.StandardError)
    $stdoutDecoded = Get-UatStrictDecodedText -Bytes ([byte[]]$Capture.StandardOutput)
    $stderrDecoded = Get-UatStrictDecodedText -Bytes ([byte[]]$Capture.StandardError)
    $utf8 = New-Object System.Text.UTF8Encoding -ArgumentList $false
    [System.IO.File]::WriteAllText((Join-Path $folder ('stdout{0}.txt' -f $suffix)), $stdoutDecoded.text, $utf8)
    [System.IO.File]::WriteAllText((Join-Path $folder ('stderr{0}.txt' -f $suffix)), $stderrDecoded.text, $utf8)
    $chunkRows = @($Capture.Chunks | Sort-Object Sequence | ForEach-Object {
        [ordered]@{ sequence = $_.Sequence; stream = $_.StreamName; offset = $_.Offset; length = $_.Length }
    })
    Write-UatJsonFile -Path $chunksPath -Value $chunkRows
    return [pscustomobject]@{
        directory = [System.IO.Path]::GetFullPath($folder)
        stdoutRaw = [System.IO.Path]::GetFullPath($stdoutPath)
        stderrRaw = [System.IO.Path]::GetFullPath($stderrPath)
        chunks = [System.IO.Path]::GetFullPath($chunksPath)
        stdoutEncoding = $stdoutDecoded.encoding
        stderrEncoding = $stderrDecoded.encoding
        stdoutBytes = ([byte[]]$Capture.StandardOutput).Length
        stderrBytes = ([byte[]]$Capture.StandardError).Length
    }
}

function Save-UatProcessPartial {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [string]$Diagnostic = $null
    )
    try {
    if ($Entry.finished) { return }
    $saveErrors = New-Object System.Collections.Generic.List[string]
    $capture = $Entry.lastCapture
    $files = $null
    if ($null -ne $capture) {
        try { $files = Write-UatCaptureFiles -Entry $Entry -Capture $capture -Partial }
        catch { [void]$saveErrors.Add(('last capture file save failed: {0}' -f $_.Exception.ToString())) }
    }
    if ($saveErrors.Count -eq 0) {
        try {
            $capture = $Entry.child.CaptureWhileRunning()
            $Entry.lastCapture = $capture
            $files = Write-UatCaptureFiles -Entry $Entry -Capture $capture -Partial
        }
        catch { [void]$saveErrors.Add(('current partial capture/save failed: {0}' -f $_.Exception.ToString())) }
    }
    $launcherExitCode = $null
    $isFinal = $null
    $outputComplete = $null
    $completionTimedOut = $null
    $captureError = $null
    if ($null -ne $capture) {
        if ($capture.HasExitCode) { $launcherExitCode = [int]$capture.ExitCode }
        $isFinal = [bool]$capture.IsFinal
        $outputComplete = [bool]$capture.OutputComplete
        $completionTimedOut = [bool]$capture.CompletionTimedOut
        $captureError = $capture.CaptureError
    }
    $nativeEngineExitCodes = @()
    if ($null -ne $Entry.PSObject.Properties['nativeEngineChildren'] -and $null -ne $Entry.nativeEngineChildren) {
        $nativeEngineExitCodes = @($Entry.nativeEngineChildren | Where-Object { -not $_.ownershipRoute } | ForEach-Object {
            $nativeExitCode = $null
            if ($null -ne $_.exitCode) { $nativeExitCode = [int]$_.exitCode }
            [ordered]@{
                processId = [int]$_.processId; startTicks = [long]$_.startTicks; imagePath = [string]$_.imagePath
                parentProcessId = [int]$_.parentProcessId; parentStartTicks = [long]$_.parentStartTicks
                launcherProcessId = [int]$_.launcherProcessId; launcherStartTicks = [long]$_.launcherStartTicks
                exitCode = $nativeExitCode; outputCapture = [string]$_.outputCapture; completedUtc = $_.completedUtc
            }
        })
    }
    $launchHandoff = $null
    if ($null -ne $Entry.PSObject.Properties['launchIntent'] -and $null -ne $Entry.launchIntent) {
        $launchHandoff = [ordered]@{ state = [string]$Entry.launchIntent.state }
    }
    $recordDiagnostic = $Diagnostic
    if ([string]::IsNullOrEmpty($recordDiagnostic) -and $null -ne $Entry.nativeObservationError) {
        $recordDiagnostic = [string]$Entry.nativeObservationError
    }
    $record = [ordered]@{
        commandId = $Entry.commandId; sessionId = $Entry.sessionId; name = $Entry.name
        executable = $Entry.executable; arguments = $Entry.arguments; workingDirectory = $Entry.workingDirectory
        processId = $Entry.processId; startedUtc = $Entry.startedUtc; observedUtc = [DateTime]::UtcNow.ToString('o')
        phase = 'partial'; exitCode = $launcherExitCode; launcherExitCode = $launcherExitCode
        isFinal = $isFinal; outputComplete = $outputComplete; completionTimedOut = $completionTimedOut
        captureError = $captureError; diagnostic = $recordDiagnostic; partialSaveErrors = @($saveErrors.ToArray())
        nativeEngineExitCodes = $nativeEngineExitCodes; launchHandoff = $launchHandoff; evidence = $files
    }
    if ($null -ne $Entry.captureIntent) { $record['captureIntent'] = $Entry.captureIntent }
    try {
        [System.IO.File]::AppendAllText($script:commandsPath,
            (ConvertTo-Json -InputObject $record -Depth 20 -Compress) + "`r`n",
            (New-Object System.Text.UTF8Encoding -ArgumentList $false))
    }
    catch { [void]$saveErrors.Add(('partial command record failed: {0}' -f $_.Exception.ToString())) }
    try {
        Add-UatEvent -Kind 'process-output-partial' -Name $Entry.name -Data @{
            commandId = $Entry.commandId; processId = $Entry.processId; phase = 'partial'; evidence = $files
            launcherExitCode = $launcherExitCode; nativeEngineExitCodes = $nativeEngineExitCodes
            outputComplete = $outputComplete; completionTimedOut = $completionTimedOut
            captureError = $captureError; diagnostic = $recordDiagnostic; saveErrors = @($saveErrors.ToArray())
        }
    }
    catch { [void]$saveErrors.Add(('partial event record failed: {0}' -f $_.Exception.ToString())) }
    if ($saveErrors.Count -gt 0) {
        throw ('Partial output preservation had errors. Original diagnostic: {0}; preservation diagnostics: {1}' -f $recordDiagnostic, ($saveErrors -join ' | '))
    }
    }
    catch {
        $Entry.partialWriteFailed = $true
        throw
    }
}

function Finish-UatProcess {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)]$CompletionBudget,
        [string]$WaitReason = 'child process'
    )
    if ($null -eq $Entry.PSObject.Properties['completedResult']) {
        $Entry | Add-Member -MemberType NoteProperty -Name completedResult -Value $null
    }
    if ($Entry.finished) {
        if ($null -eq $Entry.completedResult) { throw ('Finished process entry has no preserved completion DTO: {0}' -f $Entry.commandId) }
        return $Entry.completedResult
    }
    if ($null -eq $Entry.PSObject.Properties['completionBudget']) {
        $Entry | Add-Member -MemberType NoteProperty -Name completionBudget -Value $null
    }
    if ($null -ne $Entry.completionBudget -and -not [object]::ReferenceEquals($Entry.completionBudget, $CompletionBudget)) {
        throw ('Finish-UatProcess received a different CompletionBudget for command {0}.' -f $Entry.commandId)
    }
    if ($null -eq $Entry.completionBudget) { $Entry.completionBudget = $CompletionBudget }
    $diagnostics = New-Object System.Collections.Generic.List[string]
    $capture = $null
    $captureError = $null
    $files = $null
    $barrierComplete = $false
    try { $barrierComplete = Test-UatProcessComplete -Entry $Entry }
    catch {
        $captureError = $_.Exception.ToString()
        [void]$diagnostics.Add(('completion barrier observation failed: {0}' -f $captureError))
    }
    if (-not $barrierComplete -and $diagnostics.Count -eq 0) {
        [void]$diagnostics.Add(('completion barrier is still incomplete for {0} ({1}).' -f $WaitReason, $Entry.commandId))
    }
    if ($barrierComplete) {
        try {
            $remainingMilliseconds = Get-UatCompletionRemainingMilliseconds -CompletionBudget $CompletionBudget
            $capture = $Entry.child.CaptureAfterExit([int]$remainingMilliseconds)
            $Entry.lastCapture = $capture
        }
        catch {
            $captureError = $_.Exception.ToString()
            [void]$diagnostics.Add(('CaptureAfterExit failed: {0}' -f $captureError))
        }
    }
    $nativeEngineExitCodes = @()
    if ($null -ne $Entry.PSObject.Properties['nativeEngineChildren'] -and $null -ne $Entry.nativeEngineChildren) {
        $nativeEngineExitCodes = @($Entry.nativeEngineChildren | Where-Object { -not $_.ownershipRoute } | ForEach-Object {
            $nativeExitCode = $null
            if ($null -ne $_.exitCode) { $nativeExitCode = [int]$_.exitCode }
            [ordered]@{
                processId = [int]$_.processId; startTicks = [long]$_.startTicks; imagePath = [string]$_.imagePath
                parentProcessId = [int]$_.parentProcessId; parentStartTicks = [long]$_.parentStartTicks
                launcherProcessId = [int]$_.launcherProcessId; launcherStartTicks = [long]$_.launcherStartTicks
                exitCode = $nativeExitCode; outputCapture = [string]$_.outputCapture; completedUtc = $_.completedUtc
            }
        })
    }
    $launchHandoff = $null
    if ($null -ne $Entry.PSObject.Properties['launchIntent'] -and $null -ne $Entry.launchIntent) {
        $launchHandoff = [ordered]@{ state = [string]$Entry.launchIntent.state }
    }
    $launcherExitCode = $null
    $isFinal = $false
    $outputComplete = $null
    $completionTimedOut = $false
    if ($null -ne $capture) {
        if ($capture.HasExitCode) { $launcherExitCode = [int]$capture.ExitCode }
        $isFinal = [bool]$capture.IsFinal
        $outputComplete = [bool]$capture.OutputComplete
        $completionTimedOut = [bool]$capture.CompletionTimedOut
        if ($null -ne $capture.CaptureError) {
            $captureError = [string]$capture.CaptureError
            [void]$diagnostics.Add(('CaptureError: {0}' -f $captureError))
        }
        if (-not $capture.ProcessExited) { [void]$diagnostics.Add('CaptureAfterExit did not confirm launcher exit.') }
        if (-not $capture.HasExitCode) { [void]$diagnostics.Add('CaptureAfterExit did not obtain the launcher exit code.') }
        if (-not $capture.OutputComplete) { [void]$diagnostics.Add('CaptureAfterExit did not complete both output drains.') }
        if (-not $capture.IsFinal) { [void]$diagnostics.Add('CaptureAfterExit returned a non-final DTO.') }
        if ($capture.CompletionTimedOut) { [void]$diagnostics.Add('CaptureAfterExit reported completion timeout.') }
    }
    $remainingAfterCapture = $null
    try {
        $remainingAfterCapture = Get-UatCompletionRemainingMilliseconds -CompletionBudget $CompletionBudget
        if ($remainingAfterCapture -le 0) {
            $completionTimedOut = $true
            [void]$diagnostics.Add('The shared completion budget expired before final recording.')
        }
    }
    catch { [void]$diagnostics.Add(('Shared completion budget read failed: {0}' -f $_.Exception.ToString())) }
    if ($Entry.nativeTracking -and @($nativeEngineExitCodes | Where-Object { $null -eq $_.exitCode }).Count -gt 0) {
        [void]$diagnostics.Add('At least one pinned native engine has no captured actual exit code.')
    }
    if ($null -ne $Entry.launchIntent -and $Entry.launchIntent.state -ne 'transferred') {
        [void]$diagnostics.Add(('Launch handoff is not transferred: {0}' -f $Entry.launchIntent.state))
    }
    if ($diagnostics.Count -eq 0) {
        try { $files = Write-UatCaptureFiles -Entry $Entry -Capture $capture }
        catch { [void]$diagnostics.Add(('Final capture file save failed: {0}' -f $_.Exception.ToString())) }
    }
    if ($diagnostics.Count -eq 0) {
        try {
            if ((Get-UatCompletionRemainingMilliseconds -CompletionBudget $CompletionBudget) -le 0) {
                $completionTimedOut = $true
                [void]$diagnostics.Add('The shared completion budget expired before command recording.')
            }
        }
        catch { [void]$diagnostics.Add(('Shared completion budget read before recording failed: {0}' -f $_.Exception.ToString())) }
    }
    if ($diagnostics.Count -gt 0) {
        $diagnosticText = $diagnostics -join ' | '
        $saveFailure = $null
        try { Save-UatProcessPartial -Entry $Entry -Diagnostic $diagnosticText }
        catch { $saveFailure = $_.Exception.ToString() }
        $incompleteRecord = [ordered]@{
            commandId = $Entry.commandId; sessionId = $Entry.sessionId; name = $Entry.name
            executable = $Entry.executable; arguments = $Entry.arguments; workingDirectory = $Entry.workingDirectory
            processId = $Entry.processId; startedUtc = $Entry.startedUtc; completedUtc = [DateTime]::UtcNow.ToString('o')
            phase = 'completion-incomplete'; exitCode = $launcherExitCode; launcherExitCode = $launcherExitCode
            isFinal = $isFinal; outputComplete = $outputComplete; completionTimedOut = $completionTimedOut
            captureError = $captureError; diagnostic = $diagnosticText; partialSaveError = $saveFailure
            nativeEngineExitCodes = $nativeEngineExitCodes; launchHandoff = $launchHandoff; evidence = $files
        }
        if ($null -ne $Entry.captureIntent) { $incompleteRecord['captureIntent'] = $Entry.captureIntent }
        $recordFailure = $null
        try {
            [System.IO.File]::AppendAllText($script:commandsPath,
                (ConvertTo-Json -InputObject $incompleteRecord -Depth 20 -Compress) + "`r`n",
                (New-Object System.Text.UTF8Encoding -ArgumentList $false))
        }
        catch { $recordFailure = $_.Exception.ToString() }
        $eventFailure = $null
        try {
            Add-UatEvent -Kind 'process-completion-incomplete' -Name $Entry.name -Data @{
                commandId = $Entry.commandId; processId = $Entry.processId; phase = 'completion-incomplete'
                launcherExitCode = $launcherExitCode; nativeEngineExitCodes = $nativeEngineExitCodes
                isFinal = $isFinal; outputComplete = $outputComplete; completionTimedOut = $completionTimedOut
                captureError = $captureError; diagnostic = $diagnosticText; partialSaveError = $saveFailure
                recordError = $recordFailure; evidence = $files
            }
        }
        catch { $eventFailure = $_.Exception.ToString() }
        throw ('Completion did not finish for {0}. Original diagnostic: {1}; partial save: {2}; command record: {3}; event record: {4}' -f $WaitReason, $diagnosticText, $saveFailure, $recordFailure, $eventFailure)
    }
    $record = [ordered]@{
        commandId = $Entry.commandId; sessionId = $Entry.sessionId; name = $Entry.name
        executable = $Entry.executable; arguments = $Entry.arguments; workingDirectory = $Entry.workingDirectory
        processId = $Entry.processId; startedUtc = $Entry.startedUtc; completedUtc = [DateTime]::UtcNow.ToString('o')
        exitCode = $launcherExitCode; launcherExitCode = $launcherExitCode
        stdoutBytes = $files.stdoutBytes; stderrBytes = $files.stderrBytes
        stdoutEncoding = $files.stdoutEncoding; stderrEncoding = $files.stderrEncoding
        stdoutRaw = $files.stdoutRaw; stderrRaw = $files.stderrRaw; orderedChunks = $files.chunks
        isFinal = $isFinal; outputComplete = $outputComplete; completionTimedOut = $completionTimedOut
        captureError = $capture.CaptureError; nativeEngineExitCodes = $nativeEngineExitCodes
        launchHandoff = $launchHandoff; phase = 'exited'
    }
    if ($null -ne $Entry.captureIntent) { $record['captureIntent'] = $Entry.captureIntent }
    try {
        [System.IO.File]::AppendAllText($script:commandsPath,
            (ConvertTo-Json -InputObject $record -Depth 20 -Compress) + "`r`n",
            (New-Object System.Text.UTF8Encoding -ArgumentList $false))
        Add-UatEvent -Kind 'process-exited' -Name $Entry.name -Data @{
            commandId = $Entry.commandId; processId = $Entry.processId; exitCode = $launcherExitCode
            nativeEngineExitCodes = $nativeEngineExitCodes; evidence = $files
        }
    }
    catch {
        $recordingError = $_.Exception.ToString()
        $saveFailure = $null
        try { Save-UatProcessPartial -Entry $Entry -Diagnostic $recordingError }
        catch { $saveFailure = $_.Exception.ToString() }
        throw ('Final process recording failed for {0}. Original diagnostic: {1}; partial save: {2}' -f $Entry.commandId, $recordingError, $saveFailure)
    }
    $result = [pscustomobject]@{
        capture = $capture; evidence = $files; exitCode = $launcherExitCode; launcherExitCode = $launcherExitCode
        nativeEngineExitCodes = $nativeEngineExitCodes; launchHandoff = $launchHandoff
    }
    $Entry.finalCapture = $capture
    $Entry.completedResult = $result
    $Entry.finished = $true
    $Entry.child.Dispose()
    return $result
}

function Wait-UatProcess {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [int]$TimeoutSeconds = 240,
        [string]$WaitReason = 'child process',
        $CompletionBudget = $null
    )
    if ($null -eq $Entry.PSObject.Properties['completionBudget']) {
        $Entry | Add-Member -MemberType NoteProperty -Name completionBudget -Value $null
    }
    if ($null -eq $Entry.PSObject.Properties['completedResult']) {
        $Entry | Add-Member -MemberType NoteProperty -Name completedResult -Value $null
    }
    if ($Entry.finished) {
        if ($null -eq $Entry.completedResult) { throw ('Finished process entry has no preserved completion DTO: {0}' -f $Entry.commandId) }
        return $Entry.completedResult
    }
    if ($null -ne $Entry.completionBudget -and $null -ne $CompletionBudget -and
        -not [object]::ReferenceEquals($Entry.completionBudget, $CompletionBudget)) {
        throw ('Wait-UatProcess received a different CompletionBudget for command {0}.' -f $Entry.commandId)
    }
    if ($null -eq $Entry.completionBudget) {
        if ($null -eq $CompletionBudget) {
            $CompletionBudget = New-UatCompletionBudget -TotalMilliseconds ([long]$TimeoutSeconds * [long]1000) -Origin $WaitReason
        }
        $Entry.completionBudget = $CompletionBudget
    }
    $budget = $Entry.completionBudget
    while ($true) {
        $complete = $false
        $remainingMilliseconds = 0
        try {
            $complete = Test-UatProcessComplete -Entry $Entry
            $remainingMilliseconds = Get-UatCompletionRemainingMilliseconds -CompletionBudget $budget
        }
        catch {
            $observationFailure = $_.Exception.ToString()
            try { Save-UatProcessPartial -Entry $Entry -Diagnostic $observationFailure }
            catch {
                $saveFailure = $_.Exception.ToString()
                throw ('Completion observation failed for {0}. Original diagnostic: {1}; partial save: {2}' -f $WaitReason, $observationFailure, $saveFailure)
            }
            throw ('Completion observation failed for {0}: {1}' -f $WaitReason, $observationFailure)
        }
        if ($complete) {
            return Finish-UatProcess -Entry $Entry -CompletionBudget $budget -WaitReason $WaitReason
        }
        if ($remainingMilliseconds -le 0) {
            $timeoutDiagnostic = ('Timed out waiting for {0}; completion budget expired for PID {1}. The process was not killed.' -f $WaitReason, $Entry.processId)
            try { Save-UatProcessPartial -Entry $Entry -Diagnostic $timeoutDiagnostic }
            catch {
                $saveFailure = $_.Exception.ToString()
                throw ('{0} Partial save also failed: {1}' -f $timeoutDiagnostic, $saveFailure)
            }
            throw $timeoutDiagnostic
        }
        Start-Sleep -Milliseconds ([int][Math]::Min(50, $remainingMilliseconds))
    }
}

function Invoke-UatCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [string[]]$Argument = @(),
        [string]$WorkingDirectory = $repoRoot,
        [hashtable]$Environment = @{},
        [Parameter(Mandatory = $true)][string]$Name,
        [int]$TimeoutSeconds = 240,
        [switch]$TrackNativeEngines,
        [string]$NativeAppRoot,
        $CompletionBudget = $null
    )
    $startParameters = @{
        Executable = $Executable; Argument = $Argument; WorkingDirectory = $WorkingDirectory
        Environment = $Environment; Name = $Name
    }
    if ($TrackNativeEngines) { $startParameters.TrackNativeEngines = $true }
    if (-not [string]::IsNullOrWhiteSpace($NativeAppRoot)) { $startParameters.NativeAppRoot = $NativeAppRoot }
    if ($null -ne $CompletionBudget) { $startParameters.CompletionBudget = $CompletionBudget }
    $entry = Start-UatProcess @startParameters
    $waitParameters = @{ Entry = $entry; TimeoutSeconds = $TimeoutSeconds; WaitReason = $Name }
    if ($null -ne $CompletionBudget) { $waitParameters.CompletionBudget = $CompletionBudget }
    return Wait-UatProcess @waitParameters
}

function Get-UatFileStreams {
    param([Parameter(Mandatory = $true)][string]$Path)
    $result = New-Object System.Collections.Generic.List[object]
    foreach ($stream in @(Get-Item -LiteralPath $Path -Stream * -ErrorAction Stop)) {
        if ($stream.Stream -ceq ':$DATA') { continue }
        [byte[]]$streamBytes = Get-Content -LiteralPath $Path -Stream ([string]$stream.Stream) -Encoding Byte -Raw -ErrorAction Stop
        if ($null -eq $streamBytes) { $streamBytes = [byte[]]@() }
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        try {
            $hash = [System.BitConverter]::ToString($sha256.ComputeHash($streamBytes)).Replace('-', '').ToLowerInvariant()
        } finally {
            $sha256.Dispose()
        }
        [void]$result.Add([ordered]@{ name = [string]$stream.Stream; bytes = [long]$streamBytes.Length; sha256 = $hash })
    }
    return @($result.ToArray() | Sort-Object -CaseSensitive { $_.name })
}

function Get-UatAclSddl {
    param([Parameter(Mandatory = $true)][string]$Path)
    $acl = Get-Acl -LiteralPath $Path -ErrorAction Stop
    return $acl.GetSecurityDescriptorSddlForm([System.Security.AccessControl.AccessControlSections]::All)
}

function Get-UatTreeSnapshot {
    param([Parameter(Mandatory = $true)][string]$Path, [switch]$HashOnly, [switch]$AllowReparse)
    $fullRoot = [System.IO.Path]::GetFullPath($Path)
    $nodes = New-Object System.Collections.Generic.List[object]
    if (-not (Test-Path -LiteralPath $fullRoot)) {
        return [ordered]@{ path = $fullRoot; exists = $false; nodes = @() }
    }
    if (-not $AllowReparse) { Assert-NoReparseComponents -Path $fullRoot }
    $rootItem = Get-Item -LiteralPath $fullRoot -Force
    $rootItem.Refresh()
    if (($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw ('Snapshot root is a reparse point: {0}' -f $fullRoot)
    }
    if (-not $rootItem.PSIsContainer) {
        $rootFile = [ordered]@{
            relativePath = ''
            kind = 'file'
            bytes = [long]$rootItem.Length
            sha256 = (Get-FileHash -LiteralPath $fullRoot -Algorithm SHA256).Hash.ToLowerInvariant()
            attributes = [string]$rootItem.Attributes
            creationUtc = $rootItem.CreationTimeUtc.ToString('o')
            lastWriteUtc = $rootItem.LastWriteTimeUtc.ToString('o')
            streams = @(Get-UatFileStreams -Path $fullRoot)
        }
        if (-not $HashOnly) { $rootFile.ownerDaclSddl = Get-UatAclSddl -Path $fullRoot }
        return [ordered]@{ path = $fullRoot; exists = $true; nodes = @($rootFile) }
    }
    $rootDirectory = [ordered]@{
        relativePath = '.'
        kind = 'directory'
        attributes = [string]$rootItem.Attributes
        creationUtc = $rootItem.CreationTimeUtc.ToString('o')
        lastWriteUtc = $rootItem.LastWriteTimeUtc.ToString('o')
    }
    if (-not $HashOnly) { $rootDirectory.ownerDaclSddl = Get-UatAclSddl -Path $fullRoot }
    [void]$nodes.Add($rootDirectory)
    $stack = New-Object 'System.Collections.Generic.Stack[string]'
    $stack.Push($fullRoot)
    while ($stack.Count -gt 0) {
        $directory = $stack.Pop()
        $children = @(Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop | Sort-Object Name)
        foreach ($item in $children) {
            $item.Refresh()
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                if (-not $AllowReparse) { throw ('Snapshot refused a reparse child: {0}' -f $item.FullName) }
                $relative = $item.FullName.Substring($fullRoot.TrimEnd('\').Length).TrimStart('\')
                [void]$nodes.Add([ordered]@{
                    relativePath = $relative
                    kind = 'reparse-point-not-followed'
                    attributes = [string]$item.Attributes
                    creationUtc = $item.CreationTimeUtc.ToString('o')
                    lastWriteUtc = $item.LastWriteTimeUtc.ToString('o')
                })
                continue
            }
            $relative = $item.FullName.Substring($fullRoot.TrimEnd('\').Length).TrimStart('\')
            if ($item.PSIsContainer) {
                $directoryRow = [ordered]@{
                    relativePath = $relative
                    kind = 'directory'
                    attributes = [string]$item.Attributes
                    creationUtc = $item.CreationTimeUtc.ToString('o')
                    lastWriteUtc = $item.LastWriteTimeUtc.ToString('o')
                }
                if (-not $HashOnly) { $directoryRow.ownerDaclSddl = Get-UatAclSddl -Path $item.FullName }
                [void]$nodes.Add($directoryRow)
                $stack.Push($item.FullName)
            }
            else {
                $fileRow = [ordered]@{
                    relativePath = $relative
                    kind = 'file'
                    bytes = [long]$item.Length
                    sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
                    attributes = [string]$item.Attributes
                    creationUtc = $item.CreationTimeUtc.ToString('o')
                    lastWriteUtc = $item.LastWriteTimeUtc.ToString('o')
                    streams = @(Get-UatFileStreams -Path $item.FullName)
                }
                if (-not $HashOnly) { $fileRow.ownerDaclSddl = Get-UatAclSddl -Path $item.FullName }
                [void]$nodes.Add($fileRow)
            }
        }
    }
    $sortedNodes = @($nodes.ToArray() | Sort-Object -CaseSensitive { $_.relativePath }, { $_.kind })
    return [ordered]@{ path = $fullRoot; exists = $true; nodes = $sortedNodes }
}

function Get-UatRegistrySnapshot {
    param([Parameter(Mandatory = $true)][Microsoft.Win32.RegistryHive]$Hive, [Parameter(Mandatory = $true)][string]$SubKey)
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($Hive, [Microsoft.Win32.RegistryView]::Registry64)
    try { $key = $base.OpenSubKey($SubKey, $false) }
    finally { $base.Dispose() }
    if ($null -eq $key) { return [ordered]@{ exists = $false; hive = [string]$Hive; subKey = $SubKey; values = @() } }
    try {
        $values = New-Object System.Collections.Generic.List[object]
        foreach ($name in @($key.GetValueNames() | Sort-Object)) {
            $kind = $key.GetValueKind($name)
            $value = $key.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            if ($kind -eq [Microsoft.Win32.RegistryValueKind]::Binary -or $kind -eq [Microsoft.Win32.RegistryValueKind]::None) {
                $encoded = [Convert]::ToBase64String([byte[]]$value)
            }
            elseif ($kind -eq [Microsoft.Win32.RegistryValueKind]::MultiString) {
                $encoded = @([string[]]$value)
            }
            else { $encoded = $value }
            [void]$values.Add([ordered]@{ name = $name; kind = [string]$kind; value = $encoded })
        }
        return [ordered]@{ exists = $true; hive = [string]$Hive; subKey = $SubKey; values = @($values.ToArray()) }
    }
    finally { $key.Dispose() }
}

function Get-UatTreeContentIdentity {
    param([Parameter(Mandatory = $true)]$Snapshot)
    $rows = New-Object System.Collections.Generic.List[object]
    foreach ($node in @($Snapshot.nodes)) {
        if ([string]$node.relativePath -match '^webview(?:[\\/]|$)') { continue }
        $row = [ordered]@{ relativePath = [string]$node.relativePath; kind = [string]$node.kind }
        if ([string]$node.kind -eq 'file') {
            $row.bytes = [long]$node.bytes
            $row.sha256 = [string]$node.sha256
            $row.streams = @($node.streams)
        }
        [void]$rows.Add($row)
    }
    return ,($rows.ToArray())
}

function Test-UatSnapshotEqual {
    param([Parameter(Mandatory = $true)]$Expected, [Parameter(Mandatory = $true)]$Actual, [Parameter(Mandatory = $true)][string]$Message)
    $left = ConvertTo-Json -InputObject $Expected -Depth 30 -Compress
    $right = ConvertTo-Json -InputObject $Actual -Depth 30 -Compress
    if ($left -cne $right) { throw ('{0}; before and after snapshots differ.' -f $Message) }
}

function Get-UatShortcutSnapshot {
    param([Parameter(Mandatory = $true)][string]$Path)
    return Get-UatTreeSnapshot -Path $Path
}

function Get-UatInstallSnapshot {
    param([Parameter(Mandatory = $true)][string]$Root, [Parameter(Mandatory = $true)][string]$RegistryKey, [Parameter(Mandatory = $true)][string]$StartMenuShortcut, [Parameter(Mandatory = $true)][string]$DesktopShortcut, [switch]$AllowReparse)
    $startDirectory = Split-Path -Parent $StartMenuShortcut
    return [ordered]@{
        root = Get-UatTreeSnapshot -Path $Root -AllowReparse:$AllowReparse
        registry = Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $RegistryKey
        startMenuShortcut = Get-UatShortcutSnapshot -Path $StartMenuShortcut
        desktopShortcut = Get-UatShortcutSnapshot -Path $DesktopShortcut
        startMenuDirectory = Get-UatTreeSnapshot -Path $startDirectory
    }
}

function Get-UatProductionSnapshot {
    param([Parameter(Mandatory = $true)][string]$ProductionRegistryKey)
    $localPrograms = Join-Path $env:LOCALAPPDATA 'Programs\SDIR'
    $startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\SDIR\SDIR.lnk'
    $desktop = Join-Path $env:USERPROFILE 'Desktop\SDIR.lnk'
    $profile = Join-Path $env:LOCALAPPDATA 'SDIR'
    return [ordered]@{
        productionRegistry = Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $ProductionRegistryKey
        productionStartMenu = Get-UatShortcutSnapshot -Path $startMenu
        productionDesktop = Get-UatShortcutSnapshot -Path $desktop
        productionInstallRoot = Get-UatTreeSnapshot -Path $localPrograms -HashOnly
        productionProfile = Get-UatTreeSnapshot -Path $profile -HashOnly
    }
}

function Get-UatFileHash {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

$script:stableAppExpected = '766bf2aaf832eba9812b0d97f7dd207c0278ae9bd8c332de054548cbf6719740'
$script:stablePortableExpected = '6aa32159140ce9ed0ff1bb06a486651290cb0e74df7e7dd37104ae720fcea4d5'
$script:baselineAppExpected = 'b158955e2b1c66883c7240ad1d7d77341d7934c1b881100c3eb726101313f8fb'
$script:productionAppId = 'SDIR-PerUser-4E7E0A18-79E0-41C0-97C5-65A10A79B3F4'
$script:testAppId = 'SDIR-UAT-20261117-9E39A20D-72E4-4D1D-AC7E-E65F4E2F1861'
$script:profileMarkerName = '.sdir-installer-test-profile'
$script:profileMarkerValue = 'SDIR-INSTALLER-UAT-PROFILE-V1'
$script:portablePath = Join-Path $repoRoot 'dist\SDIR-windows-x64-portable.zip'
$script:manifestPath = Join-Path $repoRoot 'packaging\windows-installer\payload.json'
$script:readmePath = Join-Path $repoRoot 'packaging\windows-installer\README-ko.txt'
$script:builderPath = Join-Path $repoRoot 'scripts\package-installer.ps1'
$script:compilerMetadataPath = Join-Path $repoRoot 'docs\task-id\windows-installer\compiler.json'
$script:builds = [ordered]@{}
$script:profileFixture = $null
$script:testRootName = $null
$script:testRoot = $null
$script:profileRoot = $null
$script:unrelatedSentinelRoot = $null
$script:unrelatedSentinelPath = $null
$script:unrelatedSentinelHash = $null
$script:startMenuDirectoryPreexisting = $false
$script:startMenuShortcut = $null
$script:desktopShortcut = $null
$script:registryKey = $null
$script:debugPort = 0
$script:appSemanticExpected = $null
$script:productionSnapshotBefore = $null
$script:stablePayloadEntries = @()

function Assert-UatNtfsVolume {
    param([Parameter(Mandatory = $true)][string]$Path)
    $driveRoot = [System.IO.Path]::GetPathRoot([System.IO.Path]::GetFullPath($Path))
    $drive = New-Object System.IO.DriveInfo -ArgumentList $driveRoot
    if (-not $drive.IsReady -or $drive.DriveFormat -ne 'NTFS') {
        throw ('Required fixture volume is not a ready NTFS volume: {0}' -f $driveRoot)
    }
    return $driveRoot
}

function New-UatDebugPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return [int]$listener.LocalEndpoint.Port
    }
    finally { $listener.Stop() }
}

function ConvertTo-UatNativePathValue {
    param([Parameter(Mandatory = $true)][string]$Path)
    $units = New-Object System.Collections.Generic.List[int]
    foreach ($character in $Path.ToCharArray()) { [void]$units.Add([int][char]$character) }
    return ,([int[]]$units.ToArray())
}

function New-UatTabState {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string[]]$Back = @(),
        [string[]]$Forward = @(),
        [string]$Filter = '',
        [string[]]$Selection = @(),
        [AllowNull()][string]$Cursor = $null,
        [AllowNull()][string]$Anchor = $null,
        [double]$Scroll = 0,
        [bool]$Pinned = $false
    )
    $backValues = New-Object System.Collections.Generic.List[object]
    foreach ($value in $Back) { [void]$backValues.Add((ConvertTo-UatNativePathValue $value)) }
    $forwardValues = New-Object System.Collections.Generic.List[object]
    foreach ($value in $Forward) { [void]$forwardValues.Add((ConvertTo-UatNativePathValue $value)) }
    $selectionValues = New-Object System.Collections.Generic.List[object]
    foreach ($value in $Selection) { [void]$selectionValues.Add((ConvertTo-UatNativePathValue $value)) }
    return [ordered]@{
        path = ConvertTo-UatNativePathValue $Path
        back = @($backValues.ToArray())
        forward = @($forwardValues.ToArray())
        filter = $Filter
        sort = 'Name'
        descending = $false
        scroll = $Scroll
        selection = @($selectionValues.ToArray())
        cursor = if ($Cursor) { ConvertTo-UatNativePathValue $Cursor } else { $null }
        anchor = if ($Anchor) { ConvertTo-UatNativePathValue $Anchor } else { $null }
        pinned = $Pinned
    }
}

function New-UatProfileFixture {
    if (Test-Path -LiteralPath $script:profileRoot) {
        throw ('An unowned preexisting test profile exists; it was left untouched: {0}' -f $script:profileRoot)
    }
    [void](Assert-UatNtfsVolume -Path $script:profileRoot)
    Assert-NoReparseComponents -Path (Split-Path -Parent $script:profileRoot)
    New-Item -ItemType Directory -Path $script:profileRoot | Out-Null
    $markerPath = Join-Path $script:profileRoot $script:profileMarkerName
    [System.IO.File]::WriteAllText($markerPath, $script:profileMarkerValue, [System.Text.Encoding]::ASCII)
    $filesRoot = Join-Path $script:profileRoot 'UAT-Files'
    $paneOne = Join-Path $filesRoot 'Pane-One'
    $paneOneNested = Join-Path $paneOne 'Nested'
    $paneTwo = Join-Path $filesRoot 'Pane-Two'
    foreach ($directory in @($filesRoot, $paneOne, $paneOneNested, $paneTwo)) {
        New-Item -ItemType Directory -Path $directory -Force | Out-Null
    }
    $fixtureBytes = [ordered]@{
        'alpha.txt' = [System.Text.Encoding]::UTF8.GetBytes('SDIR UAT alpha ' + $script:runId)
        'beta.txt' = [System.Text.Encoding]::UTF8.GetBytes('SDIR UAT beta ' + $script:runId)
        'nested.txt' = [System.Text.Encoding]::UTF8.GetBytes('SDIR UAT nested ' + $script:runId)
        'gamma.txt' = [System.Text.Encoding]::UTF8.GetBytes('SDIR UAT gamma ' + $script:runId)
    }
    [System.IO.File]::WriteAllBytes((Join-Path $paneOne 'alpha.txt'), $fixtureBytes['alpha.txt'])
    [System.IO.File]::WriteAllBytes((Join-Path $paneOne 'beta.txt'), $fixtureBytes['beta.txt'])
    [System.IO.File]::WriteAllBytes((Join-Path $paneOneNested 'nested.txt'), $fixtureBytes['nested.txt'])
    [System.IO.File]::WriteAllBytes((Join-Path $paneTwo 'gamma.txt'), $fixtureBytes['gamma.txt'])
    $alpha = Join-Path $paneOne 'alpha.txt'
    $beta = Join-Path $paneOne 'beta.txt'
    $nestedFile = Join-Path $paneOneNested 'nested.txt'
    $gamma = Join-Path $paneTwo 'gamma.txt'
    $panes = New-Object System.Collections.ArrayList
    [void]$panes.Add([object[]]@($paneOne, $paneOneNested))
    [void]$panes.Add([object[]]@($paneTwo))
    [void]$panes.Add([object[]]@())
    [void]$panes.Add([object[]]@())
    $paneStates = New-Object System.Collections.ArrayList
    [void]$paneStates.Add([object[]]@(
        (New-UatTabState -Path $paneOne -Back @($filesRoot) -Forward @($paneOneNested) -Selection @($alpha) -Cursor $alpha -Anchor $alpha -Scroll 16),
        (New-UatTabState -Path $paneOneNested -Back @($paneOne) -Selection @($nestedFile) -Cursor $nestedFile -Anchor $nestedFile -Pinned $true)
    ))
    [void]$paneStates.Add([object[]]@(
        (New-UatTabState -Path $paneTwo -Selection @($gamma) -Cursor $gamma -Anchor $gamma)
    ))
    [void]$paneStates.Add([object[]]@())
    [void]$paneStates.Add([object[]]@())
    $paneNames = @('UAT 왼쪽 그룹', 'UAT 오른쪽 그룹', '패널 3', '패널 4')
    $views = @('details', 'list', 'details', 'details')
    $workspace = [ordered]@{
        name = 'UAT 복원 작업공간'
        layout = 2
        panes = $panes.ToArray()
        pane_names = $paneNames
        active_tabs = @(1, 0, 0, 0)
        active_pane = 1
        pane_views = $views
        pane_columns = @()
        pane_icon_sizes = @()
        tab_states = $paneStates.ToArray()
    }
    $settings = [ordered]@{
        version = 1
        layout = 2
        panes = $panes.ToArray()
        favorites = @()
        show_hidden = $false
        pane_names = $paneNames
        theme = 'dark'
        compact = $false
        search_preferences = [ordered]@{ instant = $false; select_on_focus = $true; focus_on_open = $true; memory_mib = 512 }
        active_tabs = @(1, 0, 0, 0)
        active_pane = 1
        workspaces = @($workspace)
        pane_views = $views
        pane_columns = @()
        pane_icon_sizes = @()
        tab_states = $paneStates.ToArray()
        window = [ordered]@{ x = 30; y = 35; width = 1240.0; height = 820.0; maximized = $false }
        tree_expanded = @((ConvertTo-UatNativePathValue $paneOne), (ConvertTo-UatNativePathValue $paneTwo))
        tree_follow = $true
        tree_open = $true
    }
    $settingsPath = Join-Path $script:profileRoot 'settings.json'
    Write-UatJsonFile -Path $settingsPath -Value $settings
    $script:profileFixture = [ordered]@{
        root = $script:profileRoot
        marker = $markerPath
        markerValue = $script:profileMarkerValue
        settings = $settingsPath
        filesRoot = $filesRoot
        paneOne = $paneOne
        paneOneNested = $paneOneNested
        paneTwo = $paneTwo
        alpha = $alpha
        beta = $beta
        nestedFile = $nestedFile
        gamma = $gamma
        expectedSettings = $settings
    }
    $script:appSemanticExpected = [ordered]@{
        theme = 'dark'
        layout = 2
        activePane = 1
        groupNames = $paneNames[0..1]
        paneOneTabs = @($paneOne, $paneOneNested)
        paneOneActive = $paneOneNested
        paneTwoActive = $paneTwo
        workspaceName = 'UAT 복원 작업공간'
        visibleFileNames = @('nested.txt', 'gamma.txt')
    }
}

function Initialize-UatPaths {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object System.Security.Principal.WindowsPrincipal -ArgumentList $identity
    Assert-Uat (-not $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) 'Run the UAT as an unelevated standard user; do not accept or automate UAC.'
    if (-not $env:LOCALAPPDATA -or -not $env:APPDATA -or -not $env:USERPROFILE) {
        throw 'Windows user profile environment paths are unavailable.'
    }
    $localAppData = [System.IO.Path]::GetFullPath($env:LOCALAPPDATA)
    if ($localAppData -notmatch '^[A-Za-z]:\\[A-Za-z0-9 _.-]+(?:\\[A-Za-z0-9 _.-]+)*$') {
        throw ('LOCALAPPDATA is not an ASCII local path accepted by the test builder: {0}' -f $localAppData)
    }
    $script:testRootName = 'SDIR-UAT-' + $script:runId
    $script:testRoot = [System.IO.Path]::GetFullPath((Join-Path (Join-Path $localAppData 'Programs') $script:testRootName))
    $script:initialTestRoot = $script:testRoot
    $script:initialTestRootName = $script:testRootName
    $script:profileRoot = [System.IO.Path]::GetFullPath((Join-Path $localAppData ('SDIR-UAT-Profile-' + $script:runId)))
    $script:registryKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $script:testAppId + '_is1'
    $script:startMenuShortcut = Join-Path (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\SDIR UAT') 'SDIR UAT.lnk'
    $script:desktopShortcut = Join-Path (Join-Path $env:USERPROFILE 'Desktop') 'SDIR UAT.lnk'
    $script:startMenuDirectoryPreexisting = Test-Path -LiteralPath (Split-Path -Parent $script:startMenuShortcut) -PathType Container
    $script:debugPort = New-UatDebugPort
    [void](Assert-UatNtfsVolume -Path $script:testRoot)
    Assert-NoReparseComponents -Path (Split-Path -Parent $script:testRoot)
    $preexistingIdentity = Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey
    $preexistingStart = Test-Path -LiteralPath $script:startMenuShortcut
    $preexistingDesktop = Test-Path -LiteralPath $script:desktopShortcut
    foreach ($candidateRoot in @($script:testRoot,
            (Join-Path (Split-Path -Parent $script:testRoot) ($script:testRootName + '-PORTABLE')),
            (Join-Path (Split-Path -Parent $script:testRoot) ($script:testRootName + '-SEPARATE')),
            (Join-Path (Split-Path -Parent $script:testRoot) ($script:testRootName + '-FRESH')),
            (Join-Path (Split-Path -Parent $script:testRoot) ($script:testRootName + '-SILENT')))) {
        if (Test-Path -LiteralPath $candidateRoot) {
            throw ('A preexisting test root was not created by this run and remains untouched: {0}' -f $candidateRoot)
        }
    }
    if ($preexistingIdentity.exists -or $preexistingStart -or $preexistingDesktop) {
        throw 'A preexisting test identity or shortcut exists; no existing test registration or shortcut was changed.'
    }
    $existingTestGroup = Split-Path -Parent $script:startMenuShortcut
    if ((Test-Path -LiteralPath $existingTestGroup -PathType Container) -and @(Get-ChildItem -LiteralPath $existingTestGroup -Force).Count -gt 0) {
        throw ('A preexisting Start Menu test group is not owned by this run and remains untouched: {0}' -f $existingTestGroup)
    }
    if (Test-Path -LiteralPath $script:profileRoot) {
        throw ('A preexisting test profile was not created by this run and remains untouched: {0}' -f $script:profileRoot)
    }
    if (Test-Path -LiteralPath $providedEvidencePath) {
        throw ('EvidenceDirectory already exists and will not be overwritten: {0}' -f $providedEvidencePath)
    }
    if (-not (Test-Path -LiteralPath $staticEvidenceRoot -PathType Container)) {
        throw ('The diagnostic evidence parent must already exist and is left unchanged: {0}' -f $staticEvidenceRoot)
    }
    Assert-NoReparseComponents -Path $staticEvidenceRoot
    Assert-NoReparseComponents -Path (Split-Path -Parent $providedEvidencePath)
    $createdEvidenceDirectory = New-Item -ItemType Directory -Path $providedEvidencePath -ErrorAction Stop
    $createdEvidencePath = [System.IO.Path]::GetFullPath([string]$createdEvidenceDirectory.FullName)
    Assert-Uat ([string]::Equals($createdEvidencePath, $providedEvidencePath, [System.StringComparison]::OrdinalIgnoreCase)) 'The newly created evidence directory did not resolve to the requested path.'
    $script:evidenceDirectoryCreated = $true
    $ownerCandidateRecords = [ordered]@{}
    foreach ($candidateKey in $script:diagnosticCandidateKeys) {
        if ($candidateKey -ceq 'portable') { $buildName = 'stable-portable-root'; $suffix = '-PORTABLE' }
        elseif ($candidateKey -ceq 'separate') { $buildName = 'stable-separate-root'; $suffix = '-SEPARATE' }
        elseif ($candidateKey -ceq 'fresh') { $buildName = 'stable-fresh-root'; $suffix = '-FRESH' }
        else { throw ('Unsupported diagnostic candidate key: {0}' -f $candidateKey) }
        $candidateRootName = $script:initialTestRootName + $suffix
        $candidateRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $script:initialTestRoot) $candidateRootName))
        $candidatePath = [System.IO.Path]::GetFullPath((Join-Path $providedEvidencePath ($buildName + '.exe')))
        $ownerCandidateRecords[$candidateKey] = [ordered]@{
            name = $buildName; path = $candidatePath; rootName = $candidateRootName; root = $candidateRoot
            setupBytes = $null; setupSha256 = $null; compilerExitCode = $null
            receiptPath = $null; receiptSha256 = $null; provenancePath = $null; provenanceSha256 = $null
        }
    }
    $silentCandidate = $null
    if ($script:selectedCases[0] -ceq 'A16') {
        $silentCandidateRootName = $script:initialTestRootName + '-SILENT'
        $silentCandidateRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $script:initialTestRoot) $silentCandidateRootName))
        $silentCandidatePath = [System.IO.Path]::GetFullPath((Join-Path $providedEvidencePath 'stable-silent-root.exe'))
        $silentCandidate = [ordered]@{
            name = 'stable-silent-root'; path = $silentCandidatePath; rootName = $silentCandidateRootName; root = $silentCandidateRoot
            setupBytes = $null; setupSha256 = $null; compilerExitCode = $null
            receiptPath = $null; receiptSha256 = $null; provenancePath = $null
        }
    }
    $script:runOwner = [ordered]@{
        marker = 'SDIR-INSTALLER-UAT-RUN-V1'
        runId = $script:runId
        script = [System.IO.Path]::GetFullPath($PSCommandPath)
        createdUtc = $script:startedUtc
        evidenceDirectory = $providedEvidencePath
        testRoot = $script:initialTestRoot
        profileRoot = $script:profileRoot
        rootName = $script:initialTestRootName
        debugPort = $script:debugPort
        explicitDiagnostic = $true
        selectedCases = @($script:selectedCases)
        preflight = $script:preflightResult
        silentCandidate = $silentCandidate
        candidateOwners = $ownerCandidateRecords
        candidateProvenancePath = $null
        candidateProvenanceSha256 = $null
    }
    Write-UatJsonFile -Path $script:runOwnerPath -Value $script:runOwner
    $script:runOwnerWritten = $true
    New-Item -ItemType Directory -Path $script:commandDirectory -ErrorAction Stop | Out-Null
    [System.IO.File]::WriteAllText($script:eventsPath, '', (New-Object System.Text.UTF8Encoding -ArgumentList $false))
    [System.IO.File]::WriteAllText($script:commandsPath, '', (New-Object System.Text.UTF8Encoding -ArgumentList $false))
    Add-UatEvent -Kind 'run-started' -Name 'installer-uat' -Data @{
        runId = $script:runId; rootName = $script:initialTestRootName; testRoot = $script:initialTestRoot
        profileRoot = $script:profileRoot; debugPort = $script:debugPort; explicitDiagnostic = $true
        selectedCases = @($script:selectedCases); silentCandidateRoot = if ($null -ne $silentCandidate) { $silentCandidate.root } else { $null }
        candidateOwners = $script:runOwner.candidateOwners
    }
    $script:productionSnapshotBefore = Get-UatProductionSnapshot -ProductionRegistryKey ('Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $script:productionAppId + '_is1')
    $script:stableAppHashBefore = Get-UatFileHash (Join-Path $repoRoot 'dist\SDIR.exe')
    $script:stablePortableHashBefore = Get-UatFileHash $script:portablePath
    if ($script:stableAppHashBefore -ne $script:stableAppExpected -or $script:stablePortableHashBefore -ne $script:stablePortableExpected) {
        throw 'Stable app or portable archive did not match the reviewed pre-run SHA-256 values.'
    }
    New-UatProfileFixture
    $script:unrelatedSentinelRoot = Join-Path $env:APPDATA ('SDIR-UAT-Unrelated-' + $script:runId)
    Assert-Uat (-not (Test-Path -LiteralPath $script:unrelatedSentinelRoot)) 'The unique unrelated-AppData sentinel path already exists.'
    New-Item -ItemType Directory -Path $script:unrelatedSentinelRoot | Out-Null
    $unrelatedOwner = Join-Path $script:unrelatedSentinelRoot '.sdir-installer-test-unrelated'
    [System.IO.File]::WriteAllText($unrelatedOwner, ('SDIR-INSTALLER-UAT-UNRELATED-V1|' + $script:runId), [System.Text.Encoding]::ASCII)
    $script:unrelatedSentinelPath = Join-Path $script:unrelatedSentinelRoot 'unrelated-data.bin'
    [System.IO.File]::WriteAllBytes($script:unrelatedSentinelPath, [System.Text.Encoding]::UTF8.GetBytes('SDIR-UAT-UNRELATED-' + $script:runId))
    $script:unrelatedSentinelHash = Get-UatFileHash $script:unrelatedSentinelPath
    Write-UatResult
}

function Get-UatPayloadManifest {
    $manifest = Get-Content -LiteralPath $script:manifestPath -Raw | ConvertFrom-Json
    Assert-Uat ($manifest.schemaVersion -eq 1) 'payload.json schema version changed.'
    Assert-Uat ($manifest.appSha256 -eq $script:stableAppExpected) 'payload.json stable app SHA-256 changed.'
    Assert-Uat ($manifest.portableSha256 -eq $script:stablePortableExpected) 'payload.json stable portable SHA-256 changed.'
    Assert-Uat ($manifest.files.Count -eq 18) 'The reviewed stable payload must contain exactly 18 files.'
    $destinations = @{}
    foreach ($entry in $manifest.files) {
        Assert-Uat ([string]$entry.source -match '^dist/[A-Za-z0-9._/-]+$') ('Unsafe payload source: {0}' -f $entry.source)
        Assert-Uat ([string]$entry.destination -match '^[A-Za-z0-9._/-]+$') ('Unsafe payload destination: {0}' -f $entry.destination)
        Assert-Uat (-not $destinations.ContainsKey([string]$entry.destination)) ('Duplicate payload destination: {0}' -f $entry.destination)
        $destinations[[string]$entry.destination] = $true
        $source = Join-Path $repoRoot ([string]$entry.source -replace '/', '\')
        Assert-Uat (Test-Path -LiteralPath $source -PathType Leaf) ('Payload source is missing: {0}' -f $source)
        $item = Get-Item -LiteralPath $source
        Assert-Uat ([long]$item.Length -eq [long]$entry.bytes) ('Payload source bytes changed: {0}' -f $source)
        Assert-Uat ((Get-UatFileHash $source) -eq [string]$entry.sha256) ('Payload source hash changed: {0}' -f $source)
    }
    $portableReference = @($manifest.files | Where-Object { $_.source -eq 'dist/docs/distribution.md' })
    Assert-Uat ($portableReference.Count -eq 1 -and $portableReference[0].destination -eq 'docs/distribution-portable-reference.md') 'The historical portable guide must be staged at docs/distribution-portable-reference.md.'
    $readme = [System.IO.File]::ReadAllText($script:readmePath)
    Assert-Uat ($readme.Contains('docs/distribution-portable-reference.md') -and $readme.Contains('과거 포터블 안내서')) 'README-ko.txt must distinguish the historical portable guide from the installer workflow guide.'
    $script:stablePayloadEntries = @($manifest.files | ForEach-Object {
        [ordered]@{ source = [string]$_.source; destination = [string]$_.destination; bytes = [long]$_.bytes; sha256 = [string]$_.sha256 }
    })
    return $manifest
}

function Build-UatInstaller {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$RootName,
        [switch]$Baseline,
        [switch]$MissingRuntime,
        [ValidateSet('None', 'Backup', 'Partial', 'Late')][string]$Fault = 'None'
    )
    $powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    if (-not (Test-Path -LiteralPath $powershell -PathType Leaf)) {
        throw ('Verified absolute Windows PowerShell path is unavailable: {0}' -f $powershell)
    }
    $outputPath = Join-Path $providedEvidencePath ($Name + '.exe')
    $arguments = @('-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $script:builderPath,
        '-OutputPath', $outputPath, '-TestBuild', '-TestRootName', $RootName,
        '-TestProfileDirectory', $script:profileRoot, '-TestDebugPort', [string]$script:debugPort)
    if ($Baseline) { $arguments += '-TestBaseline' }
    if ($MissingRuntime) { $arguments += '-TestMissingRuntime' }
    if ($Fault -ne 'None') { $arguments += @('-TestFault', $Fault) }
    $run = Invoke-UatCommand -Executable $powershell -Argument $arguments -WorkingDirectory $repoRoot -Name ('build-' + $Name) -TimeoutSeconds 300
    if ($run.exitCode -ne 0) {
        throw ('TestBuild candidate {0} failed to compile; full raw logs are retained at {1}.' -f $Name, $run.evidence.directory)
    }
    $outputPath = [System.IO.Path]::GetFullPath($outputPath)
    $receiptPath = $outputPath + '.build.json'
    Assert-Uat (Test-Path -LiteralPath $outputPath -PathType Leaf) ('TestBuild candidate is missing: {0}' -f $outputPath)
    Assert-Uat (Test-Path -LiteralPath $receiptPath -PathType Leaf) ('TestBuild receipt is missing: {0}' -f $receiptPath)
    $receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
    Assert-Uat ($receipt.schemaVersion -eq 1) ('Candidate {0} receipt schema changed.' -f $Name)
    Assert-Uat ($null -ne $receipt.PSObject.Properties['compilerExitCode'] -and $null -ne $receipt.compilerExitCode -and $receipt.compilerExitCode -eq 0) ('Candidate {0} raw receipt compiler exit code was missing or not zero.' -f $Name)
    Assert-Uat ([string]::Equals([string]$receipt.artifactKind, 'candidate-only', [System.StringComparison]::Ordinal)) ('Candidate {0} receipt is not candidate-only.' -f $Name)
    Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$receipt.outputPath), $outputPath, [System.StringComparison]::OrdinalIgnoreCase)) ('Candidate {0} receipt output path does not identify the actual EXE.' -f $Name)
    $setupInfo = Get-Item -LiteralPath $outputPath
    $setupSha256 = Get-UatFileHash $outputPath
    Assert-Uat ([long]$receipt.setupBytes -eq [long]$setupInfo.Length) ('Candidate {0} receipt byte count does not match the actual EXE.' -f $Name)
    Assert-Uat ([string]::Equals([string]$receipt.setupSha256, $setupSha256, [System.StringComparison]::OrdinalIgnoreCase)) ('Candidate {0} receipt hash does not match the actual EXE.' -f $Name)
    Assert-Uat ($run.exitCode -eq 0) ('Candidate {0} builder process exit code was not zero.' -f $Name)
    Assert-Uat ($receipt.testBuild -eq $true) ('Candidate {0} is not a TEST_BUILD identity.' -f $Name)
    Assert-Uat ($receipt.identity.appId -eq $script:testAppId) ('Candidate {0} has an unexpected test AppId.' -f $Name)
    Assert-Uat ($receipt.identity.appId -ne $script:productionAppId) 'TestBuild candidate reused the production AppId.'
    Assert-Uat ($receipt.identity.rootName -eq $RootName) ('Candidate {0} was compiled for the wrong root.' -f $Name)
    Assert-Uat ($receipt.testRootName -eq $RootName -and $receipt.testProfileDirectory -eq $script:profileRoot) ('Candidate {0} is not bound to the owned test root/profile.' -f $Name)
    Assert-Uat ($receipt.testDebugPort -eq $script:debugPort) ('Candidate {0} has a different debug port.' -f $Name)
    Assert-Uat ($receipt.testLauncher -eq 'sdir-installer-test-launcher.cmd') ('Candidate {0} lacks the isolated launcher.' -f $Name)
    Assert-Uat ($receipt.stableAppSha256 -eq $script:stableAppExpected) ('Candidate {0} does not retain the reviewed stable source identity.' -f $Name)
    Assert-Uat ($receipt.stablePortableSha256Before -eq $script:stablePortableExpected -and $receipt.stablePortableSha256After -eq $script:stablePortableExpected) ('Candidate {0} changed or misidentified the portable archive.' -f $Name)
    Assert-Uat ($receipt.testFault -eq $Fault) ('Candidate {0} has an unexpected test fault define.' -f $Name)
    Assert-Uat ($receipt.testMissingRuntime -eq [bool]$MissingRuntime) ('Candidate {0} has an unexpected runtime test define.' -f $Name)
    Assert-Uat ($receipt.testBaseline -eq [bool]$Baseline) ('Candidate {0} has an unexpected B158 test-payload identity.' -f $Name)
    Assert-Uat ($receipt.verification -eq 'compile-and-payload-static-checks-only; Windows UAT not claimed') ('Candidate {0} misstates its compilation evidence.' -f $Name)
    $expectedApp = if ($Baseline) { $script:baselineAppExpected } else { $script:stableAppExpected }
    Assert-Uat ($receipt.embeddedAppSha256 -eq $expectedApp) ('Candidate {0} embeds the wrong app bytes.' -f $Name)
    $knownInventoryIds = @('production-stable766-v1', 'test-stable766-v1', 'test-baselineb158-v1')
    Assert-Uat ($knownInventoryIds -ccontains [string]$receipt.activeInventoryId) ('Candidate {0} has an unknown active inventory selector.' -f $Name)
    $expectedInventoryId = if (-not [bool]$receipt.testBuild) { 'production-stable766-v1' }
        elseif ([bool]$receipt.testBaseline) { 'test-baselineb158-v1' }
        else { 'test-stable766-v1' }
    Assert-Uat ([string]::Equals([string]$receipt.activeInventoryId, $expectedInventoryId, [System.StringComparison]::Ordinal)) ('Candidate {0} has the wrong active inventory selector.' -f $Name)
    if ($Baseline) {
        Assert-Uat ($receipt.payloadInventory.Count -eq 20) ('Baseline candidate {0} must contain the B158 payload, stable docs, isolated launcher, and legacy entry.' -f $Name)
    }
    else { Assert-Uat ($receipt.payloadInventory.Count -eq 19) ('Candidate {0} must contain 18 stable entries plus its isolated launcher.' -f $Name) }
    $expectedDestinations = @($script:stablePayloadEntries | ForEach-Object { [string]$_.destination })
    $expectedDestinations += 'sdir-installer-test-launcher.cmd'
    if ($Baseline) { $expectedDestinations += 'legacy-test-only.txt' }
    $inventoryDestinations = @($receipt.payloadInventory | ForEach-Object { [string]$_.destination })
    Assert-Uat ($inventoryDestinations.Count -eq $expectedDestinations.Count) ('Candidate {0} has an incomplete payload inventory.' -f $Name)
    foreach ($destination in $expectedDestinations) {
        $destinationMatches = @($inventoryDestinations | Where-Object { [string]::Equals([string]$_, [string]$destination, [System.StringComparison]::Ordinal) })
        Assert-Uat ($destinationMatches.Count -eq 1) ('Candidate {0} omitted or duplicated payload destination {1}.' -f $Name, $destination)
    }
    foreach ($destination in $inventoryDestinations) {
        Assert-Uat ($expectedDestinations -ccontains $destination) ('Candidate {0} has an unexpected payload destination {1}.' -f $Name, $destination)
    }
    foreach ($entry in $script:stablePayloadEntries) {
        $built = @($receipt.payloadInventory | Where-Object { $_.destination -eq $entry.destination })
        Assert-Uat ($built.Count -eq 1) ('Candidate {0} omitted or duplicated {1}.' -f $Name, $entry.destination)
        if ($Baseline -and $entry.destination -eq 'SDIR.exe') {
            Assert-Uat ([string]$built[0].sha256 -eq $expectedApp) ('Baseline candidate {0} altered its B158 executable payload.' -f $Name)
        }
        else {
            Assert-Uat ($built[0].sha256 -eq $entry.sha256 -and [long]$built[0].bytes -eq [long]$entry.bytes) ('Candidate {0} altered stable payload {1}.' -f $Name, $entry.destination)
        }
    }
    if ($Baseline) {
        $historical = @($receipt.payloadInventory | Where-Object { [string]::Equals([string]$_.destination, 'legacy-test-only.txt', [System.StringComparison]::Ordinal) })
        Assert-Uat ($historical.Count -eq 1 -and [long]$historical[0].bytes -eq 25 -and [string]$historical[0].sha256 -ceq 'ea35e4c2c9646c4b489140707c1b87963d7f7912c85a6fc2e914b490fa818607') 'Baseline historical payload must match the approved 25-byte entry.'
    }
    $signature = Get-AuthenticodeSignature -FilePath ([string]$receipt.compilerProvenance.path)
    Assert-Uat ($receipt.compilerProvenance.version -eq '6.7.4') 'Build receipt does not identify Inno Setup 6.7.4.'
    Assert-Uat ($signature.Status -eq 'Valid' -and $signature.SignerCertificate.Subject -match 'Pyrsys B\.V\.') 'Installed ISCC.exe signature/publisher did not match compiler provenance.'
    $candidate = [pscustomobject]@{
        name = $Name
        rootName = $RootName
        installRoot = Join-Path (Join-Path $env:LOCALAPPDATA 'Programs') $RootName
        path = [System.IO.Path]::GetFullPath($outputPath)
        sha256 = $setupSha256
        setupBytes = [long]$setupInfo.Length
        compilerExitCode = [int]$receipt.compilerExitCode
        receiptPath = [System.IO.Path]::GetFullPath($receiptPath)
        receiptSha256 = Get-UatFileHash $receiptPath
        receipt = $receipt
        testBaseline = [bool]$Baseline
        testFault = $Fault
        testMissingRuntime = [bool]$MissingRuntime
        runtimeEvidence = [System.IO.Path]::GetFullPath($run.evidence.directory)
    }
    $script:builds[$Name] = $candidate
    [void]$script:candidateRecords.Add([ordered]@{
        name = $Name; path = $candidate.path; sha256 = $candidate.sha256; setupBytes = $candidate.setupBytes
        compilerExitCode = $candidate.compilerExitCode; receipt = $candidate.receiptPath; receiptSha256 = $candidate.receiptSha256
        testBuild = $true; testRootName = $RootName; embeddedAppSha256 = $receipt.embeddedAppSha256
        testFault = $Fault; testMissingRuntime = [bool]$MissingRuntime; testBaseline = [bool]$Baseline
        verification = $receipt.verification; runtimeExecuted = $false; provenancePath = $null
    })
    return $candidate
}

function Test-UatStaticProvenance {
    param([switch]$A16Only)
    $manifest = Get-UatPayloadManifest
    $compiler = Get-Content -LiteralPath $script:compilerMetadataPath -Raw | ConvertFrom-Json
    Assert-Uat ($compiler.version -eq '6.7.4') 'compiler.json version was not Inno Setup 6.7.4.'
    Assert-Uat ([long]$compiler.bytes -eq [long]10604576 -and [string]$compiler.sha256 -ceq 'a549929319f9120716677c685535ca4671e4b937b5fdc87e2585f32d8eb30e8c' -and [string]$compiler.authenticode -ceq 'Valid' -and [string]$compiler.publisher -ceq 'Pyrsys B.V.') 'compiler.json installer-package metadata did not match its reviewed pin.'
    $compilerPath = Join-Path ([string]$compiler.installedPath) 'ISCC.exe'
    Assert-Uat ([System.IO.Path]::IsPathRooted($compilerPath) -and (Test-Path -LiteralPath $compilerPath -PathType Leaf)) 'compiler.json does not identify an installed absolute ISCC.exe.'
    $compilerPath = [System.IO.Path]::GetFullPath($compilerPath)
    $compilerIsccSha256 = Get-UatFileHash $compilerPath
    Assert-Uat ($compilerIsccSha256 -ceq 'f02fa0e5f237b47d7ed1edae9b991866ec94fe02af61eb6a73e94f92fe8e0604') 'Installed ISCC.exe SHA-256 did not match the reviewed compiler pin.'
    $signature = Get-AuthenticodeSignature -FilePath $compilerPath
    Assert-Uat ($signature.Status -eq 'Valid' -and $signature.SignerCertificate.Subject -match 'Pyrsys B\.V\.') 'Installed compiler Authenticode publisher did not match the reviewed publisher.'
    $script:compilerReadiness = [ordered]@{
        metadataPath = [System.IO.Path]::GetFullPath($script:compilerMetadataPath)
        metadataSha256 = Get-UatFileHash $script:compilerMetadataPath
        version = [string]$compiler.version
        installerSha256 = [string]$compiler.sha256
        installerBytes = [long]$compiler.bytes
        installerAuthenticode = [string]$compiler.authenticode
        publisher = [string]$compiler.publisher
        isccPath = $compilerPath
        isccSha256 = $compilerIsccSha256
        isccSignatureStatus = [string]$signature.Status
        isccSigner = [string]$signature.SignerCertificate.Subject
    }
    if ($A16Only) {
        $testRootSilent = $script:testRootName + '-SILENT'
        $candidate = Build-UatInstaller -Name 'stable-silent-root' -RootName $testRootSilent
        $script:builds['silent'] = $candidate
        $script:sourceBuildIdentityEquivalence = $null
        $receipt = $candidate.receipt
        Assert-Uat ($receipt.schemaVersion -eq 1 -and [string]$receipt.artifactKind -ceq 'candidate-only') 'A16 candidate receipt schema or artifact kind changed.'
        Assert-Uat ([long]$candidate.compilerExitCode -eq [long]0) 'A16 candidate compiler did not return exit code 0.'
        $evidencePrefix = $providedEvidencePath.TrimEnd('\') + '\'
        Assert-Uat ($candidate.path.StartsWith($evidencePrefix, [System.StringComparison]::OrdinalIgnoreCase)) 'A16 candidate EXE is outside the newly owned evidence directory.'
        $receiptCompiler = $receipt.compilerProvenance
        Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$receiptCompiler.path), $compilerPath, [System.StringComparison]::OrdinalIgnoreCase)) 'A16 receipt compiler path did not match compiler.json.'
        Assert-Uat ([string]$receiptCompiler.version -ceq '6.7.4' -and [string]$receiptCompiler.installerSha256 -ceq [string]$compiler.sha256 -and [long]$receiptCompiler.installerBytes -eq [long]$compiler.bytes) 'A16 receipt compiler installer metadata did not match compiler.json.'
        Assert-Uat ([string]$receiptCompiler.authenticode -ceq [string]$compiler.authenticode -and [string]$receiptCompiler.publisher -ceq [string]$compiler.publisher) 'A16 receipt compiler signature metadata did not match compiler.json.'
        Assert-Uat ([string]$receiptCompiler.isccSha256 -ceq $compilerIsccSha256 -and [string]$receiptCompiler.isccSha256 -ceq 'f02fa0e5f237b47d7ed1edae9b991866ec94fe02af61eb6a73e94f92fe8e0604') 'A16 receipt ISCC.exe hash did not match the installed compiler.'
        Assert-Uat ($null -ne $receipt.sourceHashes) 'A16 candidate receipt omitted sourceHashes.'
        $sourceHashProperties = @($receipt.sourceHashes.PSObject.Properties)
        Assert-Uat ($sourceHashProperties.Count -gt 0) 'A16 candidate receipt sourceHashes was empty.'
        $sourceRecords = New-Object System.Collections.Generic.List[object]
        $repoPrefix = $repoRoot.TrimEnd('\') + '\'
        foreach ($sourceProperty in $sourceHashProperties) {
            $sourceName = [string]$sourceProperty.Name
            $expectedSourceHash = [string]$sourceProperty.Value
            if ([string]::Equals($sourceName, 'compiler/ISCC.exe', [System.StringComparison]::Ordinal)) {
                Assert-Uat ([string]::Equals($expectedSourceHash, $compilerIsccSha256, [System.StringComparison]::OrdinalIgnoreCase)) 'A16 sourceHashes compiler/ISCC.exe value did not match the installed compiler.'
                [void]$sourceRecords.Add([ordered]@{ name = $sourceName; path = $compilerPath; sha256 = $expectedSourceHash; comparison = 'installed-compiler' })
                continue
            }
            if ($sourceName -match '^(generated|staged)/') {
                [void]$sourceRecords.Add([ordered]@{ name = $sourceName; sha256 = $expectedSourceHash; comparison = 'not-mapped-to-repository' })
                continue
            }
            Assert-Uat ($sourceName -match '^[A-Za-z0-9._/-]+$' -and $sourceName -notmatch '(^|/)\.\.?(/|$)') ('A16 receipt source key was not a safe repository-relative path: {0}' -f $sourceName)
            $sourcePath = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ($sourceName -replace '/', '\')))
            Assert-Uat ($sourcePath.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)) ('A16 receipt source escaped the repository: {0}' -f $sourceName)
            Assert-Uat (Test-Path -LiteralPath $sourcePath -PathType Leaf) ('A16 receipt source file is missing: {0}' -f $sourceName)
            $actualSourceHash = Get-UatFileHash $sourcePath
            Assert-Uat ([string]::Equals($actualSourceHash, $expectedSourceHash, [System.StringComparison]::OrdinalIgnoreCase)) ('A16 receipt source hash changed: {0}' -f $sourceName)
            [void]$sourceRecords.Add([ordered]@{ name = $sourceName; path = $sourcePath; sha256 = $actualSourceHash; comparison = 'current-repository-file' })
        }
        $uatSourcePath = [System.IO.Path]::GetFullPath($PSCommandPath)
        $uatSourceSha256 = Get-UatFileHash $uatSourcePath
        $uiSourcePath = [System.IO.Path]::GetFullPath($helperPath)
        $uiSourceSha256 = Get-UatFileHash $uiSourcePath
        Assert-Uat ($uiSourceSha256 -ceq '4e5ac8db9d97212c073f8b8a7bc5efae5f34e0751347a347c81cdd0621547dd6') 'A16 UI helper SHA-256 did not match the reviewed harness pin.'
        $script:diagnosticCandidateEvidencePath = Join-Path $providedEvidencePath 'A16-candidate-provenance.json'
        $script:diagnosticCandidateRecord = [ordered]@{
            schemaVersion = 1
            artifactKind = 'candidate-only'
            name = [string]$candidate.name
            rootName = [string]$candidate.rootName
            installRoot = [System.IO.Path]::GetFullPath([string]$candidate.installRoot)
            outputPath = [System.IO.Path]::GetFullPath([string]$candidate.path)
            setupBytes = [long]$candidate.setupBytes
            setupSha256 = [string]$candidate.sha256
            compilerExitCode = [int]$candidate.compilerExitCode
            receiptPath = [System.IO.Path]::GetFullPath([string]$candidate.receiptPath)
            receiptSha256 = [string]$candidate.receiptSha256
            payloadInventorySha256 = [string]$receipt.payloadInventorySha256
            activeInventoryId = [string]$receipt.activeInventoryId
            identity = $receipt.identity
            sourceHashes = @($sourceRecords.ToArray())
            compilerProvenance = $script:compilerReadiness
            receiptCompilerProvenance = $receiptCompiler
            harnessPins = [ordered]@{
                installerUat = [ordered]@{ path = $uatSourcePath; sha256 = $uatSourceSha256; reviewedInputSha256 = 'aa71e3c761d919b3e08217a2883adc6b9c8a718af2d36506b6587a1b0005e11f' }
                installerUi = [ordered]@{ path = $uiSourcePath; sha256 = $uiSourceSha256; expectedSha256 = '4e5ac8db9d97212c073f8b8a7bc5efae5f34e0751347a347c81cdd0621547dd6' }
            }
            verification = [string]$receipt.verification
        }
        Write-UatJsonFile -Path $script:diagnosticCandidateEvidencePath -Value $script:diagnosticCandidateRecord
        $silentOwner = $script:runOwner['silentCandidate']
        $silentOwner['setupBytes'] = [long]$candidate.setupBytes
        $silentOwner['setupSha256'] = [string]$candidate.sha256
        $silentOwner['compilerExitCode'] = [int]$candidate.compilerExitCode
        $silentOwner['receiptPath'] = [System.IO.Path]::GetFullPath([string]$candidate.receiptPath)
        $silentOwner['receiptSha256'] = [string]$candidate.receiptSha256
        $silentOwner['provenancePath'] = [System.IO.Path]::GetFullPath($script:diagnosticCandidateEvidencePath)
        Write-UatJsonFile -Path $script:runOwnerPath -Value $script:runOwner
        return @($candidate.receiptPath, $script:diagnosticCandidateEvidencePath)
    }
    if ($script:selectedCases[0] -ceq 'A6' -or $script:selectedCases[0] -ceq 'A15') {
        $specifications = New-Object System.Collections.Generic.List[object]
        if ($script:selectedCases[0] -ceq 'A6') {
            [void]$specifications.Add([ordered]@{ key = 'portable'; name = 'stable-portable-root'; suffix = '-PORTABLE' })
            [void]$specifications.Add([ordered]@{ key = 'separate'; name = 'stable-separate-root'; suffix = '-SEPARATE' })
        }
        else {
            [void]$specifications.Add([ordered]@{ key = 'fresh'; name = 'stable-fresh-root'; suffix = '-FRESH' })
        }
        $candidateRecordsForProvenance = New-Object System.Collections.Generic.List[object]
        $candidateReceiptPaths = New-Object System.Collections.Generic.List[string]
        $candidateEvidencePaths = New-Object System.Collections.Generic.List[string]
        foreach ($specification in $specifications) {
            $candidateRootName = [string]$script:runOwner.rootName + [string]$specification.suffix
            $candidateRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent ([string]$script:runOwner.testRoot)) $candidateRootName))
            Assert-Uat (-not (Test-Path -LiteralPath $candidateRoot)) ('A diagnostic candidate install root already exists and remains untouched: {0}' -f $candidateRoot)
            $candidate = Build-UatInstaller -Name ([string]$specification.name) -RootName $candidateRootName
            $script:builds[[string]$specification.key] = $candidate
            $receipt = $candidate.receipt
            Assert-Uat ($receipt.schemaVersion -eq 1 -and [string]$receipt.artifactKind -ceq 'candidate-only') ('{0} candidate receipt schema or artifact kind changed.' -f $script:selectedCases[0])
            Assert-Uat ([long]$candidate.compilerExitCode -eq [long]0) ('{0} candidate compiler did not return exit code 0.' -f $script:selectedCases[0])
            $evidencePrefix = $providedEvidencePath.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
            Assert-Uat ($candidate.path.StartsWith($evidencePrefix, [System.StringComparison]::OrdinalIgnoreCase)) 'Diagnostic candidate EXE is outside the newly owned evidence directory.'
            $receiptCompiler = $receipt.compilerProvenance
            Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$receiptCompiler.path), $compilerPath, [System.StringComparison]::OrdinalIgnoreCase)) 'Candidate receipt compiler path did not match compiler.json.'
            Assert-Uat ([string]$receiptCompiler.version -ceq '6.7.4' -and [string]$receiptCompiler.installerSha256 -ceq [string]$compiler.sha256 -and [long]$receiptCompiler.installerBytes -eq [long]$compiler.bytes) 'Candidate receipt compiler installer metadata did not match compiler.json.'
            Assert-Uat ([string]$receiptCompiler.authenticode -ceq [string]$compiler.authenticode -and [string]$receiptCompiler.publisher -ceq [string]$compiler.publisher) 'Candidate receipt compiler signature metadata did not match compiler.json.'
            Assert-Uat ([string]$receiptCompiler.isccSha256 -ceq $compilerIsccSha256 -and [string]$receiptCompiler.isccSha256 -ceq 'f02fa0e5f237b47d7ed1edae9b991866ec94fe02af61eb6a73e94f92fe8e0604') 'Candidate receipt ISCC.exe hash did not match the installed compiler.'
            Assert-Uat ($null -ne $receipt.sourceHashes) 'Candidate receipt omitted sourceHashes.'
            $sourceHashProperties = @($receipt.sourceHashes.PSObject.Properties)
            Assert-Uat ($sourceHashProperties.Count -gt 0) 'Candidate receipt sourceHashes was empty.'
            $sourceRecords = New-Object System.Collections.Generic.List[object]
            $repoPrefix = $repoRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
            foreach ($sourceProperty in $sourceHashProperties) {
                $sourceName = [string]$sourceProperty.Name
                $expectedSourceHash = [string]$sourceProperty.Value
                if ([string]::Equals($sourceName, 'compiler/ISCC.exe', [System.StringComparison]::Ordinal)) {
                    Assert-Uat ([string]::Equals($expectedSourceHash, $compilerIsccSha256, [System.StringComparison]::OrdinalIgnoreCase)) 'Candidate sourceHashes compiler/ISCC.exe did not match the installed compiler.'
                    [void]$sourceRecords.Add([ordered]@{ name = $sourceName; path = $compilerPath; sha256 = $expectedSourceHash; comparison = 'installed-compiler' })
                    continue
                }
                if ($sourceName -match '^(generated|staged)/') {
                    [void]$sourceRecords.Add([ordered]@{ name = $sourceName; sha256 = $expectedSourceHash; comparison = 'not-mapped-to-repository' })
                    continue
                }
                Assert-Uat ($sourceName -match '^[A-Za-z0-9._/-]+$' -and $sourceName -notmatch '(^|/)[.][.]?(/|$)') ('Candidate receipt source key was not a safe repository-relative path: {0}' -f $sourceName)
                $sourcePath = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ($sourceName -replace '/', [System.IO.Path]::DirectorySeparatorChar)))
                Assert-Uat ($sourcePath.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)) ('Candidate receipt source escaped the repository: {0}' -f $sourceName)
                Assert-Uat (Test-Path -LiteralPath $sourcePath -PathType Leaf) ('Candidate receipt source file is missing: {0}' -f $sourceName)
                $actualSourceHash = Get-UatFileHash $sourcePath
                Assert-Uat ([string]::Equals($actualSourceHash, $expectedSourceHash, [System.StringComparison]::OrdinalIgnoreCase)) ('Candidate receipt source hash changed: {0}' -f $sourceName)
                [void]$sourceRecords.Add([ordered]@{ name = $sourceName; path = $sourcePath; sha256 = $actualSourceHash; comparison = 'current-repository-file' })
            }
            $uatSourcePath = [System.IO.Path]::GetFullPath($PSCommandPath)
            $uatSourceSha256 = Get-UatFileHash $uatSourcePath
            $uiSourcePath = [System.IO.Path]::GetFullPath($helperPath)
            $uiSourceSha256 = Get-UatFileHash $uiSourcePath
            Assert-Uat ($uiSourceSha256 -ceq '4e5ac8db9d97212c073f8b8a7bc5efae5f34e0751347a347c81cdd0621547dd6') 'The UI helper SHA-256 did not match its reviewed harness pin.'
            $candidateEvidencePath = Join-Path $providedEvidencePath ($script:selectedCases[0] + '-' + [string]$specification.key + '-candidate-provenance.json')
            $candidateRecord = [ordered]@{
                schemaVersion = 1; artifactKind = 'candidate-only'; caseId = [string]$script:selectedCases[0]
                name = [string]$candidate.name; rootName = [string]$candidate.rootName
                installRoot = [System.IO.Path]::GetFullPath([string]$candidate.installRoot)
                outputPath = [System.IO.Path]::GetFullPath([string]$candidate.path)
                setupBytes = [long]$candidate.setupBytes; setupSha256 = [string]$candidate.sha256
                compilerExitCode = [int]$candidate.compilerExitCode
                receiptPath = [System.IO.Path]::GetFullPath([string]$candidate.receiptPath)
                receiptSha256 = [string]$candidate.receiptSha256
                payloadInventorySha256 = [string]$receipt.payloadInventorySha256
                activeInventoryId = [string]$receipt.activeInventoryId
                identity = $receipt.identity
                sourceHashes = @($sourceRecords.ToArray())
                compilerProvenance = $script:compilerReadiness
                receiptCompilerProvenance = $receiptCompiler
                harnessPins = [ordered]@{
                    installerUat = [ordered]@{ path = $uatSourcePath; sha256 = $uatSourceSha256; inputSourceSha256 = '40c2524c0b0b3eae1a81c11143034c0047ec7f1f143a322584040e52b9ab6651' }
                    installerUi = [ordered]@{ path = $uiSourcePath; sha256 = $uiSourceSha256; expectedSha256 = '4e5ac8db9d97212c073f8b8a7bc5efae5f34e0751347a347c81cdd0621547dd6' }
                }
                a1Selected = $false; sourceBuildIdentityEquivalence = $null
                verification = [string]$receipt.verification
            }
            Write-UatJsonFile -Path $candidateEvidencePath -Value $candidateRecord
            $candidateOwner = $script:runOwner['candidateOwners'][[string]$specification.key]
            $candidateOwner['setupBytes'] = [long]$candidate.setupBytes
            $candidateOwner['setupSha256'] = [string]$candidate.sha256
            $candidateOwner['compilerExitCode'] = [int]$candidate.compilerExitCode
            $candidateOwner['receiptPath'] = [System.IO.Path]::GetFullPath([string]$candidate.receiptPath)
            $candidateOwner['receiptSha256'] = [string]$candidate.receiptSha256
            $candidateOwner['provenancePath'] = [System.IO.Path]::GetFullPath($candidateEvidencePath)
            $candidateOwner['provenanceSha256'] = Get-UatFileHash $candidateEvidencePath
            Write-UatJsonFile -Path $script:runOwnerPath -Value $script:runOwner
            $lastBuildRecord = $script:candidateRecords[$script:candidateRecords.Count - 1]
            $lastBuildRecord['provenancePath'] = [System.IO.Path]::GetFullPath($candidateEvidencePath)
            [void]$candidateRecordsForProvenance.Add($candidateRecord)
            [void]$candidateReceiptPaths.Add([string]$candidate.receiptPath)
            [void]$candidateEvidencePaths.Add([System.IO.Path]::GetFullPath($candidateEvidencePath))
        }
        $script:sourceBuildIdentityEquivalence = $null
        $script:diagnosticCandidateEvidencePath = Join-Path $providedEvidencePath ($script:selectedCases[0] + '-candidate-provenance.json')
        $script:diagnosticCandidateRecord = [ordered]@{
            schemaVersion = 1; artifactKind = 'candidate-only'; caseId = [string]$script:selectedCases[0]
            candidates = @($candidateRecordsForProvenance.ToArray())
            candidateEvidencePaths = @($candidateEvidencePaths.ToArray())
            sourceBuildIdentityEquivalence = $null; a1Selected = $false
        }
        Write-UatJsonFile -Path $script:diagnosticCandidateEvidencePath -Value $script:diagnosticCandidateRecord
        $script:runOwner['candidateProvenancePath'] = [System.IO.Path]::GetFullPath($script:diagnosticCandidateEvidencePath)
        $script:runOwner['candidateProvenanceSha256'] = Get-UatFileHash $script:diagnosticCandidateEvidencePath
        Write-UatJsonFile -Path $script:runOwnerPath -Value $script:runOwner
        [void]$candidateEvidencePaths.Add([System.IO.Path]::GetFullPath($script:diagnosticCandidateEvidencePath))
        foreach ($path in $candidateEvidencePaths) { [void]$script:candidateProvenancePaths.Add([string]$path) }
        return @($candidateReceiptPaths.ToArray()) + @($candidateEvidencePaths.ToArray())
    }
    $testRootPortable = $script:testRootName + '-PORTABLE'
    $testRootSeparate = $script:testRootName + '-SEPARATE'
    $testRootFresh = $script:testRootName + '-FRESH'
    $testRootSilent = $script:testRootName + '-SILENT'
    $script:builds['baseline'] = Build-UatInstaller -Name 'baseline-b158' -RootName $script:testRootName -Baseline
    $script:builds['stable'] = Build-UatInstaller -Name 'stable-current' -RootName $script:testRootName
    $script:builds['missing'] = Build-UatInstaller -Name 'stable-missing-runtime' -RootName $script:testRootName -MissingRuntime
    $script:builds['fault-backup'] = Build-UatInstaller -Name 'stable-fault-backup' -RootName $testRootFresh -Fault 'Backup'
    $script:builds['fault-partial'] = Build-UatInstaller -Name 'stable-fault-partial' -RootName $testRootFresh -Fault 'Partial'
    $script:builds['fault-late'] = Build-UatInstaller -Name 'stable-fault-late' -RootName $testRootFresh -Fault 'Late'
    $script:builds['portable'] = Build-UatInstaller -Name 'stable-portable-root' -RootName $testRootPortable
    $script:builds['separate'] = Build-UatInstaller -Name 'stable-separate-root' -RootName $testRootSeparate
    $script:builds['fresh'] = Build-UatInstaller -Name 'stable-fresh-root' -RootName $testRootFresh
    $script:builds['silent'] = Build-UatInstaller -Name 'stable-silent-root' -RootName $testRootSilent
    $stableHashes = @($script:builds['stable'].receipt.sourceHashes.PSObject.Properties |
        Where-Object { $_.Name -notmatch '^(dist/SDIR\.exe|docs/task-id/performance-optimize/baseline/SDIR-baseline\.exe|generated/|staged/)' } |
        Sort-Object Name | ForEach-Object { [ordered]@{ name = $_.Name; sha256 = [string]$_.Value } })
    $baselineHashes = @($script:builds['baseline'].receipt.sourceHashes.PSObject.Properties |
        Where-Object { $_.Name -notmatch '^(dist/SDIR\.exe|docs/task-id/performance-optimize/baseline/SDIR-baseline\.exe|generated/|staged/)' } |
        Sort-Object Name | ForEach-Object { [ordered]@{ name = $_.Name; sha256 = [string]$_.Value } })
    Assert-UatEqual $stableHashes $baselineHashes 'Baseline and stable test candidates do not share identical non-executable sources.'
    $script:sourceBuildIdentityEquivalence = [ordered]@{
        stableSourceAppSha256 = $script:stableAppExpected
        testStableCandidateEmbeddedAppSha256 = $script:builds['stable'].receipt.embeddedAppSha256
        testBaselineCandidateEmbeddedAppSha256 = $script:builds['baseline'].receipt.embeddedAppSha256
        productionAppId = $script:productionAppId
        testAppId = $script:testAppId
        productionRootName = 'SDIR'
        testRoots = @($script:testRootName, $testRootPortable, $testRootSeparate, $testRootFresh, $testRootSilent)
        productionBuildPerformed = $false
        productionInstallAttempted = $false
        stablePayloadEntries = @($manifest.files.Count)
        sameStableNonExecutableBuildSources = $true
        testBaselineAllowedAsTestPayloadOnly = $true
    }
    Assert-Uat ($script:builds['stable'].receipt.identity.appId -ne $script:productionAppId) 'Test identity aliases production identity.'
    Assert-Uat ($script:builds['stable'].receipt.identity.rootName -ne 'SDIR') 'Test root aliases production root.'
    return @($script:builds.Values | ForEach-Object { $_.receiptPath })
}

function Get-UatProcessIdentity {
    param([Parameter(Mandatory = $true)][int]$ProcessId)
    $process = [System.Diagnostics.Process]::GetProcessById($ProcessId)
    try {
        $started = $process.StartTime.ToUniversalTime()
        $imagePath = [Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath($ProcessId)
        return [pscustomobject]@{ processId = $ProcessId; startUtc = $started; startTicks = $started.Ticks; imagePath = [System.IO.Path]::GetFullPath($imagePath) }
    }
    finally { $process.Dispose() }
}

function Assert-UatProcessIdentity {
    param(
        [Parameter(Mandatory = $true)][int]$ProcessId,
        [Parameter(Mandatory = $true)][long]$ExpectedStartTicks,
        [Parameter(Mandatory = $true)][string]$ExpectedImagePath
    )
    $identity = Get-UatProcessIdentity -ProcessId $ProcessId
    Assert-Uat ($identity.startTicks -eq $ExpectedStartTicks) ('PID {0} start time changed or was reused.' -f $ProcessId)
    Assert-Uat ([string]::Equals($identity.imagePath, [System.IO.Path]::GetFullPath($ExpectedImagePath), [System.StringComparison]::OrdinalIgnoreCase)) ('PID {0} executable path changed.' -f $ProcessId)
    if ([Sdir.Installer.Uat.NativeWindowApi]::IsProcessElevated($ProcessId)) {
        throw ('Owned test process unexpectedly has an elevated token: PID {0}' -f $ProcessId)
    }
    return $identity
}

function Get-UatProcessCommandLine {
    param([Parameter(Mandatory = $true)][int]$ProcessId)
    $process = Get-CimInstance -ClassName Win32_Process -Filter ('ProcessId = {0}' -f $ProcessId) -ErrorAction Stop
    if ($null -eq $process -or [int]$process.ProcessId -ne $ProcessId) {
        throw ('Could not verify Win32_Process command line for PID {0}.' -f $ProcessId)
    }
    return [string]$process.CommandLine
}

function Assert-UatNoExistingTestLifecycle {
    Assert-Uat (-not $script:preflightCalled) 'The TEST lifecycle preflight may run only once.'
    $script:preflightCalled = $true
    $scriptPath = [System.IO.Path]::GetFullPath($PSCommandPath)
    $expectedPowerShellPath = [System.IO.Path]::GetFullPath((Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'))
    $currentIdentity = Get-UatProcessIdentity -ProcessId $PID
    Assert-Uat ([string]::Equals($currentIdentity.imagePath, $expectedPowerShellPath, [System.StringComparison]::OrdinalIgnoreCase)) 'The current host is not the verified absolute Windows PowerShell 5 executable.'
    $currentCommandLine = Get-UatProcessCommandLine -ProcessId $PID
    Assert-Uat ($currentCommandLine.IndexOf($scriptPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) 'The current process command line did not identify the actual installer-uat.ps1 path.'
    $processRows = @(Get-CimInstance -ClassName Win32_Process -ErrorAction Stop)
    $programsPath = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs')).TrimEnd('\') + '\'
    $testInstallPrefix = $programsPath + 'SDIR-UAT-'
    $staticPrefix = $staticEvidenceRoot.TrimEnd('\') + '\'
    $builderScriptPath = [System.IO.Path]::GetFullPath($script:builderPath)
    $related = New-Object System.Collections.Generic.List[object]
    foreach ($processRow in $processRows) {
        $processId = [int]$processRow.ProcessId
        if ($processId -eq $PID) { continue }
        $cimImagePath = [string]$processRow.ExecutablePath
        $cimCommandLine = [string]$processRow.CommandLine
        $imagePathCandidate = $false
        if (-not [string]::IsNullOrWhiteSpace($cimImagePath)) {
            $imagePathCandidate = $cimImagePath.StartsWith($testInstallPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
                $cimImagePath.StartsWith($staticPrefix, [System.StringComparison]::OrdinalIgnoreCase)
        }
        $commandHasUatScript = $cimCommandLine.IndexOf($scriptPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
        $commandHasProfile = $cimCommandLine.IndexOf('SDIR-UAT-Profile-', [System.StringComparison]::OrdinalIgnoreCase) -ge 0
        $commandHasTestRoot = $cimCommandLine.IndexOf('SDIR-UAT-', [System.StringComparison]::OrdinalIgnoreCase) -ge 0
        $commandHasStaticRoot = $cimCommandLine.IndexOf($staticEvidenceRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
        $commandHasTestBuilder = $cimCommandLine.IndexOf($builderScriptPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
            $cimCommandLine.IndexOf('-TestBuild', [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
            ($commandHasTestRoot -or $commandHasProfile)
        $commandPathCandidate = $commandHasUatScript -or $commandHasProfile -or $commandHasTestBuilder -or
            ($commandHasStaticRoot -and $commandHasTestRoot)
        if (-not $imagePathCandidate -and -not $commandPathCandidate) { continue }
        $identity = Get-UatProcessIdentity -ProcessId $processId
        if (-not [string]::IsNullOrWhiteSpace($cimImagePath)) {
            $normalizedCimImagePath = [System.IO.Path]::GetFullPath($cimImagePath)
            Assert-Uat ([string]::Equals($identity.imagePath, $normalizedCimImagePath, [System.StringComparison]::OrdinalIgnoreCase)) ('Related TEST process image path did not match its native identity: PID {0}.' -f $processId)
        }
        if ($commandPathCandidate) {
            $confirmedCommandLine = Get-UatProcessCommandLine -ProcessId $processId
            $confirmedHasUatScript = $confirmedCommandLine.IndexOf($scriptPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
            $confirmedHasProfile = $confirmedCommandLine.IndexOf('SDIR-UAT-Profile-', [System.StringComparison]::OrdinalIgnoreCase) -ge 0
            $confirmedHasTestRoot = $confirmedCommandLine.IndexOf('SDIR-UAT-', [System.StringComparison]::OrdinalIgnoreCase) -ge 0
            $confirmedHasStaticRoot = $confirmedCommandLine.IndexOf($staticEvidenceRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
            $confirmedHasTestBuilder = $confirmedCommandLine.IndexOf($builderScriptPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
                $confirmedCommandLine.IndexOf('-TestBuild', [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
                ($confirmedHasTestRoot -or $confirmedHasProfile)
            Assert-Uat ($confirmedHasUatScript -or $confirmedHasProfile -or $confirmedHasTestBuilder -or ($confirmedHasStaticRoot -and $confirmedHasTestRoot)) ('Related TEST process command line changed during preflight: PID {0}.' -f $processId)
        }
        $createdUtc = ConvertTo-UatCimCreationUtc -Value $processRow.CreationDate -ProcessId $processId
        Assert-Uat ([Math]::Abs($createdUtc.Ticks - $identity.startTicks) -lt [TimeSpan]::TicksPerMillisecond) ('Related TEST process CIM/native start times did not correlate: PID {0}.' -f $processId)
        $descendants = @(Get-UatProcessDescendantIds -RootProcessId $processId -RootStartTicks $identity.startTicks -IncludeRows)
        $descendantIds = @($descendants | Where-Object { [int]$_.pid -ne $processId } | ForEach-Object { [int]$_.pid })
        [void]$related.Add([pscustomobject]@{
            processId = $processId
            startUtc = $identity.startUtc
            startTicks = [long]$identity.startTicks
            imagePath = $identity.imagePath
            descendantProcessIds = $descendantIds
        })
    }
    if ($related.Count -gt 0) {
        $relatedIds = @($related | ForEach-Object { [string]$_.processId }) -join ', '
        throw ('A known TEST lifecycle process is already active; no process was changed. Related PIDs: {0}' -f $relatedIds)
    }
    $script:preflightResult = [ordered]@{
        checkedUtc = [DateTime]::UtcNow.ToString('o')
        currentProcessId = [int]$PID
        currentPowerShellPath = $currentIdentity.imagePath
        processSource = 'Win32_Process plus native PID/path/start-time identity'
        knownTestProcesses = 0
        relatedNativeChildren = 0
        mutexAcquired = [bool]$script:mutexAcquired
    }
}

function Find-UatAppProcess {
    param(
        [Parameter(Mandatory = $true)][string]$ImagePath,
        [Parameter(Mandatory = $true)][DateTime]$NotBeforeUtc,
        [string]$CommandPattern
    )
    $matches = New-Object System.Collections.Generic.List[object]
    foreach ($process in @(Get-Process -Name 'SDIR' -ErrorAction SilentlyContinue)) {
        try {
            $identity = Get-UatProcessIdentity -ProcessId $process.Id
            if (-not [string]::Equals($identity.imagePath, [System.IO.Path]::GetFullPath($ImagePath), [System.StringComparison]::OrdinalIgnoreCase)) { continue }
            if ($identity.startUtc -lt $NotBeforeUtc) { continue }
            $commandLine = Get-UatProcessCommandLine -ProcessId $process.Id
            if ($CommandPattern -and $commandLine -notmatch $CommandPattern) { continue }
            [void]$matches.Add([pscustomobject]@{
                processId = [int]$identity.processId
                startUtc = $identity.startUtc
                startTicks = [long]$identity.startTicks
                imagePath = $identity.imagePath
                commandLine = $commandLine
            })
        }
        catch { }
        finally { $process.Dispose() }
    }
    return @($matches.ToArray())
}

function Wait-UatAppProcess {
    param(
        [Parameter(Mandatory = $true)][string]$ImagePath,
        [Parameter(Mandatory = $true)][DateTime]$NotBeforeUtc,
        [string]$CommandPattern,
        [int]$TimeoutSeconds = 30
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $matches = @(Find-UatAppProcess -ImagePath $ImagePath -NotBeforeUtc $NotBeforeUtc -CommandPattern $CommandPattern)
        if ($matches.Count -eq 1) { return $matches[0] }
        if ($matches.Count -gt 1) { throw ('More than one new harness app process matches {0}; no window was selected.' -f $ImagePath) }
        Start-Sleep -Milliseconds 200
    }
    throw ('No new app process at the exact test image path appeared: {0}' -f $ImagePath)
}

function ConvertTo-UatCimCreationUtc {
    param([Parameter(Mandatory = $true)]$Value, [Parameter(Mandatory = $true)][int]$ProcessId)
    if ($Value -is [DateTime]) { return $Value.ToUniversalTime() }
    if ($Value -is [string] -and $Value -match '^[0-9]{14}\.[0-9]{6}[+-][0-9]{3}$') {
        try { return [System.Management.ManagementDateTimeConverter]::ToDateTime($Value).ToUniversalTime() }
        catch { throw ('Invalid CIM DMTF creation time for PID {0}: {1}' -f $ProcessId, $_.Exception.Message) }
    }
    throw ('Unsupported CIM creation time for PID {0}; expected DateTime or a complete DMTF string.' -f $ProcessId)
}

function Get-UatProcessDescendantIds {
    param(
        [Parameter(Mandatory = $true)][int]$RootProcessId,
        [Parameter(Mandatory = $true)][long]$RootStartTicks,
        [switch]$IncludeRows,
        [object]$RetainedRoot = $null,
        [object[]]$RetainedLineage = @(),
        [switch]$AllowNormalRootExit,
        [switch]$ScanExitedRetainedEngine,
        [ref]$SnapshotByPid
    )
    $root = $null
    $rootExitedBeforeSnapshot = $false
    $ownsRoot = $false
    $rootImagePath = $null
    $rootCimExecutablePath = $null
    try {
        if ($null -ne $RetainedRoot) {
            foreach ($propertyName in @('process', 'processId', 'pinnedHandle', 'startTicks', 'imagePath', 'parentProcessId', 'commandLine')) {
                if ($null -eq $RetainedRoot.PSObject.Properties[$propertyName]) {
                    throw ('Retained native root is missing its {0} identity field.' -f $propertyName)
                }
            }
            $root = $RetainedRoot.process
            $pinnedHandle = [IntPtr]$RetainedRoot.pinnedHandle
            if ($null -eq $root -or [int]$RetainedRoot.processId -ne $RootProcessId -or [int]$root.Id -ne $RootProcessId -or
                $pinnedHandle -eq [IntPtr]::Zero -or $root.Handle -eq [IntPtr]::Zero -or $root.Handle -ne $pinnedHandle) {
                throw ('Retained native root PID {0} no longer has its exact pinned process handle.' -f $RootProcessId)
            }
            if ([long]$RetainedRoot.startTicks -ne $RootStartTicks -or [long]$root.StartTime.ToUniversalTime().Ticks -ne $RootStartTicks) {
                throw ('Retained native root PID {0} no longer has its exact native start time.' -f $RootProcessId)
            }
            $rootImagePath = [string]$RetainedRoot.imagePath
            if ([string]::IsNullOrWhiteSpace($rootImagePath)) {
                throw ('Retained native root PID {0} has no admitted image proof.' -f $RootProcessId)
            }
            $rootImagePath = [System.IO.Path]::GetFullPath($rootImagePath)
            if (-not [string]::Equals($rootImagePath, [string]$RetainedRoot.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw ('Retained native root PID {0} has an invalid admitted image path.' -f $RootProcessId)
            }
            if ($root.HasExited) {
                if (-not $AllowNormalRootExit) {
                    throw ('Retained native root PID {0} exited before strict discovery.' -f $RootProcessId)
                }
                if (-not $ScanExitedRetainedEngine) { return @() }
                $rootExitedBeforeSnapshot = $true
            }
        }
        else {
            if ($AllowNormalRootExit) { throw 'Normal root exit requires a retained native process record.' }
            $root = [System.Diagnostics.Process]::GetProcessById($RootProcessId)
            $ownsRoot = $true
            $pinnedHandle = $root.Handle
            Assert-Uat ($pinnedHandle -ne [IntPtr]::Zero -and -not $root.HasExited) ('Root PID {0} is not live.' -f $RootProcessId)
            Assert-Uat ($root.StartTime.ToUniversalTime().Ticks -eq $RootStartTicks) ('Root PID {0} was reused since its native start time.' -f $RootProcessId)
        }
        $processes = @(Get-CimInstance -ClassName Win32_Process -ErrorAction Stop)
        $byPid = @{}
        foreach ($process in $processes) { $byPid[[int]$process.ProcessId] = $process }
        if ($null -ne $SnapshotByPid) { $SnapshotByPid.Value = $byPid }
        $rootRowMissing = $false
        if (-not $byPid.ContainsKey($RootProcessId)) {
            if ($null -eq $RetainedRoot -or -not $AllowNormalRootExit) {
                throw ('Root PID {0} disappeared from the process table.' -f $RootProcessId)
            }
            $rootRowMissing = $true
            $rootCreatedTicks = $RootStartTicks
            $rootParentId = [int]$RetainedRoot.parentProcessId
            $rootCommandLine = [string]$RetainedRoot.commandLine
        }
        else {
            $rootRow = $byPid[$RootProcessId]
            $rootCimExecutablePath = $rootRow.ExecutablePath
            $rootCreated = ConvertTo-UatCimCreationUtc -Value $rootRow.CreationDate -ProcessId $RootProcessId
            $rootCreatedTicks = [long]$rootCreated.Ticks
            # CIM time is a correlation check; exact native identity comes from the retained handle.
            Assert-Uat ([Math]::Abs($rootCreatedTicks - $RootStartTicks) -lt [TimeSpan]::TicksPerMillisecond) ('Root PID {0} CIM/native creation times do not correlate.' -f $RootProcessId)
            $rootParentId = [int]$rootRow.ParentProcessId
            $rootCommandLine = [string]$rootRow.CommandLine
            if ($null -ne $RetainedRoot -and -not [string]::IsNullOrWhiteSpace([string]$rootRow.ExecutablePath)) {
                $rowRootImagePath = [System.IO.Path]::GetFullPath([string]$rootRow.ExecutablePath)
                if (-not [string]::Equals($rowRootImagePath, $rootImagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
                    throw ('CIM/native root image paths differ for PID {0}.' -f $RootProcessId)
                }
            }
            if ($null -eq $RetainedRoot) { $rootImagePath = [string]$rootRow.ExecutablePath }
        }
        $retainedStartTicksByPid = @{}
        foreach ($lineage in @($RetainedLineage)) {
            if ($null -eq $lineage -or $null -eq $lineage.PSObject.Properties['processId'] -or
                $null -eq $lineage.PSObject.Properties['startTicks']) {
                throw 'Validated retained native lineage contains an incomplete process identity.'
            }
            $lineagePid = [int]$lineage.processId
            $lineageStartTicks = [long]$lineage.startTicks
            if ($lineagePid -le 0 -or $lineageStartTicks -le 0) {
                throw 'Validated retained native lineage contains an invalid PID or start time.'
            }
            if ($retainedStartTicksByPid.ContainsKey($lineagePid) -and
                [long]$retainedStartTicksByPid[$lineagePid] -ne $lineageStartTicks) {
                throw ('Retained native lineage has conflicting start times for PID {0}.' -f $lineagePid)
            }
            $retainedStartTicksByPid[$lineagePid] = $lineageStartTicks
        }
        if ($retainedStartTicksByPid.ContainsKey($RootProcessId) -and
            [long]$retainedStartTicksByPid[$RootProcessId] -ne $RootStartTicks) {
            throw ('Retained native root PID {0} conflicts with its historical lineage start time.' -f $RootProcessId)
        }
        $retainedStartTicksByPid[$RootProcessId] = $RootStartTicks
        $ids = New-Object System.Collections.Generic.HashSet[int]
        [void]$ids.Add($RootProcessId)
        $createdByPid = @{}
        $createdByPid[$RootProcessId] = [pscustomobject]@{
            pid = $RootProcessId; parent = $rootParentId; cimStartTicks = [long]$rootCreatedTicks
            image = [string]$rootImagePath; commandLine = [string]$rootCommandLine
            cimExecutablePath = $rootCimExecutablePath
        }
        $changed = $true
        while ($changed) {
            $changed = $false
            foreach ($process in $processes) {
                $processIdValue = [int]$process.ProcessId
                $parentIdValue = [int]$process.ParentProcessId
                if ($createdByPid.ContainsKey($processIdValue)) { continue }
                if ($createdByPid.ContainsKey($parentIdValue)) {
                    $parentStartTicks = [long]$createdByPid[$parentIdValue].cimStartTicks
                }
                elseif ($retainedStartTicksByPid.ContainsKey($parentIdValue)) {
                    # Historical identity exposes a real row; it never proves that its process is live.
                    $parentStartTicks = [long]$retainedStartTicksByPid[$parentIdValue]
                }
                else { continue }
                # Decode only rows reachable through a retained identity or an observed row.
                $created = ConvertTo-UatCimCreationUtc -Value $process.CreationDate -ProcessId $processIdValue
                if ($created.Ticks -lt ($parentStartTicks - [TimeSpan]::TicksPerMillisecond)) { continue }
                if ($rootExitedBeforeSnapshot -and -not $retainedStartTicksByPid.ContainsKey($processIdValue)) {
                    throw ('Exited retained native root PID {0} has an unadmitted descendant PID {1} in its completion snapshot.' -f $RootProcessId, $processIdValue)
                }
                [void]$ids.Add($processIdValue)
                $createdByPid[$processIdValue] = [pscustomobject]@{
                    pid = $processIdValue; parent = $parentIdValue; cimStartTicks = [long]$created.Ticks
                    image = [string]$process.ExecutablePath; commandLine = [string]$process.CommandLine
                    cimExecutablePath = $process.ExecutablePath
                }
                $changed = $true
            }
        }
        if (-not $AllowNormalRootExit -and $root.HasExited) {
            throw ('Root PID {0} exited during descendant discovery; retain evidence and do not adopt a guessed tree.' -f $RootProcessId)
        }
        if ($IncludeRows) {
            $rows = @($createdByPid.Values | Sort-Object cimStartTicks, pid)
            if ($rootRowMissing) { $rows = @($rows | Where-Object { [int]$_.pid -ne $RootProcessId }) }
            return $rows
        }
        if ($rootRowMissing) { return @($ids | Where-Object { [int]$_ -ne $RootProcessId } | ForEach-Object { [int]$_ }) }
        return @($ids | ForEach-Object { [int]$_ })
    }
    finally {
        if ($ownsRoot -and $null -ne $root) { $root.Dispose() }
    }
}

function Get-UatCompletionRemainingMilliseconds {
    param([Parameter(Mandatory = $true)]$CompletionBudget)
    if ($null -eq $CompletionBudget.stopwatch -or $null -eq $CompletionBudget.totalMilliseconds) {
        throw 'CompletionBudget is missing its original stopwatch or total duration.'
    }
    $remaining = [Math]::Floor([double]$CompletionBudget.totalMilliseconds - $CompletionBudget.stopwatch.Elapsed.TotalMilliseconds)
    return [int][Math]::Max(0, [Math]::Min([double][int]::MaxValue, $remaining))
}

function New-UatCompletionBudget {
    param(
        [Parameter(Mandatory = $true)][long]$TotalMilliseconds,
        [Parameter(Mandatory = $true)][string]$Origin
    )
    return [pscustomobject]@{
        stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
        totalMilliseconds = [long]$TotalMilliseconds
        origin = $Origin
    }
}

function New-UatSetupLaunchIntent {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)]$Candidate,
        [Parameter(Mandatory = $true)][string]$ExpectedCmdExePath,
        [Parameter(Mandatory = $true)]$CompletionBudget
    )
    if ($Entry.finished) { throw ('Cannot create a setup launch intent for finished command {0}.' -f $Entry.commandId) }
    if ($null -eq $Entry.PSObject.Properties['launchIntent'] -or $null -ne $Entry.launchIntent) {
        throw ('Setup command already has a launch intent or lacks its declared intent field: {0}.' -f $Entry.commandId)
    }
    if ($null -eq $Entry.completionBudget -or -not [object]::ReferenceEquals($Entry.completionBudget, $CompletionBudget)) {
        throw ('Setup launch intent did not receive the entry original CompletionBudget: {0}.' -f $Entry.commandId)
    }
    if ($null -eq $Candidate.PSObject.Properties['installRoot'] -or [string]::IsNullOrWhiteSpace([string]$Candidate.installRoot)) {
        throw 'TEST launch candidate has no explicit installRoot.'
    }
    if ($null -eq $Candidate.PSObject.Properties['receipt'] -or $null -eq $Candidate.receipt) {
        throw 'TEST launch candidate has no verified receipt.'
    }
    $receipt = $Candidate.receipt
    if (-not [bool]$receipt.testBuild) { throw 'Setup app handoff requires a TEST_BUILD candidate receipt.' }
    if (-not [string]::Equals([string]$receipt.testLauncher, 'sdir-installer-test-launcher.cmd', [System.StringComparison]::Ordinal)) {
        throw 'TEST receipt does not name the fixed isolated wrapper.'
    }
    if (-not [System.IO.Path]::IsPathRooted([string]$Candidate.installRoot)) { throw 'TEST installRoot must be explicitly absolute before normalization.' }
    $installRoot = [System.IO.Path]::GetFullPath([string]$Candidate.installRoot)
    if (-not [System.IO.Path]::IsPathRooted($installRoot) -or -not (Test-Path -LiteralPath $installRoot -PathType Container)) {
        throw ('TEST installRoot is not an existing absolute directory: {0}' -f $installRoot)
    }
    $launcherName = 'sdir-installer-test-launcher.cmd'
    $launcherRows = @($receipt.payloadInventory | Where-Object {
        [string]::Equals([string]$_.destination, $launcherName, [System.StringComparison]::Ordinal)
    })
    if ($launcherRows.Count -ne 1) { throw 'TEST receipt must contain one unique wrapper inventory row.' }
    $appRows = @($receipt.payloadInventory | Where-Object {
        [string]::Equals([string]$_.destination, 'SDIR.exe', [System.StringComparison]::Ordinal)
    })
    if ($appRows.Count -ne 1) { throw 'TEST receipt must contain one unique SDIR.exe inventory row.' }
    $launcherPath = [System.IO.Path]::GetFullPath((Join-Path $installRoot $launcherName))
    $appPath = [System.IO.Path]::GetFullPath((Join-Path $installRoot 'SDIR.exe'))
    if (-not (Test-Path -LiteralPath $launcherPath -PathType Leaf) -or -not (Test-Path -LiteralPath $appPath -PathType Leaf)) {
        throw 'Installed TEST wrapper or SDIR.exe is missing at the explicit installRoot.'
    }
    $launcherBytes = [long]$launcherRows[0].bytes
    $launcherSha256 = [string]$launcherRows[0].sha256
    if ($launcherBytes -le 0 -or $launcherSha256 -notmatch '^[0-9a-f]{64}$') { throw 'TEST wrapper inventory bytes or SHA-256 is invalid.' }
    $launcherFile = Get-Item -LiteralPath $launcherPath -ErrorAction Stop
    $installedLauncherSha256 = Get-UatFileHash $launcherPath
    if ([long]$launcherFile.Length -ne $launcherBytes -or
        -not [string]::Equals($installedLauncherSha256, $launcherSha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Installed TEST wrapper bytes or SHA-256 differs from its unique inventory row.'
    }
    $appBytes = [long]$appRows[0].bytes
    $inventoryAppSha256 = [string]$appRows[0].sha256
    $embeddedAppSha256 = [string]$receipt.embeddedAppSha256
    if ($appBytes -le 0 -or $inventoryAppSha256 -notmatch '^[0-9a-f]{64}$' -or $embeddedAppSha256 -notmatch '^[0-9a-f]{64}$') {
        throw 'TEST SDIR.exe inventory or embedded application SHA-256 is invalid.'
    }
    if (-not [string]::Equals($inventoryAppSha256, $embeddedAppSha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'TEST SDIR.exe inventory SHA-256 differs from the embedded application SHA-256.'
    }
    $appFile = Get-Item -LiteralPath $appPath -ErrorAction Stop
    $appSha256 = Get-UatFileHash $appPath
    if ([long]$appFile.Length -ne $appBytes -or
        -not [string]::Equals($appSha256, $embeddedAppSha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Installed TEST SDIR.exe bytes or SHA-256 differs from its receipt and embedded application hash.'
    }
    $systemRoot = [System.IO.Path]::GetFullPath([Environment]::GetEnvironmentVariable('SystemRoot'))
    $systemCmdPath = [System.IO.Path]::GetFullPath((Join-Path (Join-Path $systemRoot 'System32') 'cmd.exe'))
    if (-not [System.IO.Path]::IsPathRooted($ExpectedCmdExePath)) { throw 'Expected cmd.exe path must be explicitly absolute before normalization.' }
    $cmdPath = [System.IO.Path]::GetFullPath($ExpectedCmdExePath)
    if (-not [System.IO.Path]::IsPathRooted($cmdPath) -or
        -not [string]::Equals($cmdPath, $systemCmdPath, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not (Test-Path -LiteralPath $cmdPath -PathType Leaf)) {
        throw 'Expected cmd.exe is not the absolute SystemRoot System32 image for this Windows installation.'
    }
    $cmdSha256 = Get-UatFileHash $cmdPath
    if ($cmdSha256 -notmatch '^[0-9a-f]{64}$') { throw 'SystemRoot cmd.exe SHA-256 is invalid.' }
    $quote = [string][char]34
    $templates = @('"{C}" /C ""{W}""', '{C} /C ""{W}""')
    $expectedLines = @(
        ($quote + $cmdPath + $quote + ' /C ' + $quote + $quote + $launcherPath + $quote + $quote),
        ($cmdPath + ' /C ' + $quote + $quote + $launcherPath + $quote + $quote)
    )
    if ($expectedLines.Count -ne 2 -or [string]::Equals($expectedLines[0], $expectedLines[1], [System.StringComparison]::Ordinal)) {
        throw 'Fixed wrapper command-line policy did not produce two distinct finite expected strings.'
    }
    $policyId = 'inno-test-launcher-cmd-cmdline-v1'
    $intent = [pscustomobject]@{
        intentId = [Guid]::NewGuid().ToString('N'); setupCommandId = [string]$Entry.commandId
        candidateName = [string]$Candidate.name; installRoot = $installRoot
        launcherPath = $launcherPath; launcherSha256 = $installedLauncherSha256
        appPath = $appPath; appSha256 = $appSha256
        expectedCmdExePath = $cmdPath; expectedCmdExeSha256 = $cmdSha256
        expectedWrapperCommandLines = @($expectedLines); commandLinePolicyId = $policyId
        expectedWrapperCommandLineTemplates = @($templates)
        registeredUtc = [DateTime]::UtcNow.ToString('o'); clickNotBeforeUtc = $null
        nativeCaptureBeforeClickUtc = $null; nativeCaptureAfterClickUtc = $null
        completionBudget = $CompletionBudget; state = 'pending'; verifiedHandoff = $null
        registeredApp = $null; diagnostic = $null
    }
    $Entry.launchIntent = $intent
    Add-UatEvent -Kind 'setup-launch-intent-created' -Name $Entry.name -Data @{
        commandId = $Entry.commandId; intentId = $intent.intentId; candidateName = $intent.candidateName
        installRoot = $installRoot; launcherPath = $launcherPath; launcherSha256 = $installedLauncherSha256
        appPath = $appPath; appSha256 = $appSha256; expectedCmdExePath = $cmdPath; expectedCmdExeSha256 = $cmdSha256
        commandLinePolicyId = $policyId; expectedWrapperCommandLines = @($expectedLines)
        expectedWrapperCommandLineTemplates = @($templates)
        registeredUtc = $intent.registeredUtc
    }
    return $intent
}

function Get-UatNativeProcessObservation {
    param(
        [Parameter(Mandatory = $true)]$Record,
        [Parameter(Mandatory = $true)][string]$ExpectedImagePath
    )
    if ($null -eq $Record.process) { throw ('Native process record has no retained Process object for PID {0}.' -f $Record.processId) }
    $process = $Record.process
    $safeHandle = $process.SafeHandle
    if ($null -eq $safeHandle -or $safeHandle.IsInvalid -or $safeHandle.IsClosed) {
        throw ('Native process record has no valid retained SafeProcessHandle for PID {0}.' -f $Record.processId)
    }
    $safeHandleProperty = $Record.PSObject.Properties['safeHandle']
    if ($null -eq $safeHandleProperty) {
        $Record | Add-Member -MemberType NoteProperty -Name safeHandle -Value $safeHandle
    }
    elseif ($null -eq $Record.safeHandle -or -not [object]::ReferenceEquals($Record.safeHandle, $safeHandle)) {
        throw ('Native process record SafeProcessHandle changed for PID {0}.' -f $Record.processId)
    }
    $handle = [IntPtr]$safeHandle.DangerousGetHandle()
    if ($handle -eq [IntPtr]::Zero -or $handle -ne [IntPtr]$Record.pinnedHandle) {
        throw ('Native process record handle changed for PID {0}.' -f $Record.processId)
    }
    $observation = [Sdir.Installer.Uat.NativeWindowApi]::ObserveProcessIdentity($safeHandle, $ExpectedImagePath)
    if ([uint32]$observation.ProcessId -ne [uint32]$Record.processId -or
        [long]$observation.CreationUtcTicks -ne [long]$Record.startTicks -or
        -not [string]::Equals([string]$observation.ImagePath, [string]$ExpectedImagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw ('Native retained-handle identity differs from its pinned record for PID {0}.' -f $Record.processId)
    }
    return $observation
}

function Update-UatNativeEngineChildren {
    param([Parameter(Mandatory = $true)]$Entry)
    if (-not $Entry.nativeTracking) { return }
    if ($Entry.finished) { return }
    if ($null -ne $Entry.nativeObservationError) { throw ('Native process observation already failed for command {0}: {1}' -f $Entry.commandId, $Entry.nativeObservationError) }
    $roots = New-Object System.Collections.Generic.List[object]
    $observedRows = New-Object System.Collections.Generic.List[object]
    try {
        if ($null -eq $Entry.nativeLauncher -or $null -eq $Entry.nativeLauncher.process) {
            throw ('Native launcher identity was not pinned for command {0}.' -f $Entry.commandId)
        }
        if ($null -eq $Entry.nativeEngineChildren) { throw 'Native engine identity collection is unavailable.' }
        $knownRecords = @($Entry.nativeEngineChildren.ToArray())
        $untransferredEngines = @($knownRecords | Where-Object { -not $_.ownershipRoute })
        $hasRetainedEngine = $untransferredEngines.Count -gt 0
        $launcher = $Entry.nativeLauncher
        $launcherProcess = $launcher.process
        $launcherImage = [string]$launcher.imagePath
        $launcherObservation = Get-UatNativeProcessObservation -Record $launcher -ExpectedImagePath $launcherImage
        $launcherHandle = [IntPtr]$launcher.safeHandle.DangerousGetHandle()
        $launcherTicks = [long]$launcherObservation.CreationUtcTicks
        if ($launcherHandle -eq [IntPtr]::Zero -or [int]$launcherObservation.ProcessId -ne [int]$launcher.processId -or
            $launcherTicks -ne [long]$launcher.startTicks -or [string]::IsNullOrWhiteSpace($launcherImage) -or
            -not [string]::Equals([System.IO.Path]::GetFullPath($launcherImage), $launcherImage, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw ('Retained native launcher identity changed for PID {0}.' -f $launcher.processId)
        }
        $validatedLineageByPid = @{}
        $validatedLineageByPid[[int]$launcher.processId] = $launcher
        foreach ($known in $untransferredEngines) {
            if ($null -eq $known.process) { throw ('Retained native child PID {0} lost its process handle.' -f $known.processId) }
            $knownImage = [string]$known.imagePath
            $knownObservationFirst = Get-UatNativeProcessObservation -Record $known -ExpectedImagePath $knownImage
            $knownObservationSecond = Get-UatNativeProcessObservation -Record $known -ExpectedImagePath $knownImage
            $knownPid = [int]$known.processId
            $knownParentPid = [int]$known.parentProcessId
            $knownTicks = [long]$knownObservationFirst.CreationUtcTicks
            if ([int]$knownObservationFirst.ProcessId -ne $knownPid -or
                [long]$knownObservationSecond.CreationUtcTicks -ne $knownTicks -or
                [int]$knownObservationSecond.ProcessId -ne $knownPid -or
                -not [string]::Equals([string]$knownObservationFirst.ImagePath, [string]$knownImage, [System.StringComparison]::OrdinalIgnoreCase) -or
                -not [string]::Equals([string]$knownObservationSecond.ImagePath, [string]$knownImage, [System.StringComparison]::OrdinalIgnoreCase) -or
                [long]$known.startTicks -ne $knownTicks -or $knownTicks -lt [long]$known.parentStartTicks -or
                [int]$known.launcherProcessId -ne [int]$launcher.processId -or
                [long]$known.launcherStartTicks -ne [long]$launcher.startTicks -or
                [string]::IsNullOrWhiteSpace($knownImage) -or
                -not [string]::Equals([System.IO.Path]::GetFullPath($knownImage), $knownImage, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw ('Retained native child lineage changed for PID {0}.' -f $knownPid)
            }
            if ($knownParentPid -eq [int]$launcher.processId) {
                $parentRecord = $launcher
            }
            else {
                $parentRecords = @($knownRecords | Where-Object { [int]$_.processId -eq $knownParentPid })
                if ($parentRecords.Count -ne 1) {
                    throw ('Retained native parent lineage is unavailable for child PID {0}.' -f $knownPid)
                }
                $parentRecord = $parentRecords[0]
            }
            $parentImage = [string]$parentRecord.imagePath
            $parentObservationFirst = Get-UatNativeProcessObservation -Record $parentRecord -ExpectedImagePath $parentImage
            $parentObservationSecond = Get-UatNativeProcessObservation -Record $parentRecord -ExpectedImagePath $parentImage
            if ([int]$parentObservationFirst.ProcessId -ne $knownParentPid -or
                [long]$parentObservationFirst.CreationUtcTicks -ne [long]$parentRecord.startTicks -or
                [long]$parentObservationSecond.CreationUtcTicks -ne [long]$parentRecord.startTicks -or
                [long]$known.parentStartTicks -ne [long]$parentRecord.startTicks -or
                -not [string]::Equals([string]$parentObservationFirst.ImagePath, $parentImage, [System.StringComparison]::OrdinalIgnoreCase) -or
                -not [string]::Equals([string]$parentObservationSecond.ImagePath, $parentImage, [System.StringComparison]::OrdinalIgnoreCase) -or
                [long]$parentRecord.startTicks -gt $knownTicks) {
                throw ('Retained native parent lineage changed for child PID {0}.' -f $knownPid)
            }
            if ($parentObservationFirst.Signaled -and -not $parentObservationSecond.Signaled) {
                throw ('Retained native parent signal state moved backwards for child PID {0}.' -f $knownPid)
            }
            if ($parentObservationSecond.Signaled -and [long]$knownTicks -gt [long]$parentObservationSecond.ExitUtcTicks) {
                throw ('Retained native child PID {0} started after its retained parent exited.' -f $knownPid)
            }
            if ($knownParentPid -ne [int]$launcher.processId -and
                ([int]$parentRecord.launcherProcessId -ne [int]$launcher.processId -or
                 [long]$parentRecord.launcherStartTicks -ne [long]$launcher.startTicks)) {
                throw ('Retained native parent lost its launcher association for child PID {0}.' -f $knownPid)
            }
            if ($validatedLineageByPid.ContainsKey($knownPid)) {
                throw ('Retained native lineage contains duplicate PID {0}.' -f $knownPid)
            }
            $validatedLineageByPid[$knownPid] = $known
        }
        $retainedLineage = @($untransferredEngines)
        if (-not $launcherProcess.HasExited) {
            [void]$roots.Add($launcher)
        }
        else {
            foreach ($known in $untransferredEngines) {
                if ($null -eq $known.process) { throw ('Retained native child PID {0} lost its process handle.' -f $known.processId) }
                if (-not $known.process.HasExited) { [void]$roots.Add($known) }
            }
        }
        foreach ($root in @($roots.ToArray())) {
            $rootProcess = $root.process
            if ($null -eq $rootProcess) { throw 'Pinned native discovery root lost its Process object.' }
            $rootImage = [string]$root.imagePath
            $rootObservation = Get-UatNativeProcessObservation -Record $root -ExpectedImagePath $rootImage
            $rootHandle = [IntPtr]$root.safeHandle.DangerousGetHandle()
            $rootTicks = [long]$rootObservation.CreationUtcTicks
            if ($rootHandle -eq [IntPtr]::Zero -or $rootTicks -ne [long]$root.startTicks -or
                [int]$rootObservation.ProcessId -ne [int]$root.processId -or [string]::IsNullOrWhiteSpace($rootImage) -or
                -not [string]::Equals([System.IO.Path]::GetFullPath($rootImage), $rootImage, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw ('Pinned native discovery root identity changed: PID {0}.' -f $root.processId)
            }
            $snapshotByPid = $null
            $scanExitedEngine = -not [object]::ReferenceEquals($root, $launcher) -and
                [object]::ReferenceEquals($validatedLineageByPid[[int]$root.processId], $root) -and
                ($null -eq $Entry.launchIntent -or $Entry.launchIntent.state -eq 'transferred')
            if ($hasRetainedEngine) {
                $rows = @(Get-UatProcessDescendantIds -RootProcessId ([int]$root.processId) -RootStartTicks ([long]$root.startTicks) -RetainedRoot $root -RetainedLineage $retainedLineage -AllowNormalRootExit -ScanExitedRetainedEngine:$scanExitedEngine -SnapshotByPid ([ref]$snapshotByPid) -IncludeRows)
                if ($rootProcess.HasExited -and $rows.Count -eq 0) {
                    if (-not $scanExitedEngine -or $null -eq $snapshotByPid -or $snapshotByPid.Count -eq 0 -or
                        $snapshotByPid.ContainsKey([int]$root.processId)) {
                        throw ('Retained native root PID {0} exited without a descendant snapshot; descendant ownership remains unknown.' -f $root.processId)
                    }
                    # The snapshot found no reachable row. Confirm only this retained engine's identity and signal.
                    $exitedRootObservation = Get-UatNativeProcessObservation -Record $root -ExpectedImagePath $rootImage
                    if (-not $exitedRootObservation.Signaled) {
                        throw ('Retained native root PID {0} has no confirmed native exit signal.' -f $root.processId)
                    }
                }
            }
            else {
                $rows = @(Get-UatProcessDescendantIds -RootProcessId ([int]$root.processId) -RootStartTicks ([long]$root.startTicks) -RetainedRoot $root -SnapshotByPid ([ref]$snapshotByPid) -IncludeRows)
            }
            foreach ($observedRow in $rows) { [void]$observedRows.Add($observedRow) }
            $Entry.nativeObservationCount = [int]$Entry.nativeObservationCount + 1
            $activeByPid = @{}
            if (-not $Entry.nativeLauncher.process.HasExited) { $activeByPid[[int]$Entry.nativeLauncher.processId] = $Entry.nativeLauncher }
            foreach ($known in @($Entry.nativeEngineChildren.ToArray())) {
                if ($null -eq $known.process) {
                    if (-not $known.ownershipRoute) { throw ('Retained native child PID {0} lost its process handle.' -f $known.processId) }
                    continue
                }
                if (-not $known.process.HasExited) { $activeByPid[[int]$known.processId] = $known }
            }
            foreach ($row in $rows) {
                $candidatePid = [int]$row.pid
                $candidateRows = @($rows | Where-Object { [int]$_.pid -eq $candidatePid })
                if ($candidateRows.Count -ne 1) { throw ('CIM snapshot does not contain one unique row for native PID {0}.' -f $candidatePid) }
                if ($candidatePid -eq [int]$root.processId) {
                    if ([Math]::Abs([long]$row.cimStartTicks - [long]$root.startTicks) -ge [TimeSpan]::TicksPerMillisecond) {
                        throw ('CIM/native root correlation exceeded 1 ms for PID {0}.' -f $candidatePid)
                    }
                    if (-not [string]::IsNullOrWhiteSpace([string]$row.image) -and
                        -not [string]::Equals([string]$row.image, [string]$root.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
                        throw ('CIM/native root image paths differ for PID {0}.' -f $candidatePid)
                    }
                    continue
                }
                $parentPid = [int]$row.parent
                $existing = @($Entry.nativeEngineChildren.ToArray() | Where-Object { [int]$_.processId -eq $candidatePid })
                if ($existing.Count -gt 1) { throw ('Native process PID has multiple retained identities: {0}.' -f $candidatePid) }
                $parentRows = @($rows | Where-Object { [int]$_.pid -eq $parentPid })
                if ($parentRows.Count -gt 1) { throw ('CIM snapshot contains duplicate native parent rows for PID {0}.' -f $parentPid) }
                $parentRecordMatches = @($knownRecords | Where-Object { [int]$_.processId -eq $parentPid })
                $parentAtCallStart = $null
                if ($parentPid -eq [int]$launcher.processId) {
                    if ($parentRecordMatches.Count -gt 0) { throw ('Native launcher PID {0} has duplicate retained parent identities.' -f $parentPid) }
                    $parentAtCallStart = $launcher
                }
                elseif ($parentRecordMatches.Count -eq 1) {
                    $parentAtCallStart = $parentRecordMatches[0]
                }
                elseif ($parentRecordMatches.Count -gt 1) {
                    throw ('Native child PID {0} has no unique already-owned parent PID {1}.' -f $candidatePid, $parentPid)
                }
                if ($existing.Count -eq 1) {
                    $known = $existing[0]
                    $knownImage = [string]$known.imagePath
                    $knownObservationFirst = Get-UatNativeProcessObservation -Record $known -ExpectedImagePath $knownImage
                    $knownObservationSecond = Get-UatNativeProcessObservation -Record $known -ExpectedImagePath $knownImage
                    $knownTicks = [long]$knownObservationFirst.CreationUtcTicks
                    if ([long]$knownObservationSecond.CreationUtcTicks -ne $knownTicks -or
                        [int]$knownObservationFirst.ProcessId -ne $candidatePid -or [int]$knownObservationSecond.ProcessId -ne $candidatePid -or
                        -not [string]::Equals([string]$knownObservationFirst.ImagePath, [string]$knownImage, [System.StringComparison]::OrdinalIgnoreCase) -or
                        -not [string]::Equals([string]$knownObservationSecond.ImagePath, [string]$knownImage, [System.StringComparison]::OrdinalIgnoreCase) -or
                        [Math]::Abs([long]$row.cimStartTicks - $knownTicks) -ge [TimeSpan]::TicksPerMillisecond) {
                        throw ('Previously pinned native handle or CIM/start identity changed for PID {0}.' -f $candidatePid)
                    }
                    if ([string]::IsNullOrWhiteSpace($knownImage) -or
                        -not [string]::Equals([System.IO.Path]::GetFullPath($knownImage), $knownImage, [System.StringComparison]::OrdinalIgnoreCase)) {
                        throw ('Previously pinned native image proof is unavailable for PID {0}.' -f $candidatePid)
                    }
                    if (-not [string]::IsNullOrWhiteSpace([string]$row.image) -and
                        -not [string]::Equals([string]$row.image, $knownImage, [System.StringComparison]::OrdinalIgnoreCase)) {
                        throw ('Previously pinned native CIM/image correlation changed for PID {0}.' -f $candidatePid)
                    }
                    if ([int]$known.parentProcessId -ne $parentPid -or [long]$known.startTicks -lt [long]$known.parentStartTicks -or
                        [int]$known.launcherProcessId -ne [int]$Entry.nativeLauncher.processId -or
                        [long]$known.launcherStartTicks -ne [long]$Entry.nativeLauncher.startTicks) {
                        throw ('Previously pinned native parent or launcher association changed for PID {0}.' -f $candidatePid)
                    }
                    if ($parentPid -eq [int]$Entry.nativeLauncher.processId) {
                        $parentRecord = $Entry.nativeLauncher
                    }
                    else {
                        $parentRecords = @($Entry.nativeEngineChildren.ToArray() | Where-Object { [int]$_.processId -eq $parentPid })
                        if ($parentRecords.Count -ne 1) { throw ('Retained native parent identity is unavailable for child PID {0}.' -f $candidatePid) }
                        $parentRecord = $parentRecords[0]
                    }
                    $parentImage = [string]$parentRecord.imagePath
                    $parentObservationFirst = Get-UatNativeProcessObservation -Record $parentRecord -ExpectedImagePath $parentImage
                    $parentObservationSecond = Get-UatNativeProcessObservation -Record $parentRecord -ExpectedImagePath $parentImage
                    if ([long]$known.parentStartTicks -ne [long]$parentRecord.startTicks -or
                        [long]$parentObservationFirst.CreationUtcTicks -ne [long]$parentRecord.startTicks -or
                        [long]$parentObservationSecond.CreationUtcTicks -ne [long]$parentRecord.startTicks -or
                        -not [string]::Equals([string]$parentObservationFirst.ImagePath, $parentImage, [System.StringComparison]::OrdinalIgnoreCase) -or
                        -not [string]::Equals([string]$parentObservationSecond.ImagePath, $parentImage, [System.StringComparison]::OrdinalIgnoreCase)) {
                        throw ('Previously pinned native parent handle/start association changed for child PID {0}.' -f $candidatePid)
                    }
                    if ($parentPid -ne [int]$Entry.nativeLauncher.processId -and
                        ([int]$parentRecord.launcherProcessId -ne [int]$Entry.nativeLauncher.processId -or
                         [long]$parentRecord.launcherStartTicks -ne [long]$Entry.nativeLauncher.startTicks)) {
                        throw ('Previously pinned native parent lost its launcher association for child PID {0}.' -f $candidatePid)
                    }
                    if ($parentRows.Count -eq 1) {
                        if ([Math]::Abs([long]$parentRows[0].cimStartTicks - [long]$parentRecord.startTicks) -ge [TimeSpan]::TicksPerMillisecond) {
                            throw ('Previously pinned native parent CIM/start correlation changed for PID {0}.' -f $parentPid)
                        }
                        if (-not [string]::IsNullOrWhiteSpace([string]$parentRows[0].image) -and
                            -not [string]::Equals([string]$parentRows[0].image, [string]$parentRecord.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
                            throw ('Previously pinned native parent CIM/image correlation changed for PID {0}.' -f $parentPid)
                        }
                    }
                    $parentSignaled = [bool]$parentObservationFirst.Signaled -or [bool]$parentObservationSecond.Signaled
                    if ($parentSignaled) {
                        if ($parentObservationFirst.Signaled -and -not $parentObservationSecond.Signaled) { throw ('Retained parent signal state moved backwards for child PID {0}.' -f $candidatePid) }
                        # A missing parent row can corroborate neither a new adoption nor a handoff.
                        $knownParentAbsent = $parentRows.Count -eq 0 -and $null -ne $snapshotByPid -and
                            $snapshotByPid.Count -gt 0 -and -not $snapshotByPid.ContainsKey($parentPid) -and
                            [object]::ReferenceEquals($validatedLineageByPid[$candidatePid], $known) -and
                            ($null -eq $Entry.launchIntent -or $Entry.launchIntent.state -eq 'transferred')
                        $currentParentProven = $parentRows.Count -eq 1 -and
                            [int]$parentRows[0].parent -eq [int]$parentRecord.parentProcessId -and
                            -not [string]::IsNullOrWhiteSpace([string]$parentRows[0].cimExecutablePath) -and
                            [string]::Equals([string]$parentRows[0].cimExecutablePath, $parentImage, [System.StringComparison]::OrdinalIgnoreCase)
                        if ($null -ne $known.ownershipRoute -or $null -ne $parentRecord.ownershipRoute -or
                            -not [object]::ReferenceEquals($parentAtCallStart, $parentRecord) -or
                            -not [object]::ReferenceEquals($validatedLineageByPid[$parentPid], $parentRecord) -or
                            (-not $knownParentAbsent -and -not $currentParentProven)) {
                            throw ('Exited retained parent lacks a current real parent row or eligible owned lineage for child PID {0}.' -f $candidatePid)
                        }
                        if ([long]$knownTicks -gt [long]$parentObservationSecond.ExitUtcTicks) {
                            throw ('Retained child PID {0} started after its native parent exited.' -f $candidatePid)
                        }
                    }
                    elseif ($parentRows.Count -eq 0 -and -not $validatedLineageByPid.ContainsKey($parentPid)) {
                        throw ('Previously pinned native child PID {0} has no current or retained parent identity.' -f $candidatePid)
                    }
                    if (-not $knownObservationSecond.Signaled) { $activeByPid[$candidatePid] = $known }
                    continue
                }
                $parentWasActive = $activeByPid.ContainsKey($parentPid)
                if ($parentWasActive) {
                    $parentEntry = $activeByPid[$parentPid]
                    if ($null -ne $parentAtCallStart -and -not [object]::ReferenceEquals($parentEntry, $parentAtCallStart)) {
                        throw ('Native parent PID {0} does not match the unique retained active identity.' -f $parentPid)
                    }
                }
                elseif ($null -ne $parentAtCallStart) {
                    $parentEntry = $parentAtCallStart
                }
                else {
                    throw ('Native child PID {0} has no unique already-owned parent PID {1}.' -f $candidatePid, $parentPid)
                }
                $parentImage = [string]$parentEntry.imagePath
                $parentObservationBefore = Get-UatNativeProcessObservation -Record $parentEntry -ExpectedImagePath $parentImage
                $parentTicks = [long]$parentObservationBefore.CreationUtcTicks
                if ([int]$parentObservationBefore.ProcessId -ne $parentPid -or
                    ($parentPid -ne [int]$Entry.nativeLauncher.processId -and
                     ([int]$parentEntry.launcherProcessId -ne [int]$Entry.nativeLauncher.processId -or
                      [long]$parentEntry.launcherStartTicks -ne [long]$Entry.nativeLauncher.startTicks))) {
                    throw ('Native child PID {0} does not have its exact already-owned parent identity.' -f $candidatePid)
                }
                if (-not $parentWasActive -and -not $parentObservationBefore.Signaled) {
                    throw ('Native child PID {0} has no pinned live parent PID {1} in the observed chain.' -f $candidatePid, $parentPid)
                }
                if ($parentRows.Count -eq 1) {
                    if ([Math]::Abs([long]$parentRows[0].cimStartTicks - $parentTicks) -ge [TimeSpan]::TicksPerMillisecond) {
                        throw ('CIM/native parent start correlation exceeded 1 ms for PID {0}.' -f $parentPid)
                    }
                    if (-not [string]::IsNullOrWhiteSpace([string]$parentRows[0].image) -and
                        -not [string]::Equals([string]$parentRows[0].image, $parentImage, [System.StringComparison]::OrdinalIgnoreCase)) {
                        throw ('CIM/native parent image paths differ for PID {0}.' -f $parentPid)
                    }
                }
                if ($parentObservationBefore.Signaled) {
                    if ($null -ne $parentEntry.ownershipRoute -or
                        -not [object]::ReferenceEquals($parentAtCallStart, $parentEntry) -or
                        -not [object]::ReferenceEquals($validatedLineageByPid[$parentPid], $parentEntry) -or
                        $parentRows.Count -ne 1 -or [int]$parentRows[0].parent -ne [int]$parentEntry.parentProcessId -or
                        [string]::IsNullOrWhiteSpace([string]$parentRows[0].cimExecutablePath) -or
                        -not [string]::Equals([string]$parentRows[0].cimExecutablePath, $parentImage, [System.StringComparison]::OrdinalIgnoreCase)) {
                        throw ('Exited native parent PID {0} lacks the current real parent row and owned lineage proof.' -f $parentPid)
                    }
                }
                elseif ($parentRows.Count -eq 0 -and -not $validatedLineageByPid.ContainsKey($parentPid)) {
                    throw ('Native child PID {0} has no CIM parent row or validated retained parent identity for PID {1}.' -f $candidatePid, $parentPid)
                }
                $process = [System.Diagnostics.Process]::GetProcessById($candidatePid)
                $processRetained = $false
                try {
                    $safeHandle = $process.SafeHandle
                    if ($null -eq $safeHandle -or $safeHandle.IsInvalid -or $safeHandle.IsClosed) {
                        throw ('Native child PID {0} did not yield one valid retained SafeProcessHandle.' -f $candidatePid)
                    }
                    $handle = [IntPtr]$safeHandle.DangerousGetHandle()
                    if ($handle -eq [IntPtr]::Zero) { throw ('Native child PID {0} has a zero retained handle.' -f $candidatePid) }
                    $first = [Sdir.Installer.Uat.NativeWindowApi]::ObserveProcessIdentity($safeHandle, [string]$row.image)
                    if ([uint32]$first.ProcessId -ne [uint32]$candidatePid -or
                        [Math]::Abs([long]$row.cimStartTicks - [long]$first.CreationUtcTicks) -ge [TimeSpan]::TicksPerMillisecond -or
                        [long]$first.CreationUtcTicks -lt $parentTicks -or
                        (-not [string]::IsNullOrWhiteSpace([string]$row.image) -and
                         -not [string]::Equals([string]$first.ImagePath, [string]$row.image, [System.StringComparison]::OrdinalIgnoreCase))) {
                        throw ('First retained native proof did not match the unique CIM row and exact parent edge for PID {0}.' -f $candidatePid)
                    }
                    $parentObservationMiddle = Get-UatNativeProcessObservation -Record $parentEntry -ExpectedImagePath $parentImage
                    $second = [Sdir.Installer.Uat.NativeWindowApi]::ObserveProcessIdentity($safeHandle, [string]$row.image)
                    $parentObservationAfter = Get-UatNativeProcessObservation -Record $parentEntry -ExpectedImagePath $parentImage
                    if (-not [object]::ReferenceEquals($safeHandle, $process.SafeHandle) -or
                        [IntPtr]$safeHandle.DangerousGetHandle() -ne $handle -or
                        [uint32]$second.ProcessId -ne [uint32]$candidatePid -or
                        [long]$second.CreationUtcTicks -ne [long]$first.CreationUtcTicks -or
                        -not [string]::Equals([string]$second.ImagePath, [string]$first.ImagePath, [System.StringComparison]::OrdinalIgnoreCase) -or
                        [Math]::Abs([long]$row.cimStartTicks - [long]$second.CreationUtcTicks) -ge [TimeSpan]::TicksPerMillisecond) {
                        throw ('Second retained native proof changed PID, exact UTC creation ticks, full image path, SafeHandle, or CIM correlation for PID {0}.' -f $candidatePid)
                    }
                    if ([long]$parentObservationMiddle.CreationUtcTicks -ne $parentTicks -or
                        [long]$parentObservationAfter.CreationUtcTicks -ne $parentTicks -or
                        -not [string]::Equals([string]$parentObservationMiddle.ImagePath, $parentImage, [System.StringComparison]::OrdinalIgnoreCase) -or
                        -not [string]::Equals([string]$parentObservationAfter.ImagePath, $parentImage, [System.StringComparison]::OrdinalIgnoreCase)) {
                        throw ('Native parent PID {0} changed identity during the child proof.' -f $parentPid)
                    }
                    if (($parentObservationBefore.Signaled -and -not $parentObservationMiddle.Signaled) -or
                        ($parentObservationMiddle.Signaled -and -not $parentObservationAfter.Signaled)) {
                        throw ('Native parent PID {0} signal state moved backwards during the child proof.' -f $parentPid)
                    }
                    $parentExitTicks = $null
                    foreach ($parentObservation in @($parentObservationBefore, $parentObservationMiddle, $parentObservationAfter)) {
                        if ($parentObservation.Signaled) {
                            if ($null -eq $parentExitTicks) { $parentExitTicks = [long]$parentObservation.ExitUtcTicks }
                            elseif ([long]$parentExitTicks -ne [long]$parentObservation.ExitUtcTicks) {
                                throw ('Native parent PID {0} has inconsistent exit times across the retained-handle proof.' -f $parentPid)
                            }
                        }
                    }
                    if ($null -ne $parentExitTicks) {
                        if ($null -ne $parentEntry.ownershipRoute -or
                            -not [object]::ReferenceEquals($parentAtCallStart, $parentEntry) -or
                            -not [object]::ReferenceEquals($validatedLineageByPid[$parentPid], $parentEntry) -or
                            $parentRows.Count -ne 1 -or [int]$parentRows[0].parent -ne [int]$parentEntry.parentProcessId -or
                            [string]::IsNullOrWhiteSpace([string]$parentRows[0].cimExecutablePath) -or
                            -not [string]::Equals([string]$parentRows[0].cimExecutablePath, $parentImage, [System.StringComparison]::OrdinalIgnoreCase) -or
                            $parentTicks -gt [long]$first.CreationUtcTicks -or [long]$first.CreationUtcTicks -gt [long]$parentExitTicks) {
                            throw ('Native child PID {0} failed the exact current-row parent-exit proof.' -f $candidatePid)
                        }
                    }
                    elseif (-not $parentWasActive) {
                        throw ('Native child PID {0} did not retain an active parent during its identity proof.' -f $candidatePid)
                    }
                    $started = [DateTime]::new([long]$first.CreationUtcTicks, [DateTimeKind]::Utc)
                    $imagePath = [string]$first.ImagePath
                    $record = [pscustomobject]@{
                        processId = $candidatePid; startUtc = $started; startTicks = [long]$first.CreationUtcTicks; imagePath = $imagePath
                        parentProcessId = $parentPid; parentStartTicks = $parentTicks; launcherProcessId = [int]$Entry.nativeLauncher.processId
                        launcherStartTicks = [long]$Entry.nativeLauncher.startTicks; observedUtc = [DateTime]::UtcNow.ToString('o')
                        process = $process; safeHandle = $safeHandle; pinnedHandle = $handle; finished = $false; exitCode = $null
                        outputCapture = 'no independent stream exposed'; completedUtc = $null
                        ownershipRoute = $null; ownershipCommandId = $null; commandLine = [string]$row.commandLine
                    }
                    [void]$Entry.nativeEngineChildren.Add($record)
                    $processRetained = $true
                    if (-not $second.Signaled) { $activeByPid[$candidatePid] = $record }
                    Add-UatEvent -Kind 'native-engine-observed' -Name $Entry.name -Data @{ commandId = $Entry.commandId; processId = $candidatePid; startTicks = $record.startTicks; imagePath = $imagePath; parentProcessId = $parentPid; parentStartTicks = $parentTicks; launcherProcessId = $Entry.nativeLauncher.processId; observedUtc = $record.observedUtc }
                }
                finally { if (-not $processRetained) { $process.Dispose() } }
            }
        }
        if ($Entry.nativeLauncher.process.HasExited -and [int]$Entry.nativeEngineChildren.Count -eq 0 -and
            ($null -eq $Entry.launchIntent -or $Entry.launchIntent.state -ne 'transferred')) {
            throw ('Launcher exited before any native engine child was observed for command {0}.' -f $Entry.commandId)
        }
        if ($null -ne $Entry.launchIntent -and $Entry.launchIntent.state -eq 'pending') {
            [void](Update-UatSetupLaunchHandoff -Entry $Entry -LaunchIntent $Entry.launchIntent -ObservedRows @($observedRows.ToArray()))
        }
    }
    catch {
        $diagnostic = $_.Exception.ToString()
        if ($null -eq $Entry.nativeObservationError) { $Entry.nativeObservationError = $diagnostic }
        Add-UatEvent -Kind 'native-observation-incomplete' -Name $Entry.name -Data @{ commandId = $Entry.commandId; processId = $Entry.processId; diagnostic = $diagnostic; observedUtc = [DateTime]::UtcNow.ToString('o') }
        throw
    }
}


function Update-UatSetupLaunchHandoff {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)]$LaunchIntent,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$ObservedRows
    )
    if (-not [object]::ReferenceEquals($Entry.launchIntent, $LaunchIntent)) {
        throw ('Launch handoff received a different intent object for setup command {0}.' -f $Entry.commandId)
    }
    if ($LaunchIntent.state -eq 'transferred') { return $true }
    if ($LaunchIntent.state -ne 'pending') {
        throw ('Launch handoff is already incomplete for intent {0}: {1}' -f $LaunchIntent.intentId, $LaunchIntent.diagnostic)
    }
    if ($null -eq $LaunchIntent.clickNotBeforeUtc) { return $false }
    if ($null -eq $Entry.completionBudget -or -not [object]::ReferenceEquals($Entry.completionBudget, $LaunchIntent.completionBudget)) {
        throw ('Launch handoff lost the setup entry original CompletionBudget: {0}.' -f $Entry.commandId)
    }
    if ($null -eq $Entry.nativeLauncher -or $null -eq $Entry.nativeLauncher.process -or
        $null -eq $Entry.nativeEngineChildren) {
        throw ('Launch handoff has no retained native observer identities for command {0}.' -f $Entry.commandId)
    }
    try {
        if ($Entry.nativeLauncher.process.HasExited) { throw 'Pending setup handoff lost its live pinned launcher; no parent chain is reconstructed.' }
        $currentRows = New-Object System.Collections.Generic.List[object]
        $rowByIdentity = @{}
        foreach ($row in @($ObservedRows)) {
            if ($null -eq $row) { continue }
            $processId = [int]$row.pid
            if ($processId -eq [int]$Entry.nativeLauncher.processId) { continue }
            $matches = @($Entry.nativeEngineChildren | Where-Object {
                [int]$_.processId -eq $processId -and
                [Math]::Abs([long]$_.startTicks - [long]$row.cimStartTicks) -lt [TimeSpan]::TicksPerMillisecond -and
                [int]$_.parentProcessId -eq [int]$row.parent
            })
            if ($matches.Count -gt 1) { throw ('Current CIM row is ambiguous against pinned native identities for PID {0}.' -f $processId) }
            if ($matches.Count -eq 0) { continue }
            $record = $matches[0]
            if ([long]$record.parentStartTicks -le 0 -or [long]$record.startTicks -lt [long]$record.parentStartTicks) {
                throw ('Current CIM row has no exact pinned native parent edge for PID {0}.' -f $processId)
            }
            if (-not [string]::IsNullOrWhiteSpace([string]$row.image) -and
                -not [string]::Equals([System.IO.Path]::GetFullPath([string]$row.image), [string]$record.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw ('Current CIM and pinned native image paths differ for PID {0}.' -f $processId)
            }
            $identityKey = '{0}|{1}' -f $processId, [long]$record.startTicks
            $commandLine = [string]$row.commandLine
            if ($rowByIdentity.ContainsKey($identityKey)) {
                if (-not [string]::Equals([string]$rowByIdentity[$identityKey], $commandLine, [System.StringComparison]::Ordinal)) {
                    throw ('Current CIM snapshots disagree on the full command line for PID {0}.' -f $processId)
                }
                continue
            }
            $rowByIdentity[$identityKey] = $commandLine
            [void]$currentRows.Add([pscustomobject]@{ record = $record; commandLine = $commandLine })
        }
        $appCandidates = @($currentRows | Where-Object {
            [string]::Equals([System.IO.Path]::GetFullPath([string]$_.record.imagePath), [string]$LaunchIntent.appPath, [System.StringComparison]::OrdinalIgnoreCase)
        })
        if ($appCandidates.Count -gt 1) { throw ('More than one pinned TEST SDIR.exe candidate is attached to setup command {0}.' -f $Entry.commandId) }
        if ($appCandidates.Count -eq 0) { return $false }
        $appRow = $appCandidates[0]
        $appRecord = $appRow.record
        if ($appRecord.startUtc -lt $LaunchIntent.clickNotBeforeUtc) {
            throw ('TEST app PID {0} started before the recorded Finish click.' -f $appRecord.processId)
        }
        if ([int]$appRecord.parentProcessId -eq [int]$Entry.nativeLauncher.processId -and
            [long]$appRecord.parentStartTicks -eq [long]$Entry.nativeLauncher.startTicks) {
            $wrapperRecord = $Entry.nativeLauncher
        }
        else {
            $wrapperMatches = @($Entry.nativeEngineChildren | Where-Object {
                [int]$_.processId -eq [int]$appRecord.parentProcessId -and
                [long]$_.startTicks -eq [long]$appRecord.parentStartTicks
            })
            if ($wrapperMatches.Count -ne 1) {
                throw ('TEST app PID {0} has no unique pinned direct-parent identity.' -f $appRecord.processId)
            }
            $wrapperRecord = $wrapperMatches[0]
        }
        if (-not [string]::Equals([System.IO.Path]::GetFullPath([string]$wrapperRecord.imagePath), [string]$LaunchIntent.expectedCmdExePath, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw ('TEST app PID {0} was not a direct child of the verified SystemRoot cmd.exe wrapper.' -f $appRecord.processId)
        }
        if ([long]$appRecord.parentStartTicks -ne [long]$wrapperRecord.startTicks -or
            [int]$appRecord.parentProcessId -ne [int]$wrapperRecord.processId -or
            $appRecord.startUtc -lt $wrapperRecord.startUtc) {
            throw ('TEST app PID {0} did not have the exact direct wrapper parent edge.' -f $appRecord.processId)
        }
        $wrapperKey = '{0}|{1}' -f [int]$wrapperRecord.processId, [long]$wrapperRecord.startTicks
        if (-not $rowByIdentity.ContainsKey($wrapperKey)) {
            throw ('Pinned cmd wrapper PID {0} is absent from the same native CIM observation as its app.' -f $wrapperRecord.processId)
        }
        $wrapperCommandLine = [string]$rowByIdentity[$wrapperKey]
        $allowedLines = @($LaunchIntent.expectedWrapperCommandLines | Where-Object {
            [string]::Equals([string]$_, $wrapperCommandLine, [System.StringComparison]::Ordinal)
        })
        if ($allowedLines.Count -ne 1) {
            throw ('Full Ordinal command line for pinned cmd wrapper PID {0} did not match the finite click-time policy.' -f $wrapperRecord.processId)
        }
        if ([string]::IsNullOrWhiteSpace([string]$appRow.commandLine)) {
            throw ('Full command line was unavailable for pinned TEST app PID {0}.' -f $appRecord.processId)
        }
        $verifyPinned = {
            param($NativeRecord, [string]$ExpectedImagePath, [string]$ExpectedSha256)
            if ($null -eq $NativeRecord.process -or $null -eq $NativeRecord.pinnedHandle) {
                throw ('Native Process or retained handle is missing for PID {0}.' -f $NativeRecord.processId)
            }
            $process = $NativeRecord.process
            $handle = $process.Handle
            if ($handle -eq [IntPtr]::Zero -or $handle -ne $NativeRecord.pinnedHandle -or $process.HasExited) {
                throw ('Pinned native process is not live on its retained handle: PID {0}.' -f $NativeRecord.processId)
            }
            $nativeStartTicks = [long]$process.StartTime.ToUniversalTime().Ticks
            $nativeImagePath = [System.IO.Path]::GetFullPath([Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath([int]$NativeRecord.processId))
            if ($nativeStartTicks -ne [long]$NativeRecord.startTicks -or
                -not [string]::Equals($nativeImagePath, [string]$NativeRecord.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw ('Pinned native PID/start/path changed for PID {0}.' -f $NativeRecord.processId)
            }
            if (-not [string]::IsNullOrEmpty($ExpectedImagePath) -and
                -not [string]::Equals($nativeImagePath, [System.IO.Path]::GetFullPath($ExpectedImagePath), [System.StringComparison]::OrdinalIgnoreCase)) {
                throw ('Pinned native image path differs from the launch intent for PID {0}.' -f $NativeRecord.processId)
            }
            $sha256 = Get-UatFileHash $nativeImagePath
            if (-not [string]::IsNullOrEmpty($ExpectedSha256) -and
                -not [string]::Equals($sha256, $ExpectedSha256, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw ('Pinned native image SHA-256 differs from the launch intent for PID {0}.' -f $NativeRecord.processId)
            }
            if ($process.HasExited -or [long]$process.StartTime.ToUniversalTime().Ticks -ne $nativeStartTicks) {
                throw ('Pinned native process exited or changed during identity verification: PID {0}.' -f $NativeRecord.processId)
            }
            return [pscustomobject]@{
                processId = [int]$NativeRecord.processId; nativeStartTicks = $nativeStartTicks
                startUtc = $process.StartTime.ToUniversalTime(); imagePath = $nativeImagePath; sha256 = $sha256
                process = $process; pinnedHandle = $handle
            }
        }
        $wrapperVerified = & $verifyPinned $wrapperRecord $LaunchIntent.expectedCmdExePath $LaunchIntent.expectedCmdExeSha256
        $appVerified = & $verifyPinned $appRecord $LaunchIntent.appPath $LaunchIntent.appSha256
        if ($wrapperRecord.startUtc -lt $LaunchIntent.clickNotBeforeUtc -or $appRecord.startUtc -lt $LaunchIntent.clickNotBeforeUtc) {
            throw 'TEST wrapper and app must both start after the recorded Finish click.'
        }
        if ($null -ne $wrapperRecord.ownershipRoute -or $null -ne $appRecord.ownershipRoute) {
            throw 'TEST wrapper or app already has an ownership route before verified handoff registration.'
        }
        $wrapperIdentity = [pscustomobject]@{
            processId = [int]$wrapperRecord.processId; nativeStartTicks = [long]$wrapperVerified.nativeStartTicks
            startTicks = [long]$wrapperVerified.nativeStartTicks; startUtc = $wrapperVerified.startUtc
            imagePath = $wrapperVerified.imagePath; sha256 = $wrapperVerified.sha256
            commandLine = $wrapperCommandLine; parentProcessId = [int]$wrapperRecord.parentProcessId
            parentNativeStartTicks = [long]$wrapperRecord.parentStartTicks; process = $wrapperVerified.process
            pinnedHandle = $wrapperVerified.pinnedHandle; observedUtc = [string]$wrapperRecord.observedUtc; exitCode = $null
        }
        $appIdentity = [pscustomobject]@{
            processId = [int]$appRecord.processId; nativeStartTicks = [long]$appVerified.nativeStartTicks
            startTicks = [long]$appVerified.nativeStartTicks; startUtc = $appVerified.startUtc
            imagePath = $appVerified.imagePath; sha256 = $appVerified.sha256
            commandLine = [string]$appRow.commandLine; parentProcessId = [int]$appRecord.parentProcessId
            parentNativeStartTicks = [long]$appRecord.parentStartTicks; process = $appVerified.process
            pinnedHandle = $appVerified.pinnedHandle; observedUtc = [string]$appRecord.observedUtc; exitCode = $null
        }
        $engineIdentities = New-Object System.Collections.Generic.List[object]
        $visitedEngines = @{}
        $currentEngine = $wrapperRecord
        while ($true) {
            $engineKey = '{0}|{1}' -f [int]$currentEngine.processId, [long]$currentEngine.startTicks
            if ($visitedEngines.ContainsKey($engineKey)) { throw 'Native engine parent chain contains a repeated pinned identity.' }
            $visitedEngines[$engineKey] = $true
            $engineVerified = & $verifyPinned $currentEngine ([string]$currentEngine.imagePath) ''
            $engineCommandLine = [string]$currentEngine.commandLine
            if ($rowByIdentity.ContainsKey($engineKey)) { $engineCommandLine = [string]$rowByIdentity[$engineKey] }
            $engineObservedUtc = $Entry.nativeObservationStartedUtc
            if ($null -ne $currentEngine.PSObject.Properties['observedUtc']) { $engineObservedUtc = [string]$currentEngine.observedUtc }
            [void]$engineIdentities.Add([pscustomobject]@{
                processId = [int]$currentEngine.processId; nativeStartTicks = [long]$engineVerified.nativeStartTicks
                startUtc = $engineVerified.startUtc; imagePath = $engineVerified.imagePath; sha256 = $engineVerified.sha256
                parentProcessId = [int]$currentEngine.parentProcessId; parentNativeStartTicks = [long]$currentEngine.parentStartTicks
                commandLine = $engineCommandLine; process = $engineVerified.process; pinnedHandle = $engineVerified.pinnedHandle
                observedUtc = $engineObservedUtc
            })
            if ([object]::ReferenceEquals($currentEngine, $Entry.nativeLauncher)) { break }
            $parentId = [int]$currentEngine.parentProcessId
            $parentTicks = [long]$currentEngine.parentStartTicks
            if ($parentId -eq [int]$Entry.nativeLauncher.processId -and $parentTicks -eq [long]$Entry.nativeLauncher.startTicks) {
                $currentEngine = $Entry.nativeLauncher
            }
            else {
                $parentMatches = @($Entry.nativeEngineChildren | Where-Object {
                    [int]$_.processId -eq $parentId -and [long]$_.startTicks -eq $parentTicks
                })
                if ($parentMatches.Count -ne 1) { throw ('Native engine chain lost its exact parent identity at PID {0}.' -f $parentId) }
                $currentEngine = $parentMatches[0]
            }
        }
        $engineChain = [object[]]$engineIdentities.ToArray()
        [array]::Reverse($engineChain)
        $appDescendants = New-Object System.Collections.Generic.List[object]
        $descendantRecords = New-Object System.Collections.Generic.List[object]
        $frontier = New-Object System.Collections.Generic.List[object]
        [void]$frontier.Add($appRecord)
        $seenDescendants = @{}
        $frontierIndex = 0
        while ($frontierIndex -lt $frontier.Count) {
            $parentRecord = $frontier[$frontierIndex]
            $frontierIndex++
            foreach ($currentRow in $currentRows) {
                $descendantRecord = $currentRow.record
                if ([int]$descendantRecord.parentProcessId -ne [int]$parentRecord.processId -or
                    [long]$descendantRecord.parentStartTicks -ne [long]$parentRecord.startTicks) { continue }
                $descendantKey = '{0}|{1}' -f [int]$descendantRecord.processId, [long]$descendantRecord.startTicks
                if ($seenDescendants.ContainsKey($descendantKey)) { continue }
                if ($null -ne $descendantRecord.ownershipRoute) { throw ('App descendant PID {0} already has an ownership route.' -f $descendantRecord.processId) }
                $seenDescendants[$descendantKey] = $true
                $descendantVerified = & $verifyPinned $descendantRecord ([string]$descendantRecord.imagePath) ''
                $descendantIdentity = [pscustomobject]@{
                    processId = [int]$descendantRecord.processId; nativeStartTicks = [long]$descendantVerified.nativeStartTicks
                    startTicks = [long]$descendantVerified.nativeStartTicks; startUtc = $descendantVerified.startUtc
                    imagePath = $descendantVerified.imagePath; sha256 = $descendantVerified.sha256
                    commandLine = [string]$currentRow.commandLine; parentProcessId = [int]$descendantRecord.parentProcessId
                    parentNativeStartTicks = [long]$descendantRecord.parentStartTicks; process = $descendantVerified.process
                    pinnedHandle = $descendantVerified.pinnedHandle; observedUtc = [string]$descendantRecord.observedUtc
                    exitCode = $null; nativeRecord = $descendantRecord
                }
                [void]$appDescendants.Add($descendantIdentity)
                [void]$descendantRecords.Add($descendantRecord)
                [void]$frontier.Add($descendantRecord)
            }
        }
        $verifiedUtc = [DateTime]::UtcNow.ToString('o')
        $wrapperEvidence = [ordered]@{
            processId = $wrapperIdentity.processId; nativeStartTicks = $wrapperIdentity.nativeStartTicks
            startUtc = $wrapperIdentity.startUtc.ToString('o'); imagePath = $wrapperIdentity.imagePath
            sha256 = $wrapperIdentity.sha256; commandLine = $wrapperIdentity.commandLine
            parentProcessId = $wrapperIdentity.parentProcessId; parentNativeStartTicks = $wrapperIdentity.parentNativeStartTicks
            observedUtc = $wrapperIdentity.observedUtc; exitCode = $null
        }
        $appEvidence = [ordered]@{
            processId = $appIdentity.processId; nativeStartTicks = $appIdentity.nativeStartTicks
            startUtc = $appIdentity.startUtc.ToString('o'); imagePath = $appIdentity.imagePath
            sha256 = $appIdentity.sha256; commandLine = $appIdentity.commandLine
            parentProcessId = $appIdentity.parentProcessId; parentNativeStartTicks = $appIdentity.parentNativeStartTicks
            observedUtc = $appIdentity.observedUtc; exitCode = $null
        }
        $engineEvidence = @($engineChain | ForEach-Object {
            [ordered]@{
                processId = $_.processId; nativeStartTicks = $_.nativeStartTicks; startUtc = $_.startUtc.ToString('o')
                imagePath = $_.imagePath; sha256 = $_.sha256; parentProcessId = $_.parentProcessId
                parentNativeStartTicks = $_.parentNativeStartTicks; commandLine = $_.commandLine; observedUtc = $_.observedUtc
            }
        })
        $descendantEvidence = @($appDescendants | ForEach-Object {
            [ordered]@{
                processId = $_.processId; nativeStartTicks = $_.nativeStartTicks; startUtc = $_.startUtc.ToString('o')
                imagePath = $_.imagePath; sha256 = $_.sha256; commandLine = $_.commandLine
                parentProcessId = $_.parentProcessId; parentNativeStartTicks = $_.parentNativeStartTicks
                observedUtc = $_.observedUtc; exitCode = $null
            }
        })
        $commandEvidence = [ordered]@{
            intentId = $LaunchIntent.intentId; setupCommandId = $LaunchIntent.setupCommandId
            ownershipCommandId = $null; commandLinePolicyId = $LaunchIntent.commandLinePolicyId
            expectedWrapperCommandLines = @($LaunchIntent.expectedWrapperCommandLines)
            expectedWrapperCommandLineTemplates = @($LaunchIntent.expectedWrapperCommandLineTemplates)
            verifiedUtc = $verifiedUtc; wrapper = $wrapperEvidence; app = $appEvidence
            engineChain = $engineEvidence; appDescendants = $descendantEvidence
        }
        $nativeRecords = New-Object System.Collections.Generic.List[object]
        [void]$nativeRecords.Add($wrapperRecord)
        [void]$nativeRecords.Add($appRecord)
        foreach ($descendantRecord in $descendantRecords) { [void]$nativeRecords.Add($descendantRecord) }
        $handoff = [pscustomobject]@{
            intentId = $LaunchIntent.intentId; setupCommandId = $LaunchIntent.setupCommandId
            installRoot = $LaunchIntent.installRoot; wrapper = $wrapperIdentity; app = $appIdentity
            engineChain = $engineChain; verifiedUtc = $verifiedUtc; ownershipCommandId = $null
            appDescendants = @($appDescendants.ToArray()); nativeRecords = @($nativeRecords.ToArray())
            commandEvidence = $commandEvidence
        }
        $LaunchIntent.verifiedHandoff = $handoff
        Add-UatEvent -Kind 'setup-launch-handoff-verified' -Name $Entry.name -Data @{
            commandId = $Entry.commandId; intentId = $LaunchIntent.intentId; ownershipCommandId = $null
            commandLinePolicyId = $LaunchIntent.commandLinePolicyId; handoff = $commandEvidence
        }
        $registeredApp = Register-UatExternalAppProcess -ProcessIdentity $appIdentity -Root $LaunchIntent.installRoot `
            -Name 'app-launch-wizard' -Route 'Inno [Run] isolated test launcher' -VerifiedLaunchHandoff $handoff -DeferWindow
        $handoff.ownershipCommandId = [string]$registeredApp.ownedProcess.commandId
        $handoff.commandEvidence['ownershipCommandId'] = $handoff.ownershipCommandId
        $LaunchIntent.registeredApp = $registeredApp
        $transferIdentities = @($wrapperIdentity, $appIdentity) + @($appDescendants.ToArray())
        foreach ($engineIdentity in @($handoff.engineChain)) {
            $engineProcess = $engineIdentity.process
            if ($null -eq $engineProcess -or $engineProcess.HasExited -or
                $engineProcess.Handle -ne $engineIdentity.pinnedHandle -or
                [long]$engineProcess.StartTime.ToUniversalTime().Ticks -ne [long]$engineIdentity.nativeStartTicks -or
                -not [string]::Equals([System.IO.Path]::GetFullPath([Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath([int]$engineIdentity.processId)), [string]$engineIdentity.imagePath, [System.StringComparison]::OrdinalIgnoreCase) -or
                -not [string]::Equals((Get-UatFileHash $engineIdentity.imagePath), [string]$engineIdentity.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw ('Pinned native engine chain changed after app registration: PID {0}.' -f $engineIdentity.processId)
            }
        }
        foreach ($nativeRecord in @($handoff.nativeRecords)) {
            $transferMatches = @($transferIdentities | Where-Object {
                [int]$_.processId -eq [int]$nativeRecord.processId -and [long]$_.nativeStartTicks -eq [long]$nativeRecord.startTicks
            })
            if ($transferMatches.Count -ne 1) { throw 'Native transfer does not have one verified pinned identity.' }
            $transferIdentity = $transferMatches[0]
            if ($null -ne $nativeRecord.ownershipRoute -or $null -eq $nativeRecord.process -or $nativeRecord.process.HasExited -or
                -not [object]::ReferenceEquals($nativeRecord.process, $transferIdentity.process) -or
                $nativeRecord.process.Handle -ne $nativeRecord.pinnedHandle -or
                $nativeRecord.pinnedHandle -ne $transferIdentity.pinnedHandle -or
                [long]$nativeRecord.process.StartTime.ToUniversalTime().Ticks -ne [long]$transferIdentity.nativeStartTicks) {
                throw ('Native history changed before ownership transfer for PID {0}.' -f $nativeRecord.processId)
            }
        }
        foreach ($nativeRecord in @($handoff.nativeRecords)) {
            $nativeRecord.ownershipRoute = 'app-launch-wizard'
            $nativeRecord.ownershipCommandId = $handoff.ownershipCommandId
        }
        $LaunchIntent.state = 'transferred'
        $LaunchIntent.diagnostic = $null
        Add-UatEvent -Kind 'setup-launch-handoff-transferred' -Name $Entry.name -Data @{
            commandId = $Entry.commandId; intentId = $LaunchIntent.intentId
            ownershipCommandId = $handoff.ownershipCommandId; wrapperProcessId = $wrapperIdentity.processId
            appProcessId = $appIdentity.processId; transferredAppDescendantProcessIds = @($appDescendants | ForEach-Object { $_.processId })
        }
        return $true
    }
    catch {
        $LaunchIntent.state = 'incomplete'
        $LaunchIntent.diagnostic = $_.Exception.ToString()
        throw
    }
}

function Get-UatNativeWindowProcessRecords {
    param([Parameter(Mandatory = $true)]$ProcessEntry)
    if (-not $ProcessEntry.nativeTracking) { throw ('Native window routing requires tracked process entry: {0}' -f $ProcessEntry.commandId) }
    if ($null -eq $ProcessEntry.completionBudget) { throw ('Native window routing has no entry CompletionBudget: {0}' -f $ProcessEntry.commandId) }
    Update-UatNativeEngineChildren -Entry $ProcessEntry
    if ($null -ne $ProcessEntry.nativeObservationError) { throw ('Native window routing observation is incomplete: {0}' -f $ProcessEntry.nativeObservationError) }
    $records = New-Object System.Collections.Generic.List[object]
    if ($null -ne $ProcessEntry.nativeLauncher -and $null -ne $ProcessEntry.nativeLauncher.process -and -not $ProcessEntry.nativeLauncher.process.HasExited) {
        [void]$records.Add($ProcessEntry.nativeLauncher)
    }
    foreach ($engine in @($ProcessEntry.nativeEngineChildren.ToArray())) {
        if ($null -ne $engine -and -not $engine.ownershipRoute -and $null -ne $engine.process -and -not $engine.process.HasExited) {
            [void]$records.Add($engine)
        }
    }
    if ($records.Count -eq 0) { throw ('No live pinned launcher or native engine identity remains for command {0}.' -f $ProcessEntry.commandId) }
    $seen = @{}
    foreach ($record in $records) {
        $processId = [int]$record.processId
        if ($seen.ContainsKey($processId)) { throw ('Native window identity set contains duplicate live PID {0}.' -f $processId) }
        $seen[$processId] = $true
        $process = $record.process
        $handle = $process.Handle
        if ($handle -eq [IntPtr]::Zero -or $process.HasExited) { throw ('Pinned native window process exited before identity verification: PID {0}.' -f $processId) }
        $startTicks = [long]$process.StartTime.ToUniversalTime().Ticks
        $imagePath = [System.IO.Path]::GetFullPath([Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath($processId))
        if ($startTicks -ne [long]$record.startTicks -or -not [string]::Equals($imagePath, [string]$record.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw ('Pinned native window process identity changed: PID {0}.' -f $processId)
        }
        if ([Sdir.Installer.Uat.NativeWindowApi]::IsProcessElevated($processId)) {
            throw ('Pinned native window process unexpectedly has an elevated token: PID {0}.' -f $processId)
        }
        if ($process.HasExited -or [long]$process.StartTime.ToUniversalTime().Ticks -ne $startTicks -or
            -not [string]::Equals([System.IO.Path]::GetFullPath([Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath($processId)), $imagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw ('Pinned native window process identity changed during verification: PID {0}.' -f $processId)
        }
    }
    return @($records.ToArray())
}

function Test-UatProcessComplete {
    param([Parameter(Mandatory = $true)]$Entry)
    if ($Entry.finished) { return $true }
    if ($null -eq $Entry.child) { throw ('Owned process entry has no output-capture child: {0}' -f $Entry.name) }
    if ($Entry.nativeTracking) {
        Update-UatNativeEngineChildren -Entry $Entry
        if ($null -ne $Entry.nativeObservationError) { throw ('Native observation is incomplete for {0}: {1}' -f $Entry.name, $Entry.nativeObservationError) }
        if ($null -ne $Entry.launchIntent -and $Entry.launchIntent.state -ne 'transferred') { return $false }
        if (-not $Entry.nativeLauncher.process.HasExited) { return $false }
        if (@($Entry.nativeEngineChildren | Where-Object { -not $_.ownershipRoute }).Count -eq 0 -and
            ($null -eq $Entry.launchIntent -or $Entry.launchIntent.state -ne 'transferred')) {
            throw ('Native completion cannot be inferred from launcher exit without an observed engine child: {0}' -f $Entry.commandId)
        }
        foreach ($engine in @($Entry.nativeEngineChildren | Where-Object { -not $_.ownershipRoute })) {
            if ($engine.finished) { continue }
            if ($null -eq $engine.process -or $null -eq $engine.PSObject.Properties['safeHandle'] -or
                $null -eq $engine.safeHandle -or $engine.safeHandle.IsInvalid -or $engine.safeHandle.IsClosed) {
                throw ('Native engine retained handle is unavailable for PID {0}.' -f $engine.processId)
            }
            if (-not [object]::ReferenceEquals($engine.safeHandle, $engine.process.SafeHandle) -or
                [IntPtr]$engine.safeHandle.DangerousGetHandle() -ne [IntPtr]$engine.pinnedHandle) {
                throw ('Native engine SafeProcessHandle changed before exit-code capture for PID {0}.' -f $engine.processId)
            }
            $nativeExit = [Sdir.Installer.Uat.NativeWindowApi]::ReadSignaledProcessExitStatus($engine.safeHandle)
            if (-not $nativeExit.Signaled) { return $false }
            $engine.exitCode = [int]$nativeExit.ExitCode
            $engine.finished = $true
            $engine.completedUtc = [DateTime]::UtcNow.ToString('o')
        }
    }
    return [bool]$Entry.child.HasExited
}

function Test-UatNativeRecoverySafe {
    param(
        [Parameter(Mandatory = $true)][string]$RecoveryName,
        [object]$RecoveryRecord = $null
    )
    $pendingReason = $null
    $pendingCommandId = $null
    $pendingObservationError = $null
    try {
        $entries = @($script:activeProcesses | Where-Object {
            $_.kind -eq 'owned-child' -and ($_.nativeTracking -or $null -ne $_.captureIntent)
        })
        foreach ($entry in $entries) {
            if ($null -ne $entry.captureIntent) {
                if (-not $entry.finished -or -not $entry.portableRefusalQualified) {
                    $pendingReason = 'A6 root-only expected refusal was not functionally qualified'
                    $pendingCommandId = $entry.commandId
                    break
                }
                continue
            }
            try {
                if ($null -ne $entry.nativeObservationError) {
                    $pendingReason = 'native process observation incomplete'
                    $pendingCommandId = $entry.commandId
                    $pendingObservationError = $entry.nativeObservationError
                }
                elseif (-not $entry.finished) {
                    [void](Update-UatNativeEngineChildren -Entry $entry)
                    if ($null -ne $entry.nativeObservationError) {
                        $pendingReason = 'native process observation incomplete'
                        $pendingCommandId = $entry.commandId
                        $pendingObservationError = $entry.nativeObservationError
                    }
                    elseif (-not (Test-UatProcessComplete -Entry $entry)) {
                        $pendingReason = 'native process chain remains active'
                        $pendingCommandId = $entry.commandId
                    }
                }
            }
            catch {
                $pendingReason = 'native process observation incomplete'
                $pendingCommandId = $entry.commandId
                if ($null -ne $entry.nativeObservationError) {
                    $pendingObservationError = $entry.nativeObservationError
                }
                else {
                    $pendingObservationError = $_
                }
            }
            if ($null -ne $pendingReason) { break }
        }
    }
    catch {
        $pendingReason = 'native process enumeration incomplete'
        $pendingObservationError = $_
    }
    if ($null -ne $pendingReason) {
        if ($null -ne $RecoveryRecord) {
            $RecoveryRecord.status = 'recoverypending'
            $RecoveryRecord.reason = $pendingReason
            if ($null -ne $pendingCommandId) { $RecoveryRecord.commandId = $pendingCommandId }
            if ($null -ne $pendingObservationError) { $RecoveryRecord.observationError = $pendingObservationError }
        }
        $diagnostic = $null
        if ($pendingObservationError -is [System.Management.Automation.ErrorRecord]) {
            $diagnostic = $pendingObservationError.Exception.ToString()
        }
        elseif ($null -ne $pendingObservationError) {
            $diagnostic = [string]$pendingObservationError
        }
        try {
            [void](Add-UatEvent -Kind 'recovery-deferred' -Name $RecoveryName -Data @{ commandId = $pendingCommandId; recovery = $RecoveryName; reason = $pendingReason; diagnostic = $diagnostic; deferredUtc = [DateTime]::UtcNow.ToString('o') })
        }
        catch {
            if ($null -ne $RecoveryRecord -and $null -eq $RecoveryRecord.evidenceError) {
                $RecoveryRecord.evidenceError = $_
            }
        }
        return [bool]$false
    }
    return [bool]$true
}

function Assert-UatDescendantProcess {
    param(
        [Parameter(Mandatory = $true)][int]$RootProcessId,
        [Parameter(Mandatory = $true)][long]$RootStartTicks,
        [Parameter(Mandatory = $true)][int]$ChildProcessId,
        [string]$ExpectedImageName
    )
    $descendants = @(Get-UatProcessDescendantIds -RootProcessId $RootProcessId -RootStartTicks $RootStartTicks)
    Assert-Uat ($descendants -contains $ChildProcessId) ('PID {0} is not a verified descendant of app PID {1}.' -f $ChildProcessId, $RootProcessId)
    $childIdentity = Get-UatProcessIdentity -ProcessId $ChildProcessId
    Assert-Uat ($childIdentity.startTicks -ge $RootStartTicks) ('Descendant PID {0} predates its app root.' -f $ChildProcessId)
    if ($ExpectedImageName) {
        Assert-Uat ([System.IO.Path]::GetFileName($childIdentity.imagePath) -ieq $ExpectedImageName) ('Unexpected descendant image for PID {0}.' -f $ChildProcessId)
    }
    return $childIdentity
}

function Get-UatOwnedWindow {
    [CmdletBinding(DefaultParameterSetName = 'ProcessIdentity')]
    param(
        [Parameter(Mandatory = $true, ParameterSetName = 'ProcessIdentity')][int]$ProcessId,
        [Parameter(Mandatory = $true, ParameterSetName = 'ProcessIdentity')][long]$StartTicks,
        [Parameter(Mandatory = $true, ParameterSetName = 'ProcessIdentity')][string]$ExpectedImagePath,
        [Parameter(Mandatory = $true, ParameterSetName = 'NativeEntry')]$ProcessEntry,
        [Parameter(Mandatory = $true, ParameterSetName = 'NativeEntry')]$CompletionBudget,
        [Parameter(ParameterSetName = 'ProcessIdentity')][long]$KnownHandle = 0,
        [Parameter(ParameterSetName = 'NativeEntry')]$KnownWindow = $null,
        [Parameter(ParameterSetName = 'ProcessIdentity')][int]$TimeoutSeconds = 30
    )
    if ($PSCmdlet.ParameterSetName -eq 'NativeEntry') {
        if (-not $ProcessEntry.nativeTracking) { throw ('Native window routing requires TrackNativeEngines for command {0}.' -f $ProcessEntry.commandId) }
        if ($null -ne $ProcessEntry.completionBudget -and -not [object]::ReferenceEquals($ProcessEntry.completionBudget, $CompletionBudget)) {
            throw ('Native window routing received a different CompletionBudget for command {0}.' -f $ProcessEntry.commandId)
        }
        if ($null -eq $ProcessEntry.completionBudget) { $ProcessEntry.completionBudget = $CompletionBudget }
        if ($null -ne $KnownWindow -and
            ($null -eq $KnownWindow.PSObject.Properties['nativeProcessEntry'] -or
             -not [object]::ReferenceEquals($KnownWindow.nativeProcessEntry, $ProcessEntry) -or
             $KnownWindow.nativeWindowRole -ne 'main')) {
            throw 'Known native HWND was not returned as a main window by the same process entry.'
        }
        $selected = $KnownWindow
        $requeryVerified = $false
        while ($true) {
            $remainingMilliseconds = Get-UatCompletionRemainingMilliseconds -CompletionBudget $CompletionBudget
            if ($remainingMilliseconds -le 0) { throw ('Timed out waiting for a native owned window for command {0}.' -f $ProcessEntry.commandId) }
            $identities = @(Get-UatNativeWindowProcessRecords -ProcessEntry $ProcessEntry)
            $allWindows = New-Object System.Collections.Generic.List[object]
            $visibleWindows = New-Object System.Collections.Generic.List[object]
            foreach ($identity in $identities) {
                foreach ($nativeWindow in @([Sdir.Installer.Uat.NativeWindowApi]::TopLevelWindows([uint32]$identity.processId, $true))) {
                    if (-not [Sdir.Installer.Uat.NativeWindowApi]::IsLiveWindow($nativeWindow.Handle)) { continue }
                    if ([Sdir.Installer.Uat.NativeWindowApi]::OwnerProcessId($nativeWindow.Handle) -ne [uint32]$identity.processId) {
                        throw ('Top-level HWND owner PID did not match its pinned native identity: PID {0}, HWND {1}.' -f $identity.processId, $nativeWindow.Handle.ToInt64())
                    }
                    $ownerHandle = [Sdir.Installer.Uat.NativeWindowApi]::OwnerWindow($nativeWindow.Handle)
                    $row = [pscustomobject]@{ window = $nativeWindow; nativeIdentity = $identity; ownerWindowHandle = $ownerHandle }
                    [void]$allWindows.Add($row)
                    if ([Sdir.Installer.Uat.NativeWindowApi]::IsVisible($nativeWindow.Handle) -and
                        $nativeWindow.Right -gt $nativeWindow.Left -and $nativeWindow.Bottom -gt $nativeWindow.Top) {
                        [void]$visibleWindows.Add($row)
                    }
                }
            }
            if ($null -ne $selected) {
                $identityMatches = @($identities | Where-Object { [object]::ReferenceEquals($_, $selected.nativeIdentity) })
                if ($identityMatches.Count -ne 1 -or [int]$selected.processId -ne [int]$selected.nativeIdentity.processId -or
                    [long]$selected.startTicks -ne [long]$selected.nativeIdentity.startTicks -or
                    -not [string]::Equals([string]$selected.imagePath, [string]$selected.nativeIdentity.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
                    throw 'Known native HWND no longer matches the same live pinned process identity.'
                }
                $knownMatches = @($allWindows | Where-Object {
                    [object]::ReferenceEquals($_.nativeIdentity, $selected.nativeIdentity) -and $_.window.Handle -eq $selected.handle
                })
                if ($knownMatches.Count -ne 1) {
                    if (-not [Sdir.Installer.Uat.NativeWindowApi]::IsLiveWindow($selected.handle)) { throw ('Known native main HWND disappeared: {0}' -f $selected.handle.ToInt64()) }
                    throw ('Known native main HWND could not be re-queried under the same identity: {0}' -f $selected.handle.ToInt64())
                }
                if ($knownMatches[0].ownerWindowHandle -ne $selected.ownerWindowHandle) { throw 'Known native HWND owner-window relation changed.' }
                if (-not $knownMatches[0].window.Visible -or $knownMatches[0].window.Minimized -or
                    $knownMatches[0].window.Right -le $knownMatches[0].window.Left -or $knownMatches[0].window.Bottom -le $knownMatches[0].window.Top) {
                    if (-not [Sdir.Installer.Uat.NativeWindowApi]::RestoreAndFocus($selected.handle, [uint32]$selected.processId)) {
                        throw ('Known native main HWND could not be restored safely: HWND {0}' -f $selected.handle.ToInt64())
                    }
                    Start-Sleep -Milliseconds ([int][Math]::Min(50, $remainingMilliseconds))
                    continue
                }
            }
            $visibleHandles = @{}
            foreach ($row in $visibleWindows) { $visibleHandles[$row.window.Handle.ToInt64()] = $true }
            foreach ($row in $visibleWindows) {
                $owner = $row.ownerWindowHandle
                if ($owner -ne [IntPtr]::Zero -and -not $visibleHandles.ContainsKey($owner.ToInt64()) -and
                    [Sdir.Installer.Uat.NativeWindowApi]::IsLiveWindow($owner) -and [Sdir.Installer.Uat.NativeWindowApi]::IsVisible($owner)) {
                    throw ('Visible native HWND has an owner outside the captured pinned window set: owner HWND {0}.' -f $owner.ToInt64())
                }
            }
            $mainWindows = @($visibleWindows | Where-Object {
                $_.ownerWindowHandle -eq [IntPtr]::Zero -or -not $visibleHandles.ContainsKey($_.ownerWindowHandle.ToInt64())
            })
            if ($mainWindows.Count -gt 1) {
                throw ('Native process entry has multiple visible main-window candidates; none was guessed for command {0}.' -f $ProcessEntry.commandId)
            }
            if ($null -eq $selected) {
                if ($mainWindows.Count -eq 1) {
                    $candidate = $mainWindows[0]
                    $selected = [pscustomobject]@{
                        handle = $candidate.window.Handle; processId = [int]$candidate.nativeIdentity.processId
                        startTicks = [long]$candidate.nativeIdentity.startTicks; imagePath = [string]$candidate.nativeIdentity.imagePath
                        window = $candidate.window; nativeProcessEntry = $ProcessEntry; nativeIdentity = $candidate.nativeIdentity
                        ownerWindowHandle = $candidate.ownerWindowHandle; nativeWindowRole = 'main'
                    }
                    if (-not $candidate.window.Visible -or $candidate.window.Minimized -or
                        $candidate.window.Right -le $candidate.window.Left -or $candidate.window.Bottom -le $candidate.window.Top) {
                        if (-not [Sdir.Installer.Uat.NativeWindowApi]::RestoreAndFocus($selected.handle, [uint32]$selected.processId)) {
                            throw ('Native main HWND could not be restored safely: HWND {0}' -f $selected.handle.ToInt64())
                        }
                    }
                    Start-Sleep -Milliseconds ([int][Math]::Min(50, $remainingMilliseconds))
                    continue
                }
                Start-Sleep -Milliseconds ([int][Math]::Min(100, $remainingMilliseconds))
                continue
            }
            $selectedMain = @($mainWindows | Where-Object {
                $_.window.Handle -eq $selected.handle -and [object]::ReferenceEquals($_.nativeIdentity, $selected.nativeIdentity)
            })
            if ($selectedMain.Count -ne 1) {
                if ($null -ne $KnownWindow -or $requeryVerified) { throw 'Known native HWND is no longer the unique main window for the same pinned identity.' }
                Start-Sleep -Milliseconds ([int][Math]::Min(100, $remainingMilliseconds))
                continue
            }
            if (-not $requeryVerified) {
                [void](Assert-UatOwnedWindow -Window $selected -ProcessEntry $ProcessEntry)
                $requeryVerified = $true
                continue
            }
            if ((Get-UatCompletionRemainingMilliseconds -CompletionBudget $CompletionBudget) -le 0) {
                throw ('CompletionBudget expired while routing native window for command {0}.' -f $ProcessEntry.commandId)
            }
            return $selected
        }
    }
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $handle = if ($KnownHandle -ne 0) { [System.IntPtr]::new($KnownHandle) } else { [System.IntPtr]::Zero }
    while ([DateTime]::UtcNow -lt $deadline) {
        [void](Assert-UatProcessIdentity -ProcessId $ProcessId -ExpectedStartTicks $StartTicks -ExpectedImagePath $ExpectedImagePath)
        if ($handle -ne [System.IntPtr]::Zero) {
            $matches = @([Sdir.Installer.Uat.NativeWindowApi]::TopLevelWindows([uint32]$ProcessId, $true) | Where-Object { $_.Handle -eq $handle })
            if ($matches.Count -eq 1) {
                if (-not $matches[0].Visible -or $matches[0].Minimized) {
                    if (-not [Sdir.Installer.Uat.NativeWindowApi]::RestoreAndFocus($handle, [uint32]$ProcessId)) {
                        throw ('Known owned window could not be restored safely: HWND {0}' -f $KnownHandle)
                    }
                }
                $matches = @([Sdir.Installer.Uat.NativeWindowApi]::TopLevelWindows([uint32]$ProcessId, $true) | Where-Object { $_.Handle -eq $handle })
                if ($matches.Count -ne 1 -or -not $matches[0].Visible -or $matches[0].Minimized) {
                    Start-Sleep -Milliseconds 50
                    continue
                }
                [void](Assert-UatProcessIdentity -ProcessId $ProcessId -ExpectedStartTicks $StartTicks -ExpectedImagePath $ExpectedImagePath)
                return [pscustomobject]@{ handle = $handle; processId = $ProcessId; startTicks = $StartTicks; imagePath = [System.IO.Path]::GetFullPath($ExpectedImagePath); window = $matches[0] }
            }
            if (-not [Sdir.Installer.Uat.NativeWindowApi]::IsLiveWindow($handle)) { throw ('Known owned main HWND disappeared: {0}' -f $KnownHandle) }
        }
        else {
            $windows = @([Sdir.Installer.Uat.NativeWindowApi]::TopLevelWindows([uint32]$ProcessId, $false) |
                Where-Object { $_.Right -gt $_.Left -and $_.Bottom -gt $_.Top })
            if ($windows.Count -eq 1) {
                $handle = $windows[0].Handle
                return [pscustomobject]@{ handle = $handle; processId = $ProcessId; startTicks = $StartTicks; imagePath = [System.IO.Path]::GetFullPath($ExpectedImagePath); window = $windows[0] }
            }
            if ($windows.Count -gt 1) {
                throw ('Process {0} has multiple visible top-level windows before its main HWND was identified; none was guessed.' -f $ProcessId)
            }
        }
        Start-Sleep -Milliseconds 200
    }
    throw ('Timed out waiting for a visible owned window for PID {0}.' -f $ProcessId)
}

function Assert-UatOwnedWindow {
    [CmdletBinding(DefaultParameterSetName = 'Window')]
    param(
        [Parameter(Mandatory = $true, ParameterSetName = 'Window')]
        [Parameter(Mandatory = $true, ParameterSetName = 'NativeEntry')]$Window,
        [Parameter(Mandatory = $true, ParameterSetName = 'NativeEntry')]$ProcessEntry
    )
    $nativeEntry = $null
    if ($PSCmdlet.ParameterSetName -eq 'NativeEntry') { $nativeEntry = $ProcessEntry }
    elseif ($null -ne $Window.PSObject.Properties['nativeProcessEntry']) { $nativeEntry = $Window.nativeProcessEntry }
    if ($null -ne $nativeEntry) {
        if (-not $nativeEntry.nativeTracking -or -not [object]::ReferenceEquals($Window.nativeProcessEntry, $nativeEntry)) {
            throw 'Native window is not bound to its tracked process entry.'
        }
        if ($null -eq $nativeEntry.completionBudget -or (Get-UatCompletionRemainingMilliseconds -CompletionBudget $nativeEntry.completionBudget) -le 0) {
            throw 'Native window operation has no remaining time in its original CompletionBudget.'
        }
        if ($null -eq $Window.nativeIdentity) { throw 'Native window has no selected pinned process identity.' }
        $identities = @(Get-UatNativeWindowProcessRecords -ProcessEntry $nativeEntry)
        $identityMatches = @($identities | Where-Object { [object]::ReferenceEquals($_, $Window.nativeIdentity) })
        Assert-Uat ($identityMatches.Count -eq 1) ('Native HWND identity is not a live pinned launcher/engine candidate: {0}' -f $Window.handle.ToInt64())
        Assert-Uat ([int]$Window.processId -eq [int]$Window.nativeIdentity.processId -and
            [long]$Window.startTicks -eq [long]$Window.nativeIdentity.startTicks -and
            [string]::Equals([string]$Window.imagePath, [string]$Window.nativeIdentity.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) 'Native HWND process PID/startTicks/image did not match its selected engine identity.'
        Assert-Uat ([Sdir.Installer.Uat.NativeWindowApi]::IsLiveWindow($Window.handle)) ('Owned native HWND is no longer valid: {0}' -f $Window.handle.ToInt64())
        Assert-Uat ([Sdir.Installer.Uat.NativeWindowApi]::OwnerProcessId($Window.handle) -eq [uint32]$Window.processId) ('Native HWND owner changed: {0}' -f $Window.handle.ToInt64())
        if ($null -ne $Window.PSObject.Properties['ownerWindowHandle'] -and
            [Sdir.Installer.Uat.NativeWindowApi]::OwnerWindow($Window.handle) -ne $Window.ownerWindowHandle) {
            throw ('Native HWND owner-window relation changed: {0}' -f $Window.handle.ToInt64())
        }
        if (-not [Sdir.Installer.Uat.NativeWindowApi]::IsVisible($Window.handle) -or [Sdir.Installer.Uat.NativeWindowApi]::IsMinimized($Window.handle)) {
            if (-not [Sdir.Installer.Uat.NativeWindowApi]::RestoreAndFocus($Window.handle, [uint32]$Window.processId)) {
                throw 'Known native owned window could not be restored before a UI operation.'
            }
        }
        $fresh = @([Sdir.Installer.Uat.NativeWindowApi]::TopLevelWindows([uint32]$Window.processId, $true) | Where-Object { $_.Handle -eq $Window.handle })
        Assert-Uat ($fresh.Count -eq 1 -and $fresh[0].Visible -and -not $fresh[0].Minimized -and
            $fresh[0].Right -gt $fresh[0].Left -and $fresh[0].Bottom -gt $fresh[0].Top) 'Native HWND did not re-query as visible, restored, and sized after the native request.'
        Assert-Uat ([Sdir.Installer.Uat.NativeWindowApi]::OwnerProcessId($Window.handle) -eq [uint32]$Window.processId) 'Native HWND owner PID changed after restoration.'
        if ($null -ne $Window.PSObject.Properties['ownerWindowHandle'] -and
            [Sdir.Installer.Uat.NativeWindowApi]::OwnerWindow($Window.handle) -ne $Window.ownerWindowHandle) {
            throw 'Native HWND owner-window relation changed after restoration.'
        }
        $confirmed = @(Get-UatNativeWindowProcessRecords -ProcessEntry $nativeEntry | Where-Object { [object]::ReferenceEquals($_, $Window.nativeIdentity) })
        Assert-Uat ($confirmed.Count -eq 1) 'Native HWND process identity changed after restoration and re-query.'
        $Window.window = $fresh[0]
        return
    }
    [void](Assert-UatProcessIdentity -ProcessId $Window.processId -ExpectedStartTicks $Window.startTicks -ExpectedImagePath $Window.imagePath)
    Assert-Uat ([Sdir.Installer.Uat.NativeWindowApi]::IsLiveWindow($Window.handle)) ('Owned HWND is no longer valid: {0}' -f $Window.handle.ToInt64())
    Assert-Uat ([Sdir.Installer.Uat.NativeWindowApi]::OwnerProcessId($Window.handle) -eq [uint32]$Window.processId) ('HWND owner changed: {0}' -f $Window.handle.ToInt64())
    if (-not [Sdir.Installer.Uat.NativeWindowApi]::IsVisible($Window.handle) -or [Sdir.Installer.Uat.NativeWindowApi]::IsMinimized($Window.handle)) {
        if (-not [Sdir.Installer.Uat.NativeWindowApi]::RestoreAndFocus($Window.handle, [uint32]$Window.processId)) {
            throw 'Known owned main window could not be restored before a UI/CDP operation.'
        }
    }
    $fresh = @([Sdir.Installer.Uat.NativeWindowApi]::TopLevelWindows([uint32]$Window.processId, $true) | Where-Object { $_.Handle -eq $Window.handle })
    Assert-Uat ($fresh.Count -eq 1 -and $fresh[0].Visible -and -not $fresh[0].Minimized) 'Owned HWND did not become visible and restored after the native request.'
    [void](Assert-UatProcessIdentity -ProcessId $Window.processId -ExpectedStartTicks $Window.startTicks -ExpectedImagePath $Window.imagePath)
    $Window.window = $fresh[0]
}

function Save-UatWindowEvidence {
    param([Parameter(Mandatory = $true)]$Window, [Parameter(Mandatory = $true)][string]$Name)
    Assert-UatOwnedWindow -Window $Window
    $safeName = $Name -replace '[^A-Za-z0-9._-]', '-'
    $path = Join-Path $providedEvidencePath ($safeName + '.png')
    $captured = Capture-UatOwnedWindow -WindowHandle $Window.handle -ProcessId ([uint32]$Window.processId) -Path $path
    Add-UatEvent -Kind 'owned-window-captured' -Name $Name -Data @{ processId = $Window.processId; processStartTicks = $Window.startTicks; imagePath = $Window.imagePath; windowHandle = $Window.handle.ToInt64(); evidence = $captured }
    return $captured
}

function Get-UatWindowSummary {
    param([Parameter(Mandatory = $true)]$Window)
    Assert-UatOwnedWindow -Window $Window
    return @(Get-UatAutomationSummary -WindowHandle $Window.handle -ProcessId ([uint32]$Window.processId))
}

function Get-UatWindowSummaryText {
    param([Parameter(Mandatory = $true)]$Window)
    $summary = @(Get-UatWindowSummary -Window $Window)
    return (@($summary | ForEach-Object { [string]$_.name }) -join "`n")
}

function Get-UatInnoWizardPhase {
    param([object[]]$Summary)
    $headings = @($Summary | Where-Object {
        -not $_.offscreen -and $_.controlType -cin @('ControlType.Pane', 'ControlType.Text')
    } | ForEach-Object { ([string]$_.name) -creplace '\r?\n\z', '' })
    $phases = @(
        if (@($headings | Where-Object { $_ -cin @('추가 작업 선택', 'Select Additional Tasks') }).Count) { 'Options' }
        if (@($headings | Where-Object { $_ -cin @('설치 준비 완료', 'Ready to Install') }).Count) { 'Ready' }
        if (@($headings | Where-Object { $_ -cin @('설치 중', 'Installing') }).Count) { 'Installing' }
        if (@($headings | Where-Object { $_ -cmatch '^(?:SDIR(?: UAT)? 설치 마법사 완료|Completing the SDIR(?: UAT)? Setup Wizard)\z' }).Count) { 'Finish' }
    )
    if ($phases.Count -gt 1) { throw 'The owned Inno wizard exposed conflicting page headings.' }
    if ($phases.Count -eq 1) { return $phases[0] }
    return 'Navigation'
}

function Select-UatOwnedInnoButton {
    param([object[]]$Summary, [object[]]$NativeButtons, [uint32]$ProcessId, [string]$Role, [string]$Phase)

    $allowedPhases = @{
        Next = @('Navigation', 'Options')
        Back = @('Navigation', 'Options', 'Ready')
        Install = @('Ready')
        Finish = @('Finish')
        Cancel = @('Navigation', 'Options', 'Ready', 'Installing')
    }
    if (-not $allowedPhases.ContainsKey($Role) -or $Phase -cnotin $allowedPhases[$Role]) { return }

    $named = @($Summary | Where-Object {
        $_.controlType -cin @('ControlType.Button', 'ControlType.Pane') -and
        [Sdir.Installer.Uat.NativeWindowApi]::IsInnoButtonCaption($Role, $_.name, $false)
    })
    if ($named.Count -ne 1 -or $NativeButtons.Count -ne 1) { return }
    $target = $named[0]
    $native = $NativeButtons[0]
    if (-not $target.enabled -or $target.offscreen -or $target.nativeHandle -eq 0 -or
        $target.nativeHandle -ne $native.handle -or $native.processId -ne $ProcessId -or
        $native.className -cne 'TNewButton' -or
        -not [Sdir.Installer.Uat.NativeWindowApi]::IsInnoButtonCaption($Role, $native.title, $true) -or
        -not $native.isDescendant -or -not $native.visible -or -not $native.enabled -or
        $native.right -le $native.left -or $native.bottom -le $native.top) { return }
    return [pscustomobject]@{
        name = [string]$target.name; controlType = $target.controlType; nativeHandle = $target.nativeHandle
        nativeCaption = $native.title; innoRole = $Role; innoPhase = $Phase
    }
}

function Find-UatWindowButton {
    param([Parameter(Mandatory = $true)]$Window, [Parameter(Mandatory = $true)][string]$NamePattern,
        [ValidateSet('Next', 'Back', 'Install', 'Finish', 'Cancel')][string]$InnoRole)
    $summary = @(Get-UatWindowSummary -Window $Window)
    if (-not $InnoRole) {
        return @($summary | Where-Object { $_.controlType -eq 'ControlType.Button' -and $_.enabled -and -not $_.offscreen -and $_.name -match $NamePattern })
    }
    if ($null -eq $Window.PSObject.Properties['nativeProcessEntry'] -or $Window.nativeWindowRole -cne 'main' -or
        $Window.window.ClassName -cne 'TWizardForm') { return }

    $phase = Get-UatInnoWizardPhase -Summary $summary
    $pidExpected = [uint32]$Window.processId
    $nativeButtons = @(
        foreach ($child in [Sdir.Installer.Uat.NativeWindowApi]::OwnedInnoButtons($Window.handle, $pidExpected, $InnoRole)) {
            [pscustomobject]@{
                className = $child.ClassName; title = $child.Title
                handle = $child.Handle.ToInt64(); processId = $child.ProcessId
                isDescendant = [Sdir.Installer.Uat.NativeWindowApi]::IsDescendant($Window.handle, $child.Handle)
                visible = $child.Visible; enabled = [Sdir.Installer.Uat.NativeWindowApi]::IsEnabled($child.Handle)
                left = $child.Left; top = $child.Top; right = $child.Right; bottom = $child.Bottom
            }
        }
    )
    return Select-UatOwnedInnoButton -Summary $summary -NativeButtons $nativeButtons -ProcessId $pidExpected -Role $InnoRole -Phase $phase
}

function Invoke-UatWindowButton {
    param(
        [Parameter(Mandatory = $true)]$Window,
        [Parameter(Mandatory = $true)][string]$NamePattern,
        [switch]$AsyncPostMessage,
        [ValidateSet('Next', 'Back', 'Install', 'Finish', 'Cancel')][string]$InnoRole,
        $ExpectedInnoButton
    )
    Assert-UatOwnedWindow -Window $Window
    if ($InnoRole) {
        $fresh = @(Find-UatWindowButton -Window $Window -NamePattern $NamePattern -InnoRole $InnoRole)
        if ($fresh.Count -ne 1 -or ($null -ne $ExpectedInnoButton -and
            ($fresh[0].nativeHandle -ne $ExpectedInnoButton.nativeHandle -or $fresh[0].name -cne $ExpectedInnoButton.name -or
             $fresh[0].controlType -cne $ExpectedInnoButton.controlType -or $fresh[0].nativeCaption -cne $ExpectedInnoButton.nativeCaption -or
             $fresh[0].innoRole -cne $ExpectedInnoButton.innoRole -or $fresh[0].innoPhase -cne $ExpectedInnoButton.innoPhase))) {
            throw ('The unique owned Inno {0} button or its page changed before input.' -f $InnoRole)
        }
        $handle = [System.IntPtr]::new([long]$fresh[0].nativeHandle)
        if ($fresh[0].controlType -ceq 'ControlType.Pane' -or $AsyncPostMessage) {
            if (-not [Sdir.Installer.Uat.NativeWindowApi]::PostOwnedInnoButtonClick($Window.handle, $handle, [uint32]$Window.processId, $InnoRole, $fresh[0].nativeCaption)) {
                throw ('The owned Inno {0} button failed native revalidation or input queuing.' -f $InnoRole)
            }
            $action = [pscustomobject]@{ name = $fresh[0].name; mechanism = 'PostMessage(WM_LBUTTONDOWN/UP)'; nativeHandle = $handle.ToInt64() }
        }
        else {
            $exactName = '^' + [regex]::Escape($fresh[0].name) + '$'
            $action = Invoke-UatAutomationButton -WindowHandle $Window.handle -ProcessId ([uint32]$Window.processId) -NamePattern $exactName -ExpectedNativeHandle $handle
        }
    }
    else {
        $action = Invoke-UatAutomationButton -WindowHandle $Window.handle -ProcessId ([uint32]$Window.processId) -NamePattern $NamePattern -AsyncPostMessage:$AsyncPostMessage
    }
    Add-UatEvent -Kind 'native-ui-input' -Name 'button' -Data @{ processId = $Window.processId; processStartTicks = $Window.startTicks; imagePath = $Window.imagePath; windowHandle = $Window.handle.ToInt64(); target = $action.name; mechanism = $action.mechanism; targetHandle = $action.nativeHandle }
    return $action
}

function Get-UatOwnedInnoDesktopOption {
    param([Parameter(Mandatory = $true)]$Window)
    $summary = @(Get-UatWindowSummary -Window $Window)
    if ((Get-UatInnoWizardPhase -Summary $summary) -cne 'Options') { return }
    if ($null -eq $Window.PSObject.Properties['nativeProcessEntry'] -or $Window.nativeWindowRole -cne 'main' -or
        $Window.window.ClassName -cne 'TWizardForm') { throw 'The desktop task page is not an owned native Inno wizard.' }
    $item = [Sdir.Installer.Uat.NativeWindowApi]::ReadOwnedInnoCheckItem($Window.handle, [uint32]$Window.processId, '바탕 화면 바로 가기 추가')
    if ($null -eq $item) { throw 'The Options page did not expose the exact owned native desktop shortcut checkbox.' }
    return $item
}

function Set-UatWindowCheckBox {
    param([Parameter(Mandatory = $true)]$Window, [Parameter(Mandatory = $true)][string]$NamePattern, [Parameter(Mandatory = $true)][bool]$Checked)
    Assert-UatOwnedWindow -Window $Window
    if ($NamePattern -ceq '바탕 화면 바로 가기 추가') {
        $before = Get-UatOwnedInnoDesktopOption -Window $Window
        if ($null -eq $before) { throw 'The desktop shortcut task must be read on the owned Inno Options page.' }
        $after = $before
        $mechanism = 'MSAA.IAccessible'
        $selected = $null
        if ($before.Checked -ne $Checked) {
            $selected = [Sdir.Installer.Uat.NativeWindowApi]::PostOwnedInnoCheckItemSpace($Window.handle, [uint32]$Window.processId, $before)
            $mechanism = 'posted-control-input: WM_KEYDOWN/WM_KEYUP(VK_SPACE)'
            $deadline = [DateTime]::UtcNow.AddSeconds(1)
            do {
                $after = Get-UatOwnedInnoDesktopOption -Window $Window
                if (-not [Sdir.Installer.Uat.NativeWindowApi]::SameInnoCheckItem($before, $after)) {
                    throw 'The exact owned MSAA checkbox changed after Space.'
                }
                if ($after.Checked -eq $Checked) { break }
                $remaining = Get-UatCompletionRemainingMilliseconds -CompletionBudget $Window.nativeProcessEntry.completionBudget
                if ($remaining -le 0) { break }
                Start-Sleep -Milliseconds ([int][Math]::Min(25, $remaining))
            } while ([DateTime]::UtcNow -lt $deadline)
        }
        if ($after.Checked -ne $Checked) { throw 'The owned desktop shortcut checkbox did not reach the requested state.' }
        $action = [pscustomobject]@{
            name = $after.Name; checked = $after.Checked; mechanism = $mechanism; role = $after.Role
            nativeHandle = $after.Handle.ToInt64(); childId = $after.ChildId
            beforeState = $before.State; selectedState = if ($null -ne $selected) { $selected.State } else { $null }; afterState = $after.State
            left = $after.Left; top = $after.Top; width = $after.Width; height = $after.Height
        }
    }
    else {
        $action = Set-UatAutomationCheckBox -WindowHandle $Window.handle -ProcessId ([uint32]$Window.processId) -NamePattern $NamePattern -Checked $Checked
    }
    Add-UatEvent -Kind 'native-ui-input' -Name 'checkbox' -Data @{ processId = $Window.processId; processStartTicks = $Window.startTicks; imagePath = $Window.imagePath; windowHandle = $Window.handle.ToInt64(); target = $action.name; checked = $action.checked; mechanism = $action.mechanism; observation = $action }
    return $action
}

function Wait-UatWindowButton {
    param([Parameter(Mandatory = $true)]$Window, [Parameter(Mandatory = $true)][string]$NamePattern, [int]$TimeoutSeconds = 15, $CompletionBudget = $null,
        [ValidateSet('Next', 'Back', 'Install', 'Finish', 'Cancel')][string]$InnoRole)
    $buttonArguments = @{ Window = $Window; NamePattern = $NamePattern }
    if ($InnoRole) { $buttonArguments['InnoRole'] = $InnoRole }
    if ($null -ne $Window.PSObject.Properties['nativeProcessEntry']) {
        if ($null -eq $CompletionBudget) { $CompletionBudget = $Window.nativeProcessEntry.completionBudget }
        if (-not [object]::ReferenceEquals($CompletionBudget, $Window.nativeProcessEntry.completionBudget)) {
            throw 'Native button wait received a different CompletionBudget from its process entry.'
        }
    }
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $remainingMilliseconds = $null
        if ($null -ne $CompletionBudget) {
            $remainingMilliseconds = Get-UatCompletionRemainingMilliseconds -CompletionBudget $CompletionBudget
            if ($remainingMilliseconds -le 0) { throw ('Timed out waiting for native button /{0}/; the original CompletionBudget expired.' -f $NamePattern) }
        }
        $buttons = @(Find-UatWindowButton @buttonArguments)
        if ($buttons.Count -eq 1) { return $buttons[0] }
        if ($buttons.Count -gt 1) { throw ('Owned UI has multiple enabled buttons matching /{0}/.' -f $NamePattern) }
        $sleepMilliseconds = 150
        if ($null -ne $remainingMilliseconds) { $sleepMilliseconds = [int][Math]::Min($sleepMilliseconds, $remainingMilliseconds) }
        Start-Sleep -Milliseconds $sleepMilliseconds
    }
    throw ('Timed out waiting for an owned enabled button matching /{0}/.' -f $NamePattern)
}

function Get-UatOwnedModalWindow {
    [CmdletBinding(DefaultParameterSetName = 'Window')]
    param(
        [Parameter(Mandatory = $true, ParameterSetName = 'Window')]
        [Parameter(Mandatory = $true, ParameterSetName = 'NativeEntry')]$MainWindow,
        [Parameter(Mandatory = $true, ParameterSetName = 'NativeEntry')]$ProcessEntry,
        [int]$TimeoutSeconds = 5,
        $CompletionBudget = $null
    )
    $nativeEntry = $null
    if ($PSCmdlet.ParameterSetName -eq 'NativeEntry') { $nativeEntry = $ProcessEntry }
    elseif ($null -ne $MainWindow.PSObject.Properties['nativeProcessEntry']) { $nativeEntry = $MainWindow.nativeProcessEntry }
    if ($null -ne $nativeEntry) {
        if (-not [object]::ReferenceEquals($MainWindow.nativeProcessEntry, $nativeEntry)) { throw 'Native main window belongs to a different process entry.' }
        if ($null -eq $CompletionBudget) { $CompletionBudget = $nativeEntry.completionBudget }
        if ($null -eq $CompletionBudget -or -not [object]::ReferenceEquals($CompletionBudget, $nativeEntry.completionBudget)) {
            throw 'Native modal lookup has no matching original CompletionBudget.'
        }
        $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
        while ([DateTime]::UtcNow -lt $deadline) {
            $remainingMilliseconds = Get-UatCompletionRemainingMilliseconds -CompletionBudget $CompletionBudget
            if ($remainingMilliseconds -le 0) { return $null }
            [void](Assert-UatOwnedWindow -Window $MainWindow -ProcessEntry $nativeEntry)
            $identityMatches = @(Get-UatNativeWindowProcessRecords -ProcessEntry $nativeEntry | Where-Object { [object]::ReferenceEquals($_, $MainWindow.nativeIdentity) })
            if ($identityMatches.Count -ne 1) { throw 'Native modal owner identity is no longer a live pinned process.' }
            $modals = @([Sdir.Installer.Uat.NativeWindowApi]::TopLevelWindows([uint32]$MainWindow.processId, $false) |
                Where-Object {
                    $_.Handle -ne $MainWindow.handle -and $_.Visible -and -not $_.Minimized -and
                    $_.Right -gt $_.Left -and $_.Bottom -gt $_.Top -and
                    [Sdir.Installer.Uat.NativeWindowApi]::OwnerProcessId($_.Handle) -eq [uint32]$MainWindow.processId -and
                    [Sdir.Installer.Uat.NativeWindowApi]::OwnerWindow($_.Handle) -eq $MainWindow.handle
                })
            if ($modals.Count -eq 1) {
                $modal = [pscustomobject]@{
                    handle = $modals[0].Handle; processId = $MainWindow.processId; startTicks = $MainWindow.startTicks
                    imagePath = $MainWindow.imagePath; window = $modals[0]; nativeProcessEntry = $nativeEntry
                    nativeIdentity = $MainWindow.nativeIdentity; ownerWindowHandle = $MainWindow.handle; nativeWindowRole = 'modal'
                }
                [void](Assert-UatOwnedWindow -Window $modal -ProcessEntry $nativeEntry)
                return $modal
            }
            if ($modals.Count -gt 1) { throw 'Multiple native modal windows match the selected engine PID and exact owner HWND; none was guessed.' }
            Start-Sleep -Milliseconds ([int][Math]::Min(150, $remainingMilliseconds))
        }
        return $null
    }
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        [void](Assert-UatProcessIdentity -ProcessId $MainWindow.processId -ExpectedStartTicks $MainWindow.startTicks -ExpectedImagePath $MainWindow.imagePath)
        $modals = @([Sdir.Installer.Uat.NativeWindowApi]::TopLevelWindows([uint32]$MainWindow.processId, $false) |
            Where-Object { $_.Handle -ne $MainWindow.handle -and [Sdir.Installer.Uat.NativeWindowApi]::OwnerWindow($_.Handle) -eq $MainWindow.handle })
        if ($modals.Count -eq 1) {
            $modal = [pscustomobject]@{ handle = $modals[0].Handle; processId = $MainWindow.processId; startTicks = $MainWindow.startTicks; imagePath = $MainWindow.imagePath; window = $modals[0] }
            Assert-UatOwnedWindow -Window $modal
            return $modal
        }
        if ($modals.Count -gt 1) { throw 'Multiple owned modal windows are open; no caption-based or arbitrary window selection was made.' }
        Start-Sleep -Milliseconds 150
    }
    return $null
}

function Wait-UatWindowSummaryChange {
    param([Parameter(Mandatory = $true)]$Window, [Parameter(Mandatory = $true)][string]$Before, [int]$TimeoutSeconds = 10)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $remainingMilliseconds = $null
        if ($null -ne $Window.PSObject.Properties['nativeProcessEntry']) {
            $budget = $Window.nativeProcessEntry.completionBudget
            if ($null -eq $budget) { throw 'Native window summary wait has no original CompletionBudget.' }
            $remainingMilliseconds = Get-UatCompletionRemainingMilliseconds -CompletionBudget $budget
            if ($remainingMilliseconds -le 0) { throw 'Native window summary wait exhausted its original CompletionBudget.' }
        }
        $after = ConvertTo-Json -InputObject (Get-UatWindowSummary -Window $Window) -Depth 12 -Compress
        if ($after -cne $Before) { return $after }
        $sleepMilliseconds = 150
        if ($null -ne $remainingMilliseconds) { $sleepMilliseconds = [int][Math]::Min($sleepMilliseconds, $remainingMilliseconds) }
        Start-Sleep -Milliseconds $sleepMilliseconds
    }
    throw 'The expected native wizard page transition did not change the owned UI Automation tree.'
}

function Start-UatSetupGui {
    param([Parameter(Mandatory = $true)]$Candidate, [string]$Name = 'setup-gui', [string[]]$AdditionalArgument = @(), [int]$TimeoutSeconds = 30, $CompletionBudget = $null)
    $logPath = Join-Path $providedEvidencePath ($Name + '.setup.log')
    $started = [DateTime]::UtcNow
    $budget = $CompletionBudget
    if ($null -eq $budget) {
        $budget = New-UatCompletionBudget -TotalMilliseconds ([long]$TimeoutSeconds * [long]1000) -Origin ($Name + ' setup GUI launch and window')
    }
    $process = Start-UatProcess -Executable $Candidate.path -Argument (@('/LOG=' + $logPath) + @($AdditionalArgument)) -WorkingDirectory $repoRoot -Name $Name -TrackNativeEngines -NativeAppRoot $Candidate.installRoot -CompletionBudget $budget
    $window = Get-UatOwnedWindow -ProcessEntry $process -CompletionBudget $budget
    [void](Save-UatWindowEvidence -Window $window -Name ($Name + '-welcome'))
    return [pscustomobject]@{ process = $process; window = $window; logPath = $logPath; startedUtc = $started; candidate = $Candidate }
}

function Start-UatSetupSilent {
    param(
        [Parameter(Mandatory = $true)]$Candidate,
        [Parameter(Mandatory = $true)][string]$Name,
        [string[]]$ExtraArgument = @(),
        [int]$TimeoutSeconds = 240
    )
    $logPath = Join-Path $providedEvidencePath ($Name + '.setup.log')
    $arguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', ('/LOG=' + $logPath)) + @($ExtraArgument)
    $budget = New-UatCompletionBudget -TotalMilliseconds ([long]$TimeoutSeconds * [long]1000) -Origin ($Name + ' silent setup')
    $result = Invoke-UatCommand -Executable $Candidate.path -Argument $arguments -WorkingDirectory $repoRoot -Name $Name -TimeoutSeconds $TimeoutSeconds -TrackNativeEngines -NativeAppRoot $Candidate.installRoot -CompletionBudget $budget
    $innoLog = if (Test-Path -LiteralPath $logPath -PathType Leaf) { Get-UatTreeSnapshot -Path $logPath } else { $null }
    Add-UatEvent -Kind 'installer-result' -Name $Name -Data @{ exitCode = $result.exitCode; expectedRange = 'asserted by caller'; processEvidence = $result.evidence; innoLogPath = $logPath; innoLogSnapshot = $innoLog }
    return [pscustomobject]@{ result = $result; logPath = $logPath }
}

function Get-UatTestShortcutTarget {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw ('Required test shortcut is missing: {0}' -f $Path) }
    $shell = New-Object -ComObject WScript.Shell
    try {
        $shortcut = $shell.CreateShortcut($Path)
        return [pscustomobject]@{ targetPath = [System.IO.Path]::GetFullPath([string]$shortcut.TargetPath); workingDirectory = [System.IO.Path]::GetFullPath([string]$shortcut.WorkingDirectory); arguments = [string]$shortcut.Arguments }
    }
    finally { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell) }
}

function Get-UatShortcutProcessStart {
    param([Parameter(Mandatory = $true)][string]$ShortcutPath, [Parameter(Mandatory = $true)][string]$ExpectedTarget, [Parameter(Mandatory = $true)][DateTime]$NotBeforeUtc, [int]$TimeoutSeconds = 30)
    $shortcut = Get-UatTestShortcutTarget -Path $ShortcutPath
    Assert-Uat ([string]::Equals($shortcut.targetPath, [System.IO.Path]::GetFullPath($ExpectedTarget), [System.StringComparison]::OrdinalIgnoreCase)) ('Shortcut target did not point at the isolated test launcher: {0}' -f $ShortcutPath)
    Assert-Uat ([string]::Equals($shortcut.workingDirectory, [System.IO.Path]::GetFullPath($script:testRoot), [System.StringComparison]::OrdinalIgnoreCase)) ('Shortcut working directory did not point at the test root: {0}' -f $ShortcutPath)
    $shell = New-Object -ComObject Shell.Application
    try {
        $folder = $shell.Namespace((Split-Path -Parent $ShortcutPath))
        if ($null -eq $folder) { throw ('Explorer shell namespace could not open owned test shortcut folder: {0}' -f $ShortcutPath) }
        $item = $folder.ParseName((Split-Path -Leaf $ShortcutPath))
        if ($null -eq $item) { throw ('Explorer shell did not resolve the owned shortcut: {0}' -f $ShortcutPath) }
        $item.InvokeVerb('open')
    }
    finally { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell) }
    return Wait-UatAppProcess -ImagePath (Join-Path $script:testRoot 'SDIR.exe') -NotBeforeUtc $NotBeforeUtc
}

function Get-UatCdpSessionForApp {
    param([Parameter(Mandatory = $true)]$App, [int]$TimeoutSeconds = 25)
    $expectedImage = Join-Path $App.installRoot 'SDIR.exe'
    $window = Get-UatOwnedWindow -ProcessId $App.processId -StartTicks $App.startTicks -ExpectedImagePath $expectedImage -KnownHandle $App.window.handle.ToInt64()
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $listenerPid = 0
    $target = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        Assert-UatOwnedWindow -Window $window
        $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $script:debugPort -ErrorAction SilentlyContinue)
        $owners = @($listeners | Select-Object -ExpandProperty OwningProcess -Unique | ForEach-Object { [int]$_ })
        if ($owners.Count -eq 1) {
            $candidatePid = $owners[0]
            if ($candidatePid -eq $App.processId) {
                [void](Assert-UatProcessIdentity -ProcessId $candidatePid -ExpectedStartTicks $App.startTicks -ExpectedImagePath $expectedImage)
                $listenerPid = $candidatePid
            }
            else {
                [void](Assert-UatDescendantProcess -RootProcessId $App.processId -RootStartTicks $App.startTicks -ChildProcessId $candidatePid -ExpectedImageName 'msedgewebview2.exe')
                $listenerPid = $candidatePid
            }
            try { $target = Get-UatCdpTarget -Port $script:debugPort -ProcessId $listenerPid }
            catch { $target = $null }
            if ($target) { break }
        }
        elseif ($owners.Count -gt 1) {
            throw ('The debug port is owned by multiple PIDs; no DevTools target was selected: {0}' -f ($owners -join ','))
        }
        Start-Sleep -Milliseconds 200
    }
    if (-not $target) { throw ('No DevTools target was exposed by the expected app process tree on port {0}.' -f $script:debugPort) }
    Assert-Uat ($target.title -match 'SDIR') ('DevTools page title did not identify SDIR: {0}' -f $target.title)
    $session = New-UatCdpSocket -WebSocketUrl $target.webSocketDebuggerUrl -OwnerWindow $window -ListenerProcessId $listenerPid -RootProcessId $App.processId -RootStartTicks $App.startTicks
    [void](Invoke-UatCdpCommand -Session $session -Method 'Runtime.enable')
    [void](Invoke-UatCdpCommand -Session $session -Method 'Page.enable')
    Add-UatEvent -Kind 'cdp-session' -Name 'owned-app' -Data @{ appPid = $App.processId; appStartTicks = $App.startTicks; imagePath = $expectedImage; windowHandle = $window.handle.ToInt64(); listenerPid = $listenerPid; targetTitle = $target.title; targetUrl = $target.url }
    return [pscustomobject]@{ session = $session; window = $window; target = $target; listenerPid = $listenerPid }
}

function Get-UatAppSemanticState {
    param([Parameter(Mandatory = $true)]$Session)
    $expression = @'
(() => {
  const app = document.querySelector('.app');
  const panes = Array.from(document.querySelectorAll('[data-pane]')).map(p => ({
    id: p.getAttribute('data-pane'),
    current: p.getAttribute('aria-current') === 'true',
    name: p.querySelector('.group-title')?.textContent?.trim() || '',
    view: p.getAttribute('data-view') || '',
    address: p.querySelector('.address-input')?.value || '',
    tabs: Array.from(p.querySelectorAll('.tab')).map(t => ({
      title: t.title,
      text: t.textContent?.trim() || '',
      selected: t.classList.contains('selected')
    })),
    rows: Array.from(p.querySelectorAll('.list-scroll [data-drag-key]')).map(r => r.getAttribute('data-drag-key'))
  }));
  return { theme: app?.getAttribute('data-theme') || '', panes };
})()
'@
    return Invoke-UatCdpEvaluate -Session $Session -Expression $expression
}

function Assert-UatAppSemanticState {
    param([Parameter(Mandatory = $true)]$Session, [Parameter(Mandatory = $true)][string]$Name)
    $deadline = [DateTime]::UtcNow.AddSeconds(25)
    $state = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        $state = Get-UatAppSemanticState -Session $Session
        $allRows = @($state.panes | ForEach-Object { $_.rows })
        $rowText = $allRows -join "`n"
        if ($state.panes.Count -ge 2 -and $rowText.Contains('nested.txt') -and $rowText.Contains('gamma.txt')) { break }
        Start-Sleep -Milliseconds 180
    }
    Assert-Uat ($null -ne $state) ('No semantic app state was returned for {0}.' -f $Name)
    Assert-Uat ($state.theme -eq $script:appSemanticExpected.theme) ('Theme did not restore in the visible app UI for {0}.' -f $Name)
    Assert-Uat ($state.panes.Count -eq 2) ('The visible app did not restore the two-pane layout for {0}.' -f $Name)
    Assert-Uat ($state.panes[0].name -eq $script:appSemanticExpected.groupNames[0] -and $state.panes[1].name -eq $script:appSemanticExpected.groupNames[1]) ('Visible group names did not restore for {0}.' -f $Name)
    Assert-Uat ($state.panes[1].current -eq $true) ('Active pane did not restore in the visible UI for {0}.' -f $Name)
    Assert-Uat ([string]::Equals([string]$state.panes[0].address, $script:appSemanticExpected.paneOneActive, [System.StringComparison]::OrdinalIgnoreCase)) ('Pane 1 did not restore its active folder path for {0}.' -f $Name)
    Assert-Uat ([string]::Equals([string]$state.panes[1].address, $script:appSemanticExpected.paneTwoActive, [System.StringComparison]::OrdinalIgnoreCase)) ('Pane 2 did not restore its active folder path for {0}.' -f $Name)
    Assert-Uat ((@($state.panes[0].tabs | Where-Object { $_.selected }).Count) -eq 1) ('Pane 1 did not restore exactly one selected tab for {0}.' -f $Name)
    Assert-Uat ((@($state.panes[0].tabs | Where-Object { $_.title -eq $script:appSemanticExpected.paneOneActive }).Count) -eq 1) ('Pane 1 did not restore the saved nested tab for {0}.' -f $Name)
    foreach ($filename in $script:appSemanticExpected.visibleFileNames) {
        Assert-Uat ($rowText.Contains($filename)) ('The visible installed listing omitted {0} for {1}.' -f $filename, $Name)
    }
    $statePath = Join-Path $providedEvidencePath ($Name + '-semantic-ui.json')
    Write-UatJsonFile -Path $statePath -Value $state
    Add-UatEvent -Kind 'app-semantic-ui' -Name $Name -Data @{ evidence = $statePath; theme = $state.theme; paneCount = $state.panes.Count; activePane = 1; activePaths = @($state.panes | ForEach-Object { $_.address }); visibleFixtureFiles = $script:appSemanticExpected.visibleFileNames }
    $workspaceToggle = Send-UatCdpClick -Session $Session -Selector 'button.workspace-toggle' -ExpectedChangeExpression "!!document.querySelector('.workspaces-dialog')"
    $workspaceScreenshot = Save-UatWindowEvidence -Window $Session.ownerWindow -Name ($Name + '-workspace-dialog')
    $workspaceExpression = @'
(() => Array.from(document.querySelectorAll('.saved-workspace button.open-workspace strong')).map(e => e.textContent.trim()))()
'@
    $workspaceNames = @(Invoke-UatCdpEvaluate -Session $Session -Expression $workspaceExpression)
    Assert-Uat ($workspaceNames -contains $script:appSemanticExpected.workspaceName) ('Saved workspace was not visible in the actual workspace dialog for {0}.' -f $Name)
    $workspacePath = Join-Path $providedEvidencePath ($Name + '-workspace-ui.json')
    Write-UatJsonFile -Path $workspacePath -Value @{ workspaceNames = $workspaceNames; openedBy = $workspaceToggle.mechanism; screenshot = $workspaceScreenshot }
    [void](Send-UatCdpClick -Session $Session -Selector 'button[title="작업공간 닫기"]' -ExpectedChangeExpression "!document.querySelector('.workspaces-dialog')")
    [void](Send-UatCdpClick -Session $Session -Selector 'button.workspace-toggle' -ExpectedChangeExpression "!!document.querySelector('.workspaces-dialog')")
    Assert-UatOwnedWindow -Window $Session.ownerWindow
    $nativeKey = Send-UatOwnedKey -WindowHandle $Session.ownerWindow.handle -ProcessId ([uint32]$Session.ownerWindow.processId) -VirtualKey 0x1B
    $Session.ownerWindow = Get-UatOwnedWindow -ProcessId $Session.ownerWindow.processId -StartTicks $Session.ownerWindow.startTicks -ExpectedImagePath $Session.ownerWindow.imagePath -KnownHandle $Session.ownerWindow.handle.ToInt64()
    [void](Wait-UatCdpCondition -Session $Session -Expression "!document.querySelector('.workspaces-dialog')" -TimeoutSeconds 10)
    $nativeKeyPath = Join-Path $providedEvidencePath ($Name + '-native-escape.json')
    Write-UatJsonFile -Path $nativeKeyPath -Value @{ input = $nativeKey; expectedWorkspaceClosed = $true; processId = $Session.ownerWindow.processId; startTicks = $Session.ownerWindow.startTicks; imagePath = $Session.ownerWindow.imagePath }
    return @($statePath, $workspacePath, $workspaceScreenshot, $nativeKeyPath)
}

function ConvertFrom-UatNativePathValue {
    param([Parameter(Mandatory = $true)]$Value)
    $builder = New-Object System.Text.StringBuilder
    foreach ($unit in @($Value)) { [void]$builder.Append([char][int]$unit) }
    return $builder.ToString()
}

function Assert-UatPersistedProfile {
    param([Parameter(Mandatory = $true)][string]$Name)
    $settingsPath = [string]$script:profileFixture.settings
    Assert-Uat (Test-Path -LiteralPath $settingsPath -PathType Leaf) ('App did not persist settings.json under the isolated profile for {0}.' -f $Name)
    $settings = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
    Assert-Uat ([int]$settings.version -eq 1 -and [int]$settings.layout -eq 2) ('Persisted profile schema/layout did not survive {0}.' -f $Name)
    Assert-Uat ([string]$settings.theme -eq 'dark' -and [int]$settings.active_pane -eq 1) ('Persisted theme or active pane did not survive {0}.' -f $Name)
    Assert-Uat ([string]$settings.pane_names[0] -eq $script:appSemanticExpected.groupNames[0] -and [string]$settings.pane_names[1] -eq $script:appSemanticExpected.groupNames[1]) ('Persisted group names changed during {0}.' -f $Name)
    $paneOnePaths = @($settings.panes[0] | ForEach-Object { ConvertFrom-UatNativePathValue $_ })
    Assert-UatEqual $script:appSemanticExpected.paneOneTabs $paneOnePaths ('Persisted tabs changed during {0}.' -f $Name)
    Assert-Uat (@($settings.workspaces | Where-Object { $_.name -eq $script:appSemanticExpected.workspaceName }).Count -eq 1) ('Persisted workspace was missing after {0}.' -f $Name)
    $evidencePath = Join-Path $providedEvidencePath ($Name + '-persisted-profile.json')
    Write-UatJsonFile -Path $evidencePath -Value ([ordered]@{
        profileDirectory = $script:profileRoot
        settingsPath = $settingsPath
        settingsSha256 = Get-UatFileHash $settingsPath
        version = [int]$settings.version
        layout = [int]$settings.layout
        theme = [string]$settings.theme
        activePane = [int]$settings.active_pane
        paneNames = @($settings.pane_names)
        paneOnePaths = $paneOnePaths
        workspaceNames = @($settings.workspaces | ForEach-Object { [string]$_.name })
    })
    return $evidencePath
}

function Start-UatDirectApp {
    param([Parameter(Mandatory = $true)][string]$Root, [Parameter(Mandatory = $true)][string]$Name)
    $image = Join-Path $Root 'SDIR.exe'
    $started = [DateTime]::UtcNow
    $entry = Start-UatProcess -Executable $image -Argument @() -WorkingDirectory $Root -Environment @{ SDIR_PROFILE_DIR = $script:profileRoot; SDIR_DEBUG_PORT = [string]$script:debugPort } -Name $Name
    $identity = Get-UatProcessIdentity -ProcessId $entry.processId
    Assert-Uat ($identity.startTicks -eq $entry.child.StartTimeUtc.Ticks) 'Direct launch PID/start-time identity changed before HWND discovery.'
    $window = Get-UatOwnedWindow -ProcessId $entry.processId -StartTicks $identity.startTicks -ExpectedImagePath $image
    Assert-Uat (-not [Sdir.Installer.Uat.NativeWindowApi]::IsProcessElevated($entry.processId)) 'Direct test app unexpectedly requested elevation.'
    return [pscustomobject]@{ processId = $entry.processId; startTicks = $identity.startTicks; imagePath = $image; installRoot = $Root; window = $window; ownedProcess = $entry; externalProcess = $null; launchedUtc = $started; route = 'direct ProcessStartInfo with explicit SDIR_PROFILE_DIR and SDIR_DEBUG_PORT' }
}

function Register-UatExternalAppProcess {
    param(
        [Parameter(Mandatory = $true)]$ProcessIdentity,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Route,
        $VerifiedLaunchHandoff = $null,
        [switch]$DeferWindow
    )
    $expectedImage = [System.IO.Path]::GetFullPath((Join-Path $Root 'SDIR.exe'))
    if ($null -ne $VerifiedLaunchHandoff) {
        if (-not [object]::ReferenceEquals($ProcessIdentity.process, $VerifiedLaunchHandoff.app.process) -or
            [int]$ProcessIdentity.processId -ne [int]$VerifiedLaunchHandoff.app.processId -or
            [long]$ProcessIdentity.startTicks -ne [long]$VerifiedLaunchHandoff.app.nativeStartTicks -or
            -not [string]::Equals([System.IO.Path]::GetFullPath([string]$Root), [string]$VerifiedLaunchHandoff.installRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
            -not [string]::Equals($expectedImage, [string]$VerifiedLaunchHandoff.app.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw 'Handoff registration did not receive the same pinned TEST app identity and installRoot.'
        }
        $process = $ProcessIdentity.process
        if ($null -eq $process -or $process.Handle -eq [IntPtr]::Zero -or
            $process.Handle -ne $VerifiedLaunchHandoff.app.pinnedHandle -or $process.HasExited -or
            [long]$process.StartTime.ToUniversalTime().Ticks -ne [long]$VerifiedLaunchHandoff.app.nativeStartTicks -or
            -not [string]::Equals([System.IO.Path]::GetFullPath([Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath([int]$ProcessIdentity.processId)), $expectedImage, [System.StringComparison]::OrdinalIgnoreCase) -or
            -not [string]::Equals((Get-UatFileHash $expectedImage), [string]$VerifiedLaunchHandoff.app.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw 'Handoff registration lost the pinned TEST app Process, handle, native identity, path, or hash.'
        }
        if ([Sdir.Installer.Uat.NativeWindowApi]::IsProcessElevated([int]$ProcessIdentity.processId)) {
            throw 'Registered TEST app unexpectedly has an elevated token.'
        }
        $identity = [pscustomobject]@{
            processId = [int]$ProcessIdentity.processId; startUtc = $VerifiedLaunchHandoff.app.startUtc
            startTicks = [long]$VerifiedLaunchHandoff.app.nativeStartTicks; imagePath = $expectedImage
        }
        $window = $null
        if (-not $DeferWindow) {
            $window = Get-UatOwnedWindow -ProcessId $identity.processId -StartTicks $identity.startTicks -ExpectedImagePath $expectedImage
        }
    }
    else {
        if ($DeferWindow) { throw 'DeferWindow is valid only for a verified setup app handoff.' }
        $identity = Assert-UatProcessIdentity -ProcessId $ProcessIdentity.processId -ExpectedStartTicks $ProcessIdentity.startTicks -ExpectedImagePath $expectedImage
        $window = Get-UatOwnedWindow -ProcessId $identity.processId -StartTicks $identity.startTicks -ExpectedImagePath $expectedImage
        $process = [System.Diagnostics.Process]::GetProcessById([int]$identity.processId)
    }
    $pinnedHandle = $process.Handle
    Assert-Uat ($pinnedHandle -ne [IntPtr]::Zero -and $process.Id -eq $identity.processId -and
        $process.StartTime.ToUniversalTime().Ticks -eq $identity.startTicks) 'External TEST app process did not match its retained PID, handle, and start identity.'
    $commandId = [Guid]::NewGuid().ToString('N')
    $entry = [pscustomobject]@{
        commandId = $commandId; sessionId = $script:runId; name = $Name; executable = $expectedImage
        arguments = '[inherited from validated isolated test launcher]'; workingDirectory = $Root
        kind = 'external-app'; processId = [int]$identity.processId; startedUtc = $identity.startUtc.ToString('o')
        startTicks = [long]$identity.startTicks; pinnedHandle = $pinnedHandle
        process = $process; child = $null; nativeEngineChildren = @(); finished = $false; outputCapture = 'GUI subsystem child launched by Inno/Shell; no redirected process stream was exposed'
        partialWriteFailed = $false
        launchHandoff = $VerifiedLaunchHandoff
        wrapperProcess = if ($null -ne $VerifiedLaunchHandoff) { $VerifiedLaunchHandoff.wrapper.process } else { $null }
        wrapperIdentity = if ($null -ne $VerifiedLaunchHandoff) { $VerifiedLaunchHandoff.wrapper } else { $null }
        wrapperFinished = $false; wrapperExitCode = $null; appExitCode = $null; closeBudget = $null
    }
    [void]$script:activeProcesses.Add($entry)
    [System.IO.File]::AppendAllText($script:commandsPath,
        (ConvertTo-Json -InputObject ([ordered]@{ commandId = $commandId; sessionId = $script:runId; name = $Name; executable = $expectedImage; arguments = $entry.arguments; workingDirectory = $Root; processId = [int]$identity.processId; startedUtc = $entry.startedUtc; phase = 'started'; outputCapture = $entry.outputCapture; launchHandoff = if ($null -ne $VerifiedLaunchHandoff) { $VerifiedLaunchHandoff.commandEvidence } else { $null } }) -Depth 12 -Compress) + "`r`n",
        (New-Object System.Text.UTF8Encoding -ArgumentList $false))
    Add-UatEvent -Kind 'process-started' -Name $Name -Data @{
        commandId = $commandId; processId = [int]$identity.processId; executable = $expectedImage
        route = $Route; outputCapture = $entry.outputCapture
        launchIntentId = if ($null -ne $VerifiedLaunchHandoff) { $VerifiedLaunchHandoff.intentId } else { $null }
        wrapperProcessId = if ($null -ne $VerifiedLaunchHandoff) { [int]$VerifiedLaunchHandoff.wrapper.processId } else { $null }
    }
    return [pscustomobject]@{
        processId = [int]$identity.processId; startTicks = [long]$identity.startTicks; imagePath = $expectedImage
        installRoot = [System.IO.Path]::GetFullPath($Root); window = $window; ownedProcess = $entry
        externalProcess = $process; launchedUtc = $identity.startUtc; route = $Route
        launchHandoff = $VerifiedLaunchHandoff
    }
}

function Start-UatAppFromWizard {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$VerifiedLaunchHandoff,
        [Parameter(Mandatory = $true)]$RegisteredApp
    )
    if (-not [string]::Equals([System.IO.Path]::GetFullPath($Root), [string]$VerifiedLaunchHandoff.installRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not [object]::ReferenceEquals($RegisteredApp.launchHandoff, $VerifiedLaunchHandoff) -or
        -not [object]::ReferenceEquals($RegisteredApp.externalProcess, $VerifiedLaunchHandoff.app.process) -or
        -not [object]::ReferenceEquals($RegisteredApp.ownedProcess.launchHandoff, $VerifiedLaunchHandoff)) {
        throw 'Wizard app HWND preparation did not receive its already registered pinned handoff object.'
    }
    if ($null -ne $RegisteredApp.window) { throw 'Deferred wizard app registration already performed HWND discovery.' }
    $RegisteredApp.window = Get-UatOwnedWindow -ProcessId $RegisteredApp.processId `
        -StartTicks $VerifiedLaunchHandoff.app.nativeStartTicks -ExpectedImagePath $VerifiedLaunchHandoff.app.imagePath
    return $RegisteredApp
}

function Start-UatAppFromShortcut {
    param([Parameter(Mandatory = $true)][string]$Root, [Parameter(Mandatory = $true)][string]$ShortcutPath, [Parameter(Mandatory = $true)][string]$Name)
    $started = [DateTime]::UtcNow
    $identity = Get-UatShortcutProcessStart -ShortcutPath $ShortcutPath -ExpectedTarget (Join-Path $Root 'sdir-installer-test-launcher.cmd') -NotBeforeUtc $started
    return Register-UatExternalAppProcess -ProcessIdentity $identity -Root $Root -Name $Name -Route ('Shell.Application open of ' + $ShortcutPath)
}

function Close-UatAppNormally {
    param([Parameter(Mandatory = $true)]$App, [Parameter(Mandatory = $true)][string]$Name)
    if ($null -ne $App.PSObject.Properties['launchHandoff'] -and $null -ne $App.launchHandoff) {
        $handoff = $App.launchHandoff
        $entry = $App.ownedProcess
        if ($entry.kind -ne 'external-app' -or $null -eq $App.externalProcess -or
            -not [object]::ReferenceEquals($App.externalProcess, $handoff.app.process) -or
            -not [object]::ReferenceEquals($entry.process, $handoff.app.process) -or
            -not [object]::ReferenceEquals($entry.wrapperProcess, $handoff.wrapper.process)) {
            throw ('Handoff app close lost its registered app or wrapper Process object for {0}.' -f $Name)
        }
        $appProcess = $App.externalProcess
        $wrapperProcess = $handoff.wrapper.process
        if ($appProcess.Handle -eq [IntPtr]::Zero -or $appProcess.Handle -ne $handoff.app.pinnedHandle -or
            [long]$appProcess.StartTime.ToUniversalTime().Ticks -ne [long]$handoff.app.nativeStartTicks -or
            -not [string]::Equals([System.IO.Path]::GetFullPath([Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath([int]$App.processId)), [string]$handoff.app.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw ('Pinned app identity changed before WM_CLOSE for {0}.' -f $Name)
        }
        if ($wrapperProcess.Handle -eq [IntPtr]::Zero -or $wrapperProcess.Handle -ne $handoff.wrapper.pinnedHandle -or
            [long]$wrapperProcess.StartTime.ToUniversalTime().Ticks -ne [long]$handoff.wrapper.nativeStartTicks -or
            (-not $wrapperProcess.HasExited -and
             -not [string]::Equals([System.IO.Path]::GetFullPath([Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath([int]$handoff.wrapper.processId)), [string]$handoff.wrapper.imagePath, [System.StringComparison]::OrdinalIgnoreCase))) {
            throw ('Pinned handoff wrapper handle or native identity changed before WM_CLOSE for {0}.' -f $Name)
        }
        $entry.closeBudget = New-UatCompletionBudget -TotalMilliseconds ([long]45000) -Origin ('normal app and wrapper close: ' + $Name)
        $knownCloseIdentities = @($handoff.app) + @($handoff.appDescendants)
        $closeRows = @(Get-UatProcessDescendantIds -RootProcessId $App.processId -RootStartTicks $App.startTicks -IncludeRows)
        foreach ($closeRow in $closeRows) {
            $knownCloseMatches = @($knownCloseIdentities | Where-Object {
                [int]$_.processId -eq [int]$closeRow.pid -and
                [Math]::Abs([long]$_.nativeStartTicks - [long]$closeRow.cimStartTicks) -lt [TimeSpan]::TicksPerMillisecond -and
                [int]$_.parentProcessId -eq [int]$closeRow.parent
            })
            if ($knownCloseMatches.Count -ne 1) {
                throw ('Normal close found an untransferred app descendant; no identity is adopted or excluded: PID {0}.' -f $closeRow.pid)
            }
            $knownCloseIdentity = $knownCloseMatches[0]
            if ($knownCloseIdentity.process.HasExited -or $knownCloseIdentity.process.Handle -ne $knownCloseIdentity.pinnedHandle -or
                [long]$knownCloseIdentity.process.StartTime.ToUniversalTime().Ticks -ne [long]$knownCloseIdentity.nativeStartTicks) {
                throw ('Normal close snapshot lost its retained native identity: PID {0}.' -f $closeRow.pid)
            }
        }
        Assert-UatOwnedWindow -Window $App.window
        if (-not [Sdir.Installer.Uat.NativeWindowApi]::PostClose($App.window.handle, [uint32]$App.processId)) {
            throw ('WM_CLOSE could not be posted to the validated handoff app HWND for {0}.' -f $Name)
        }
        Add-UatEvent -Kind 'normal-close-requested' -Name $Name -Data @{
            processId = $App.processId; startTicks = $App.startTicks; imagePath = $App.imagePath
            wrapperProcessId = $handoff.wrapper.processId; wrapperNativeStartTicks = $handoff.wrapper.nativeStartTicks
            windowHandle = $App.window.handle.ToInt64(); mechanism = 'PostMessage(WM_CLOSE)'; closeBudgetMilliseconds = 45000
        }
        $diagnostics = New-Object System.Collections.Generic.List[string]
        $appExitCode = $null
        $wrapperExitCode = $null
        $appExited = $false
        $wrapperExited = $false
        while (-not $appProcess.HasExited) {
            $remaining = Get-UatCompletionRemainingMilliseconds -CompletionBudget $entry.closeBudget
            if ($remaining -le 0) { break }
            [void]$appProcess.WaitForExit([int][Math]::Min(250, $remaining))
        }
        if ($appProcess.HasExited) {
            $appExited = $true
            try {
                if ($appProcess.Handle -ne $handoff.app.pinnedHandle -or
                    [long]$appProcess.StartTime.ToUniversalTime().Ticks -ne [long]$handoff.app.nativeStartTicks) {
                    [void]$diagnostics.Add('App exit handle or native start identity changed.')
                }
                $appExitCode = [int]$appProcess.ExitCode
            }
            catch { [void]$diagnostics.Add(('App exit code could not be read from the retained handle: {0}' -f $_.Exception.ToString())) }
        }
        else { [void]$diagnostics.Add('App did not exit within the shared 45-second closeBudget.') }
        foreach ($descendant in @($handoff.appDescendants)) {
            $descendantProcess = $descendant.process
            if ($null -eq $descendantProcess -or $descendantProcess.Handle -eq [IntPtr]::Zero -or
                $descendantProcess.Handle -ne $descendant.pinnedHandle) {
                [void]$diagnostics.Add(('Verified app descendant lost its retained Process handle: PID {0}.' -f $descendant.processId))
                continue
            }
            while (-not $descendantProcess.HasExited) {
                $remaining = Get-UatCompletionRemainingMilliseconds -CompletionBudget $entry.closeBudget
                if ($remaining -le 0) { break }
                [void]$descendantProcess.WaitForExit([int][Math]::Min(250, $remaining))
            }
            if ($descendantProcess.HasExited) {
                try {
                    if ([long]$descendantProcess.StartTime.ToUniversalTime().Ticks -ne [long]$descendant.nativeStartTicks) {
                        [void]$diagnostics.Add(('Verified app descendant native start identity changed for PID {0}.' -f $descendant.processId))
                    }
                    $descendant.exitCode = [int]$descendantProcess.ExitCode
                    $descendant.nativeRecord.exitCode = [int]$descendant.exitCode
                    $descendant.nativeRecord.finished = $true
                    $descendant.nativeRecord.completedUtc = [DateTime]::UtcNow.ToString('o')
                }
                catch { [void]$diagnostics.Add(('Verified app descendant exit code unavailable for PID {0}: {1}' -f $descendant.processId, $_.Exception.ToString())) }
            }
            else { [void]$diagnostics.Add(('Verified app descendant did not exit within the shared closeBudget: PID {0}.' -f $descendant.processId)) }
        }
        while (-not $wrapperProcess.HasExited) {
            $remaining = Get-UatCompletionRemainingMilliseconds -CompletionBudget $entry.closeBudget
            if ($remaining -le 0) { break }
            [void]$wrapperProcess.WaitForExit([int][Math]::Min(250, $remaining))
        }
        if ($wrapperProcess.HasExited) {
            $wrapperExited = $true
            try {
                if ($wrapperProcess.Handle -ne $handoff.wrapper.pinnedHandle -or
                    [long]$wrapperProcess.StartTime.ToUniversalTime().Ticks -ne [long]$handoff.wrapper.nativeStartTicks) {
                    [void]$diagnostics.Add('Wrapper exit handle or native start identity changed.')
                }
                $wrapperExitCode = [int]$wrapperProcess.ExitCode
            }
            catch { [void]$diagnostics.Add(('Wrapper exit code could not be read from the retained handle: {0}' -f $_.Exception.ToString())) }
        }
        else { [void]$diagnostics.Add('Handoff wrapper did not exit within the shared 45-second closeBudget.') }
        if ((Get-UatCompletionRemainingMilliseconds -CompletionBudget $entry.closeBudget) -le 0) {
            [void]$diagnostics.Add('The shared 45-second closeBudget expired before close completion recording.')
        }
        $handoff.app.exitCode = $appExitCode
        $handoff.wrapper.exitCode = $wrapperExitCode
        $handoff.commandEvidence.app.exitCode = $appExitCode
        $handoff.commandEvidence.wrapper.exitCode = $wrapperExitCode
        foreach ($descendant in @($handoff.appDescendants)) {
            $descendantEvidence = @($handoff.commandEvidence.appDescendants | Where-Object { [int]$_.processId -eq [int]$descendant.processId })
            if ($descendantEvidence.Count -eq 1) { $descendantEvidence[0].exitCode = $descendant.exitCode }
        }
        $entry.appExitCode = $appExitCode
        $entry.wrapperFinished = $wrapperExited -and $null -ne $wrapperExitCode
        $entry.wrapperExitCode = $wrapperExitCode
        $commandFolder = Join-Path $script:commandDirectory $entry.commandId
        New-Item -ItemType Directory -Path $commandFolder | Out-Null
        $empty = New-Object byte[] 0
        $stdoutPath = Join-Path $commandFolder 'stdout-gui-empty.bin'
        $stderrPath = Join-Path $commandFolder 'stderr-gui-empty.bin'
        [System.IO.File]::WriteAllBytes($stdoutPath, $empty)
        [System.IO.File]::WriteAllBytes($stderrPath, $empty)
        $record = [ordered]@{
            commandId = $entry.commandId; sessionId = $script:runId; name = $Name; executable = $App.imagePath
            arguments = $entry.arguments; workingDirectory = $App.installRoot; processId = $App.processId
            startedUtc = $entry.startedUtc; completedUtc = [DateTime]::UtcNow.ToString('o')
            exitCode = $appExitCode; appExitCode = $appExitCode; wrapperExitCode = $wrapperExitCode
            appExited = $appExited; wrapperExited = $wrapperExited; closeBudgetMilliseconds = 45000
            outputCapture = $entry.outputCapture; stdoutBytes = 0; stderrBytes = 0
            stdoutRaw = $stdoutPath; stderrRaw = $stderrPath; orderedChunks = @()
            phase = if ($diagnostics.Count -eq 0) { 'exited' } else { 'close-incomplete' }
            launchHandoff = $handoff.commandEvidence; diagnostic = if ($diagnostics.Count -gt 0) { $diagnostics -join ' | ' } else { $null }
        }
        [System.IO.File]::AppendAllText($script:commandsPath,
            (ConvertTo-Json -InputObject $record -Depth 20 -Compress) + [System.Environment]::NewLine,
            (New-Object System.Text.UTF8Encoding -ArgumentList $false))
        if ($diagnostics.Count -gt 0) {
            Add-UatEvent -Kind 'process-close-incomplete' -Name $Name -Data @{
                commandId = $entry.commandId; processId = $App.processId; appExitCode = $appExitCode
                wrapperProcessId = $handoff.wrapper.processId; wrapperExitCode = $wrapperExitCode
                diagnostic = $diagnostics.ToArray(); launchHandoff = $handoff.commandEvidence
            }
            throw ('App handoff close was incomplete for {0}; appExitCode={1}; wrapperExitCode={2}; diagnostic={3}' -f $Name, $appExitCode, $wrapperExitCode, ($diagnostics -join ' | '))
        }
        $entry.finished = $true
        Add-UatEvent -Kind 'process-exited' -Name $Name -Data @{
            commandId = $entry.commandId; processId = $App.processId; exitCode = $appExitCode
            wrapperProcessId = $handoff.wrapper.processId; wrapperExitCode = $wrapperExitCode
            outputCapture = $entry.outputCapture; streams = 'GUI subsystem; empty observation files do not imply redirected-stream capture'
        }
        $App.externalProcess.Dispose()
        $wrapperProcess.Dispose()
        foreach ($descendant in @($handoff.appDescendants)) { $descendant.process.Dispose() }
        Assert-Uat ($appExitCode -eq 0) ('App normal close did not exit with code 0 for {0}.' -f $Name)
        Assert-Uat ($wrapperExitCode -eq 0) ('Handoff wrapper normal close did not exit with code 0 for {0}.' -f $Name)
        Add-UatEvent -Kind 'normal-close-verified' -Name $Name -Data @{
            processId = $App.processId; exitCode = $appExitCode; wrapperProcessId = $handoff.wrapper.processId
            wrapperExitCode = $wrapperExitCode; descendantPids = @($handoff.appDescendants | ForEach-Object { $_.processId })
        }
        return $appExitCode
    }
    $descendants = @(Get-UatProcessDescendantIds -RootProcessId $App.processId -RootStartTicks $App.startTicks)
    Assert-UatOwnedWindow -Window $App.window
    if (-not [Sdir.Installer.Uat.NativeWindowApi]::PostClose($App.window.handle, [uint32]$App.processId)) {
        throw ('WM_CLOSE could not be posted to the validated app HWND for {0}.' -f $Name)
    }
    Add-UatEvent -Kind 'normal-close-requested' -Name $Name -Data @{ processId = $App.processId; startTicks = $App.startTicks; imagePath = $App.imagePath; windowHandle = $App.window.handle.ToInt64(); mechanism = 'PostMessage(WM_CLOSE)' }
    if ($null -eq $App.ownedProcess) { throw ('App close refused without a process entry kind for {0}.' -f $Name) }
    if ($App.ownedProcess.kind -notin @('owned-child', 'external-app')) {
        throw ('App close refused unknown process entry kind for {0}: {1}' -f $Name, [string]$App.ownedProcess.kind)
    }
    if ($App.ownedProcess.kind -eq 'owned-child') {
        if ($null -eq $App.ownedProcess.child) { throw ('App close refused an owned-child entry without its child handle for {0}.' -f $Name) }
        $exit = Wait-UatProcess -Entry $App.ownedProcess -TimeoutSeconds 45 -WaitReason ('normal app close: ' + $Name)
    }
    else {
        if ($null -eq $App.externalProcess) { throw ('App close refused an external-app entry without its process handle for {0}.' -f $Name) }
        $deadline = [DateTime]::UtcNow.AddSeconds(45)
        while (-not $App.externalProcess.HasExited -and [DateTime]::UtcNow -lt $deadline) { [void]$App.externalProcess.WaitForExit(250) }
        if (-not $App.externalProcess.HasExited) { throw ('App did not exit after WM_CLOSE for {0}; it was not terminated.' -f $Name) }
        $exitCode = [int]$App.externalProcess.ExitCode
        $commandFolder = Join-Path $script:commandDirectory $App.ownedProcess.commandId
        New-Item -ItemType Directory -Path $commandFolder | Out-Null
        $empty = New-Object byte[] 0
        $stdoutPath = Join-Path $commandFolder 'stdout-gui-empty.bin'
        $stderrPath = Join-Path $commandFolder 'stderr-gui-empty.bin'
        [System.IO.File]::WriteAllBytes($stdoutPath, $empty)
        [System.IO.File]::WriteAllBytes($stderrPath, $empty)
        $record = [ordered]@{
            commandId = $App.ownedProcess.commandId; sessionId = $script:runId; name = $Name; executable = $App.imagePath
            arguments = $App.ownedProcess.arguments; workingDirectory = $App.installRoot; processId = $App.processId
            startedUtc = $App.ownedProcess.startedUtc; completedUtc = [DateTime]::UtcNow.ToString('o'); exitCode = $exitCode
            outputCapture = $App.ownedProcess.outputCapture; stdoutBytes = 0; stderrBytes = 0
            stdoutRaw = $stdoutPath; stderrRaw = $stderrPath; orderedChunks = @(); phase = 'exited'
        }
        [System.IO.File]::AppendAllText($script:commandsPath,
            (ConvertTo-Json -InputObject $record -Depth 16 -Compress) + "`r`n",
            (New-Object System.Text.UTF8Encoding -ArgumentList $false))
        $App.ownedProcess.appExitCode = $exitCode
        $App.ownedProcess.finished = $true
        $App.externalProcess.Dispose()
        Add-UatEvent -Kind 'process-exited' -Name $Name -Data @{ commandId = $App.ownedProcess.commandId; processId = $App.processId; exitCode = $exitCode; outputCapture = $App.ownedProcess.outputCapture; streams = 'GUI subsystem; empty observation files do not imply redirected-stream capture' }
        $exit = [pscustomobject]@{ exitCode = $exitCode }
    }
    Assert-Uat ($exit.exitCode -eq 0) ('App normal close did not exit with code 0 for {0}.' -f $Name)
    foreach ($childId in $descendants) {
        if ($childId -eq $App.processId) { continue }
        try {
            $child = [System.Diagnostics.Process]::GetProcessById([int]$childId)
            $childStart = $child.StartTime.ToUniversalTime().Ticks
            $child.Dispose()
            if ($childStart -ge $App.startTicks) { throw ('Harness app child PID {0} remained active after normal close.' -f $childId) }
        }
        catch [System.ArgumentException] { }
    }
    Add-UatEvent -Kind 'normal-close-verified' -Name $Name -Data @{ processId = $App.processId; exitCode = $exit.exitCode; descendantPids = $descendants }
    return $exit.exitCode
}

function Test-UatLaunchRoute {
    param([Parameter(Mandatory = $true)]$App, [Parameter(Mandatory = $true)][string]$Name)
    $windowEvidence = Save-UatWindowEvidence -Window $App.window -Name ($Name + '-window')
    $restoreEvidence = New-Object System.Collections.Generic.List[string]
    foreach ($testState in @('hidden', 'minimized')) {
        Assert-UatOwnedWindow -Window $App.window
        Assert-Uat ([Sdir.Installer.Uat.NativeWindowApi]::SetOwnedWindowTestState($App.window.handle, [uint32]$App.processId, ($testState -eq 'minimized'))) 'Owned window test-state request was refused.'
        $observed = $null
        $stateDeadline = [DateTime]::UtcNow.AddSeconds(5)
        while ([DateTime]::UtcNow -lt $stateDeadline) {
            [void](Assert-UatProcessIdentity -ProcessId $App.processId -ExpectedStartTicks $App.startTicks -ExpectedImagePath $App.imagePath)
            $states = @([Sdir.Installer.Uat.NativeWindowApi]::TopLevelWindows([uint32]$App.processId, $true) | Where-Object { $_.Handle -eq $App.window.handle })
            if ($states.Count -eq 1 -and (($testState -eq 'hidden' -and -not $states[0].Visible) -or ($testState -eq 'minimized' -and $states[0].Minimized))) { $observed = $states[0]; break }
            Start-Sleep -Milliseconds 50
        }
        Assert-Uat ($null -ne $observed) ('Owned HWND did not enter the requested test state: {0}' -f $testState)
        $App.window = Get-UatOwnedWindow -ProcessId $App.processId -StartTicks $App.startTicks -ExpectedImagePath $App.imagePath -KnownHandle $App.window.handle.ToInt64()
        $restoredScreenshot = Save-UatWindowEvidence -Window $App.window -Name ($Name + '-' + $testState + '-restored')
        $restorePath = Join-Path $providedEvidencePath ($Name + '-' + $testState + '-restore.json')
        Write-UatJsonFile -Path $restorePath -Value @{ requestedState = $testState; observedVisible = $observed.Visible; observedMinimized = $observed.Minimized; restoredVisible = $App.window.window.Visible; restoredMinimized = $App.window.window.Minimized; windowHandle = $App.window.handle.ToInt64(); processId = $App.processId; startTicks = $App.startTicks; imagePath = $App.imagePath; screenshot = $restoredScreenshot }
        [void]$restoreEvidence.Add($restorePath)
        [void]$restoreEvidence.Add($restoredScreenshot)
    }
    $sessionRecord = Get-UatCdpSessionForApp -App $App
    try {
        $uiEvidence = @(Assert-UatAppSemanticState -Session $sessionRecord.session -Name $Name)
        [void](Close-UatAppNormally -App $App -Name $Name)
    }
    finally { $sessionRecord.session.socket.Dispose() }
    $profileEvidence = Assert-UatPersistedProfile -Name $Name
    return @($windowEvidence) + @($restoreEvidence.ToArray()) + $uiEvidence + @($profileEvidence)
}

function Get-UatPayloadPresence {
    param([Parameter(Mandatory = $true)][string]$Root)
    $paths = New-Object System.Collections.Generic.List[object]
    foreach ($entry in $script:stablePayloadEntries) {
        $path = Join-Path $Root ([string]$entry.destination -replace '/', '\')
        [void]$paths.Add([ordered]@{ path = $path; relativePath = [string]$entry.destination; exists = (Test-Path -LiteralPath $path -PathType Leaf) })
    }
    return @($paths.ToArray())
}

function Assert-UatInstalledPayload {
    param([Parameter(Mandatory = $true)]$Candidate, [Parameter(Mandatory = $true)][string]$Root, [bool]$DesktopExpected = $false)
    $expectedEntries = @($Candidate.receipt.payloadInventory)
    foreach ($entry in $expectedEntries) {
        $path = Join-Path $Root ([string]$entry.destination -replace '/', '\')
        Assert-Uat (Test-Path -LiteralPath $path -PathType Leaf) ('Installed payload entry is missing: {0}' -f $entry.destination)
        $item = Get-Item -LiteralPath $path
        Assert-Uat ([long]$item.Length -eq [long]$entry.bytes) ('Installed payload byte count differs: {0}' -f $entry.destination)
        Assert-Uat ((Get-UatFileHash $path) -eq [string]$entry.sha256) ('Installed payload SHA-256 differs: {0}' -f $entry.destination)
    }
    $receiptPath = Join-Path $Root 'sdir.install-receipt'
    Assert-Uat (Test-Path -LiteralPath $receiptPath -PathType Leaf) 'The installed ownership receipt is missing.'
    Assert-Uat ((Get-UatFileHash $receiptPath) -match '^[0-9a-f]{64}$') 'The installed ownership receipt could not be hashed.'
    $receiptLines = [System.IO.File]::ReadAllLines($receiptPath)
    Assert-Uat ($receiptLines.Count -gt 0 -and [string]::Equals([string]$receiptLines[0], 'SDIR-RECEIPT-V2', [System.StringComparison]::Ordinal)) 'The installed ownership receipt is not SDIR-RECEIPT-V2.'
    $knownInventoryIds = @('production-stable766-v1', 'test-stable766-v1', 'test-baselineb158-v1')
    $candidateInventoryId = [string]$Candidate.receipt.activeInventoryId
    Assert-Uat ($knownInventoryIds -ccontains $candidateInventoryId) 'The candidate build receipt has an unknown active inventory selector.'
    $expectedInventoryId = if (-not [bool]$Candidate.receipt.testBuild) { 'production-stable766-v1' }
        elseif ([bool]$Candidate.receipt.testBaseline) { 'test-baselineb158-v1' }
        else { 'test-stable766-v1' }
    Assert-Uat ([string]::Equals($candidateInventoryId, $expectedInventoryId, [System.StringComparison]::Ordinal)) 'The candidate build receipt active inventory selector does not match its identity.'
    $inventoryLines = @($receiptLines | Where-Object { $_ -match '^Inventory' })
    Assert-Uat ($inventoryLines.Count -eq 1) 'The installed receipt must contain exactly one Inventory record.'
    $inventoryParts = $inventoryLines[0].Split('|')
    Assert-Uat ($inventoryParts.Count -eq 2 -and [string]::Equals([string]$inventoryParts[0], 'Inventory', [System.StringComparison]::Ordinal) -and [string]::Equals([string]$inventoryParts[1], $expectedInventoryId, [System.StringComparison]::Ordinal)) 'The installed receipt inventory selector does not match the actual candidate build receipt.'
    $historyLines = @($receiptLines | Where-Object { $_ -match '^History(\||$)' })
    Assert-Uat ($historyLines.Count -eq 0) 'The installed receipt must not contain Historical ownership records.'
    $payloadLines = @($receiptLines | Where-Object { $_ -match '^Payload' })
    Assert-Uat ($payloadLines.Count -eq $expectedEntries.Count) 'The installed receipt payload record count does not match the candidate inventory.'
    $expectedPayload = New-Object System.Collections.Generic.List[object]
    foreach ($entry in $expectedEntries) {
        $relativePath = [string]$entry.destination
        $duplicate = @($expectedPayload | Where-Object { [string]::Equals([string]$_.relativePath, $relativePath, [System.StringComparison]::Ordinal) })
        Assert-Uat ($duplicate.Count -eq 0) ('The candidate payload inventory duplicates {0}.' -f $relativePath)
        [void]$expectedPayload.Add([pscustomobject]@{ relativePath = $relativePath; sha256 = [string]$entry.sha256 })
    }
    $seenPayload = New-Object System.Collections.Generic.List[string]
    foreach ($line in $payloadLines) {
        $payloadParts = $line.Split('|')
        Assert-Uat ($payloadParts.Count -eq 3 -and [string]::Equals([string]$payloadParts[0], 'Payload', [System.StringComparison]::Ordinal) -and [string]$payloadParts[2] -match '^[0-9a-f]{64}$') 'The installed receipt contains a malformed Payload record.'
        $expectedMatches = @($expectedPayload | Where-Object { [string]::Equals([string]$_.relativePath, [string]$payloadParts[1], [System.StringComparison]::Ordinal) })
        Assert-Uat ($expectedMatches.Count -eq 1) ('The installed receipt contains an unexpected payload path: {0}' -f $payloadParts[1])
        $alreadySeen = @($seenPayload | Where-Object { [string]::Equals([string]$_, [string]$payloadParts[1], [System.StringComparison]::Ordinal) })
        Assert-Uat ($alreadySeen.Count -eq 0) ('The installed receipt duplicates payload path: {0}' -f $payloadParts[1])
        Assert-Uat ([string]::Equals([string]$expectedMatches[0].sha256, [string]$payloadParts[2], [System.StringComparison]::Ordinal)) ('The installed receipt payload hash differs from the candidate inventory: {0}' -f $payloadParts[1])
        [void]$seenPayload.Add([string]$payloadParts[1])
    }
    Assert-Uat ($seenPayload.Count -eq $expectedPayload.Count) 'The installed receipt is missing candidate payload records.'
    $registry = Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey
    Assert-Uat $registry.exists 'The test HKCU uninstall registration is missing.'
    $matchingKeys = @(Get-ChildItem -LiteralPath 'Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction Stop |
        Where-Object { $_.PSChildName -eq ($script:testAppId + '_is1') })
    Assert-Uat ($matchingKeys.Count -eq 1) 'The installed test identity does not have exactly one matching HKCU uninstall key.'
    $registryKeyObject = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($script:registryKey, $false)
    try {
        $uninstallText = [string]$registryKeyObject.GetValue('UninstallString', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $quietText = [string]$registryKeyObject.GetValue('QuietUninstallString', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    }
    finally { $registryKeyObject.Dispose() }
    $uninstaller = Join-Path $Root 'unins000.exe'
    Assert-Uat ((Get-UatFileHash $uninstaller) -match '^[0-9a-f]{64}$') 'The registered native uninstaller is missing or unreadable.'
    Assert-Uat ($uninstallText -match [Regex]::Escape($uninstaller)) 'UninstallString does not target the installed native uninstaller.'
    Assert-Uat ($quietText -match [Regex]::Escape($uninstaller)) 'QuietUninstallString does not target the installed native uninstaller.'
    $start = Get-UatTestShortcutTarget -Path $script:startMenuShortcut
    $expectedLauncher = Join-Path $Root 'sdir-installer-test-launcher.cmd'
    Assert-Uat ([string]::Equals($start.targetPath, [System.IO.Path]::GetFullPath($expectedLauncher), [System.StringComparison]::OrdinalIgnoreCase)) 'Start Menu shortcut target is not the isolated launcher.'
    Assert-Uat ([string]::Equals($start.workingDirectory, [System.IO.Path]::GetFullPath($Root), [System.StringComparison]::OrdinalIgnoreCase)) 'Start Menu shortcut working directory is not the test root.'
    if ($DesktopExpected) {
        $desktop = Get-UatTestShortcutTarget -Path $script:desktopShortcut
        Assert-Uat ([string]::Equals($desktop.targetPath, [System.IO.Path]::GetFullPath($expectedLauncher), [System.StringComparison]::OrdinalIgnoreCase)) 'Desktop shortcut target is not the isolated launcher.'
        Assert-Uat ([string]::Equals($desktop.workingDirectory, [System.IO.Path]::GetFullPath($Root), [System.StringComparison]::OrdinalIgnoreCase)) 'Desktop shortcut working directory is not the test root.'
    }
    else { Assert-Uat (-not (Test-Path -LiteralPath $script:desktopShortcut)) 'Desktop shortcut exists when the desktop option was disabled.' }
    return [ordered]@{
        root = $Root
        receiptSha256 = Get-UatFileHash $receiptPath
        payloadFiles = @($expectedEntries | ForEach-Object { [ordered]@{ destination = $_.destination; bytes = $_.bytes; sha256 = $_.sha256 } })
        registryKey = $script:registryKey
        uninstallString = $uninstallText
        quietUninstallString = $quietText
        startMenu = $start
        desktop = if ($DesktopExpected) { Get-UatTestShortcutTarget -Path $script:desktopShortcut } else { $null }
    }
}

function Get-UatWizardButtonNames {
    param([Parameter(Mandatory = $true)]$Window)
    $summary = @(Get-UatWindowSummary -Window $Window)
    return @($summary | Where-Object { $_.controlType -eq 'ControlType.Button' -and $_.enabled -and -not $_.offscreen } | ForEach-Object { [string]$_.name })
}

function Save-UatWizardState {
    param([Parameter(Mandatory = $true)]$Window, [Parameter(Mandatory = $true)][string]$Prefix, [Parameter(Mandatory = $true)][string]$Step)
    $summary = @(Get-UatWindowSummary -Window $Window)
    $jsonPath = Join-Path $providedEvidencePath (($Prefix + '-' + $Step + '-uia.json') -replace '[^A-Za-z0-9._-]', '-')
    Write-UatJsonFile -Path $jsonPath -Value $summary
    $imagePath = Save-UatWindowEvidence -Window $Window -Name ($Prefix + '-' + $Step)
    Add-UatEvent -Kind 'wizard-page-observed' -Name $Step -Data @{ processId = $Window.processId; windowHandle = $Window.handle.ToInt64(); accessibleText = @($summary | ForEach-Object { $_.name }); screenshot = $imagePath; uiAutomation = $jsonPath }
    return @($imagePath, $jsonPath)
}

function Move-UatWizardToOptions {
    param([Parameter(Mandatory = $true)]$Window, [Parameter(Mandatory = $true)][string]$Prefix)
    $welcomeText = Get-UatWindowSummaryText -Window $Window
    Assert-Uat ($welcomeText -match 'SDIR|설치') 'The actual Korean SDIR setup welcome page was not exposed by UI Automation.'
    $evidence = New-Object System.Collections.Generic.List[string]
    foreach ($path in (Save-UatWizardState -Window $Window -Prefix $Prefix -Step 'welcome')) { [void]$evidence.Add($path) }
    for ($step = 0; $step -lt 12; $step++) {
        $summary = @(Get-UatWindowSummary -Window $Window)
        $option = Get-UatOwnedInnoDesktopOption -Window $Window
        if ($null -ne $option) {
            foreach ($path in (Save-UatWizardState -Window $Window -Prefix $Prefix -Step 'options')) { [void]$evidence.Add($path) }
            return @($evidence.ToArray())
        }
        $next = @(Find-UatWindowButton -Window $Window -NamePattern '^다음' -InnoRole Next)
        if ($next.Count -ne 1) { throw 'The Korean setup wizard did not reach the desktop task options page.' }
        $before = ConvertTo-Json -InputObject $summary -Depth 12 -Compress
        [void](Invoke-UatWindowButton -Window $Window -NamePattern '^다음' -InnoRole Next -ExpectedInnoButton $next[0])
        [void](Wait-UatWindowSummaryChange -Window $Window -Before $before)
        foreach ($path in (Save-UatWizardState -Window $Window -Prefix $Prefix -Step ('page-' + ($step + 1)))) { [void]$evidence.Add($path) }
    }
    throw 'Setup wizard did not expose the desktop shortcut option within the bounded page count.'
}

function Move-UatWizardToReady {
    param([Parameter(Mandatory = $true)]$Window, [Parameter(Mandatory = $true)][string]$Prefix)
    $evidence = New-Object System.Collections.Generic.List[string]
    for ($step = 0; $step -lt 12; $step++) {
        $install = @(Find-UatWindowButton -Window $Window -NamePattern '^(설치|Install)' -InnoRole Install)
        if ($install.Count -eq 1) {
            $text = Get-UatWindowSummaryText -Window $Window
            Assert-Uat ($text -match '설치|프로그램|SDIR') 'The ready page did not expose Korean installation controls.'
            foreach ($path in (Save-UatWizardState -Window $Window -Prefix $Prefix -Step 'ready')) { [void]$evidence.Add($path) }
            return @($evidence.ToArray())
        }
        if ($install.Count -gt 1) { throw 'The setup wizard has multiple install controls.' }
        $next = @(Find-UatWindowButton -Window $Window -NamePattern '^다음' -InnoRole Next)
        if ($next.Count -ne 1) { throw 'The setup wizard did not reach its ready-to-install page.' }
        $before = ConvertTo-Json -InputObject (Get-UatWindowSummary -Window $Window) -Depth 12 -Compress
        [void](Invoke-UatWindowButton -Window $Window -NamePattern '^다음' -InnoRole Next -ExpectedInnoButton $next[0])
        [void](Wait-UatWindowSummaryChange -Window $Window -Before $before)
        foreach ($path in (Save-UatWizardState -Window $Window -Prefix $Prefix -Step ('ready-page-' + ($step + 1)))) { [void]$evidence.Add($path) }
    }
    throw 'Setup wizard did not reach its ready page within the bounded page count.'
}

function Select-UatInnoExitChoice {
    param([object[]]$Summary, [object[]]$NativeChoices, [uint32]$ProcessId, [string]$Role)
    if ($NativeChoices.Count -ne 1) { return }
    $native = $NativeChoices[0]
    $api = [Sdir.Installer.Uat.NativeWindowApi]
    if ($native.handle -le 0 -or $native.processId -ne $ProcessId -or $native.className -cne 'Button' -or
        -not $native.isDirectChild -or -not $native.visible -or -not $native.enabled -or
        $native.right -le $native.left -or $native.bottom -le $native.top -or
        -not $api::IsInnoExitChoice($Role, $native.id, $native.title, $true)) { return }
    $matches = @($Summary | Where-Object {
        $_.nativeHandle -eq $native.handle -and $_.enabled -and -not $_.offscreen -and
        $_.controlType -cin @('ControlType.Button', 'ControlType.Pane') -and
        $api::IsInnoExitChoice($Role, $native.id, $_.name, $false)
    })
    if ($matches.Count -ne 1) { return }
    return [pscustomobject]@{
        role = $Role; id = $native.id; name = $matches[0].name; controlType = $matches[0].controlType
        nativeHandle = $native.handle; nativeCaption = $native.title
        left = $native.left; top = $native.top; right = $native.right; bottom = $native.bottom
    }
}

function Get-UatInnoExitChoices {
    param([Parameter(Mandatory = $true)]$MainWindow, [Parameter(Mandatory = $true)]$Modal)
    Assert-UatOwnedWindow -Window $MainWindow
    Assert-UatOwnedWindow -Window $Modal
    $api = [Sdir.Installer.Uat.NativeWindowApi]
    if ($null -eq $MainWindow.PSObject.Properties['nativeProcessEntry'] -or
        $null -eq $Modal.PSObject.Properties['nativeProcessEntry'] -or
        -not [object]::ReferenceEquals($MainWindow.nativeProcessEntry, $Modal.nativeProcessEntry) -or
        -not [object]::ReferenceEquals($MainWindow.nativeIdentity, $Modal.nativeIdentity) -or
        $MainWindow.nativeWindowRole -cne 'main' -or $Modal.nativeWindowRole -cne 'modal' -or
        $MainWindow.window.ClassName -cne 'TWizardForm' -or $Modal.window.ClassName -cne '#32770' -or
        $Modal.ownerWindowHandle -ne $MainWindow.handle -or -not $api::IsInnoExitCaption($Modal.window.Title)) {
        throw 'The setup exit dialog does not match the pinned wizard identity, owner, class and caption.'
    }
    $summary = @(Get-UatWindowSummary -Window $Modal)
    foreach ($role in @('Yes', 'No')) {
        $native = @($api::OwnedInnoExitChoices($MainWindow.handle, $Modal.handle, [uint32]$Modal.processId, $role) |
            ForEach-Object {
                [pscustomobject]@{
                    handle = $_.Handle.ToInt64(); processId = $api::OwnerProcessId($_.Handle)
                    className = $_.ClassName; title = $_.Title; id = $api::GetDlgCtrlID($_.Handle)
                    isDirectChild = $api::IsDescendant($Modal.handle, $_.Handle)
                    visible = $_.Visible; enabled = $api::IsEnabled($_.Handle)
                    left = $_.Left; top = $_.Top; right = $_.Right; bottom = $_.Bottom
                }
            })
        $choice = Select-UatInnoExitChoice -Summary $summary -NativeChoices $native -ProcessId $Modal.processId -Role $role
        if ($null -eq $choice) { throw ('No unique exact setup exit {0} control matched native and UIA observations.' -f $role) }
        $choice
    }
}

function Invoke-UatInnoExitConfirmation {
    param([Parameter(Mandatory = $true)]$MainWindow, [Parameter(Mandatory = $true)]$Modal)
    $choices = @(Get-UatInnoExitChoices -MainWindow $MainWindow -Modal $Modal)
    $yes = $choices[0]
    Assert-UatOwnedWindow -Window $MainWindow
    Assert-UatOwnedWindow -Window $Modal
    $handle = [IntPtr]::new([long]$yes.nativeHandle)
    if ($yes.controlType -ceq 'ControlType.Button') {
        $exactName = '^' + [regex]::Escape($yes.name) + '$'
        $action = Invoke-UatAutomationButton -WindowHandle $Modal.handle -ProcessId ([uint32]$Modal.processId) -NamePattern $exactName -ExpectedNativeHandle $handle
    } else {
        $api = [Sdir.Installer.Uat.NativeWindowApi]
        if (-not $api::PostOwnedInnoExitChoice($MainWindow.handle, $Modal.handle, $handle, [uint32]$Modal.processId, $yes.role, $yes.id, $yes.nativeCaption)) {
            throw 'The setup exit Yes control changed before native input or the click could not be queued.'
        }
        $action = [pscustomobject]@{ name = $yes.name; mechanism = 'PostMessage(WM_LBUTTONDOWN/UP)'; nativeHandle = $yes.nativeHandle }
    }
    Add-UatEvent -Kind 'native-ui-input' -Name 'setup exit confirmation' -Data @{
        processId = $Modal.processId; processStartTicks = $Modal.startTicks; imagePath = $Modal.imagePath
        windowHandle = $Modal.handle.ToInt64(); ownerWindowHandle = $MainWindow.handle.ToInt64()
        caption = $Modal.window.Title; mechanism = $action.mechanism; targetHandle = $action.nativeHandle; choices = $choices
    }
    return $action
}

function Confirm-UatWizardCancellation {
    param([Parameter(Mandatory = $true)]$Wizard, [Parameter(Mandatory = $true)][string]$Prefix)
    $window = $Wizard.window
    $completionBudget = $Wizard.process.completionBudget
    [void](Invoke-UatWindowButton -Window $window -NamePattern '^(취소|Cancel)' -InnoRole Cancel)
    $modal = Get-UatOwnedModalWindow -MainWindow $window -ProcessEntry $Wizard.process -TimeoutSeconds 5 -CompletionBudget $completionBudget
    if ($modal) {
        $modalEvidence = Save-UatWindowEvidence -Window $modal -Name ($Prefix + '-cancel-confirmation')
        [void](Invoke-UatInnoExitConfirmation -MainWindow $window -Modal $modal)
        $evidence = @($modalEvidence)
    }
    else { $evidence = @() }
    $result = Wait-UatProcess -Entry $Wizard.process -TimeoutSeconds 45 -WaitReason 'setup wizard cancellation' -CompletionBudget $completionBudget
    return [pscustomobject]@{ exitCode = $result.exitCode; evidence = @($evidence) + @($result.evidence.directory); logPath = $Wizard.logPath }
}

function Complete-UatSetupFinish {
    param(
        [Parameter(Mandatory = $true)]$Wizard,
        [Parameter(Mandatory = $true)][string]$Prefix,
        [Parameter(Mandatory = $true)]$Candidate,
        [bool]$LaunchAfterInstall = $false,
        [int]$TimeoutSeconds = 90
    )
    if (-not [object]::ReferenceEquals($Wizard.candidate, $Candidate)) {
        throw 'Finish received a different Candidate than the one preserved by Start-UatSetupGui.'
    }
    $completionBudget = $Wizard.process.completionBudget
    if ($null -eq $completionBudget -or -not [object]::ReferenceEquals($Wizard.process.completionBudget, $completionBudget)) {
        throw 'Finish has no original setup Entry CompletionBudget.'
    }
    $window = $Wizard.window
    if ($LaunchAfterInstall) {
        $runSummary = @(Get-UatWindowSummary -Window $window | Where-Object { $_.controlType -eq 'ControlType.CheckBox' -and $_.name -match 'SDIR.*(실행|빌드 실행)' })
        if ($runSummary.Count -ne 1) { throw 'The finish page did not expose the isolated test app launch checkbox.' }
        [void](Set-UatWindowCheckBox -Window $window -NamePattern 'SDIR.*(실행|빌드 실행)' -Checked $true)
    }
    else {
        $runSummary = @(Get-UatWindowSummary -Window $window | Where-Object { $_.controlType -eq 'ControlType.CheckBox' -and $_.name -match 'SDIR.*(실행|빌드 실행)' })
        if ($runSummary.Count -eq 1) {
            [void](Set-UatWindowCheckBox -Window $window -NamePattern 'SDIR.*(실행|빌드 실행)' -Checked $false)
        }
    }
    $finishEvidence = Save-UatWindowEvidence -Window $window -Name ($Prefix + '-finish')
    $launchIntent = $null
    $startAppBefore = $null
    if ($LaunchAfterInstall) {
        $cmdPath = [System.IO.Path]::GetFullPath((Join-Path (Join-Path ([Environment]::GetEnvironmentVariable('SystemRoot')) 'System32') 'cmd.exe'))
        $launchIntent = New-UatSetupLaunchIntent -Entry $Wizard.process -Candidate $Candidate `
            -ExpectedCmdExePath $cmdPath -CompletionBudget $completionBudget
        Update-UatNativeEngineChildren -Entry $Wizard.process
        $launchIntent.nativeCaptureBeforeClickUtc = [DateTime]::UtcNow.ToString('o')
        $launchIntent.clickNotBeforeUtc = [DateTime]::UtcNow
        $startAppBefore = $launchIntent.clickNotBeforeUtc
    }
    [void](Invoke-UatWindowButton -Window $window -NamePattern '^(마침|완료|Finish)' -InnoRole Finish)
    if ($LaunchAfterInstall) {
        Update-UatNativeEngineChildren -Entry $Wizard.process
        $launchIntent.nativeCaptureAfterClickUtc = [DateTime]::UtcNow.ToString('o')
    }
    $setupResult = Wait-UatProcess -Entry $Wizard.process -TimeoutSeconds $TimeoutSeconds -WaitReason 'setup finish page' -CompletionBudget $completionBudget
    $launchedApp = $null
    if ($LaunchAfterInstall) {
        if ($launchIntent.state -ne 'transferred' -or $null -eq $launchIntent.verifiedHandoff -or $null -eq $launchIntent.registeredApp) {
            throw ('Finish setup exited without a registered app handoff for intent {0}.' -f $launchIntent.intentId)
        }
        $launchedApp = Start-UatAppFromWizard -Root $Candidate.installRoot `
            -VerifiedLaunchHandoff $launchIntent.verifiedHandoff -RegisteredApp $launchIntent.registeredApp
    }
    return [pscustomobject]@{
        setupExitCode = $setupResult.exitCode; setupEvidence = $setupResult.evidence.directory
        finishScreenshot = $finishEvidence; app = $launchedApp; innoLog = $Wizard.logPath
        launchIntent = $launchIntent; startAppNotBeforeUtc = $startAppBefore
    }
}

function Invoke-UatWizardInstall {
    param(
        [Parameter(Mandatory = $true)]$Candidate,
        [Parameter(Mandatory = $true)][string]$Name,
        [bool]$DesktopShortcut = $false,
        [bool]$CancelAtReady = $false,
        [bool]$CancelDuringInstall = $false,
        [bool]$DeferFinish = $false,
        [bool]$LaunchAfterInstall = $false,
        [int]$TimeoutSeconds = 180
    )
    Assert-Uat (-not ($CancelAtReady -and $CancelDuringInstall)) 'A setup journey cannot cancel both before and during installation.'
    $installBudget = New-UatCompletionBudget -TotalMilliseconds ([long]$TimeoutSeconds * [long]1000) -Origin ($Name + ' setup route')
    $wizard = Start-UatSetupGui -Candidate $Candidate -Name $Name -TimeoutSeconds $TimeoutSeconds -CompletionBudget $installBudget
    $evidence = New-Object System.Collections.Generic.List[string]
    foreach ($path in (Move-UatWizardToOptions -Window $wizard.window -Prefix $Name)) { [void]$evidence.Add($path) }
    $option = Set-UatWindowCheckBox -Window $wizard.window -NamePattern '바탕 화면 바로 가기 추가' -Checked $DesktopShortcut
    [void]$evidence.Add((Save-UatWindowEvidence -Window $wizard.window -Name ($Name + '-desktop-' + [string]$DesktopShortcut)))
    foreach ($path in (Move-UatWizardToReady -Window $wizard.window -Prefix $Name)) { [void]$evidence.Add($path) }
    if ($CancelAtReady) {
        $cancel = Confirm-UatWizardCancellation -Wizard $wizard -Prefix $Name
        return [pscustomobject]@{ cancelled = $true; phase = 'before-mutation'; exitCode = $cancel.exitCode; evidence = @($evidence.ToArray()) + @($cancel.evidence); logPath = $cancel.logPath; wizard = $wizard }
    }
    $preRoot = Get-UatTreeSnapshot -Path $Candidate.installRoot
    $preProfile = Get-UatTreeSnapshot -Path $script:profileRoot
    $installInput = Invoke-UatWindowButton -Window $wizard.window -NamePattern '^(설치|Install)' -InnoRole Install -AsyncPostMessage
    $partialObserved = $false
    $installEvidence = New-Object System.Collections.Generic.List[string]
    while ((Get-UatCompletionRemainingMilliseconds -CompletionBudget $installBudget) -gt 0) {
        $complete = $false
        try { $complete = Test-UatProcessComplete -Entry $wizard.process }
        catch {
            $completionFailure = $_.Exception.ToString()
            try { Save-UatProcessPartial -Entry $wizard.process -Diagnostic $completionFailure }
            catch { throw ('Setup completion observation failed for {0}; partial save also failed: {1}; original: {2}' -f $Name, $_.Exception.ToString(), $completionFailure) }
            throw ('Setup completion observation failed for {0}: {1}' -f $Name, $completionFailure)
        }
        if ($complete) {
            $capture = Wait-UatProcess -Entry $wizard.process -TimeoutSeconds $TimeoutSeconds -CompletionBudget $installBudget -WaitReason ('setup exit before finish state: ' + $Name)
            throw ('Setup exited before the wizard finish state was observed; exit={0}. Evidence={1}' -f $capture.exitCode, $capture.evidence.directory)
        }
        $finishButtons = @(Find-UatWindowButton -Window $wizard.window -NamePattern '^(마침|완료|Finish)' -InnoRole Finish)
        if ($finishButtons.Count -eq 1) {
            $finishEvidence = Save-UatWindowEvidence -Window $wizard.window -Name ($Name + '-install-finished')
            [void]$evidence.Add($finishEvidence)
            if ($CancelDuringInstall) {
                throw 'The installer reached Finish before a verified partial-copy state was available for cancellation.'
            }
            if ($DeferFinish) {
                return [pscustomobject]@{ cancelled = $false; finishedPage = $true; deferred = $true; evidence = @($evidence.ToArray()); installEvidence = @($installEvidence.ToArray()); logPath = $wizard.logPath; wizard = $wizard; preRoot = $preRoot; preProfile = $preProfile }
            }
            $finish = Complete-UatSetupFinish -Wizard $wizard -Prefix $Name -Candidate $Candidate -LaunchAfterInstall:$LaunchAfterInstall
            return [pscustomobject]@{ cancelled = $false; finishedPage = $true; deferred = $false; evidence = @($evidence.ToArray()) + @($finish.finishScreenshot, $finish.setupEvidence); installEvidence = @($installEvidence.ToArray()); setupExitCode = $finish.setupExitCode; app = $finish.app; logPath = $finish.innoLog; preRoot = $preRoot; preProfile = $preProfile }
        }
        if ($CancelDuringInstall) {
            $presence = @(Get-UatPayloadPresence -Root $Candidate.installRoot)
            $presentCount = @($presence | Where-Object { $_.exists }).Count
            if ($presentCount -gt 0 -and $presentCount -lt $presence.Count) {
                $partialObserved = $true
                $partialState = Join-Path $providedEvidencePath ($Name + '-partial-copy.json')
                Write-UatJsonFile -Path $partialState -Value @{ expectedEntries = $presence.Count; presentEntries = $presentCount; observedUtc = [DateTime]::UtcNow.ToString('o'); presence = $presence }
                [void]$installEvidence.Add($partialState)
                [void]$installEvidence.Add((Save-UatWindowEvidence -Window $wizard.window -Name ($Name + '-partial-copy-window')))
                [void](Wait-UatWindowButton -Window $wizard.window -NamePattern '^(취소|Cancel)' -InnoRole Cancel -TimeoutSeconds 8)
                $cancel = Confirm-UatWizardCancellation -Wizard $wizard -Prefix ($Name + '-during-copy')
                return [pscustomobject]@{ cancelled = $true; phase = 'after-quarantine-partial-copy'; exitCode = $cancel.exitCode; evidence = @($evidence.ToArray()) + @($installEvidence.ToArray()) + @($cancel.evidence); logPath = $cancel.logPath; wizard = $wizard; preRoot = $preRoot; preProfile = $preProfile; partialObserved = $partialObserved }
            }
        }
        $remainingMilliseconds = Get-UatCompletionRemainingMilliseconds -CompletionBudget $installBudget
        if ($remainingMilliseconds -gt 0) { Start-Sleep -Milliseconds ([int][Math]::Min(120, $remainingMilliseconds)) }
    }
    Save-UatProcessPartial -Entry $wizard.process
    throw ('Timed out waiting for setup wizard to finish: {0}' -f $Name)
}

function Read-UatRawLog {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw ('Expected native log file was not written: {0}' -f $Path) }
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $decoded = Get-UatStrictDecodedText -Bytes $bytes
    return [pscustomobject]@{ bytes = $bytes.Length; sha256 = (Get-UatFileHash $Path); encoding = $decoded.encoding; text = $decoded.text }
}

function Save-UatFileFixtureState {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Name)
    Assert-NoReparseComponents -Path $Path
    $item = Get-Item -LiteralPath $Path -Force
    Assert-Uat (-not $item.PSIsContainer) ('Expected a regular test fixture file: {0}' -f $Path)
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { throw ('Refusing to back up a reparse file: {0}' -f $Path) }
    $backupPath = Join-Path $providedEvidencePath ($Name + '.fixture-backup.bin')
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    [System.IO.File]::WriteAllBytes($backupPath, $bytes)
    $streams = New-Object System.Collections.Generic.List[object]
    foreach ($stream in @(Get-Item -LiteralPath $Path -Stream * -ErrorAction Stop | Where-Object { $_.Stream -ne '$DATA' })) {
        $streamPath = $Path + ':' + [string]$stream.Stream
        $streamBytes = [System.IO.File]::ReadAllBytes($streamPath)
        $streamBackup = Join-Path $providedEvidencePath ($Name + '.stream-' + ([string]$stream.Stream -replace '[^A-Za-z0-9._-]', '-') + '.bin')
        [System.IO.File]::WriteAllBytes($streamBackup, $streamBytes)
        [void]$streams.Add([ordered]@{ name = [string]$stream.Stream; path = $streamBackup; sha256 = (Get-UatFileHash $streamBackup); bytes = $streamBytes.Length })
    }
    return [ordered]@{
        path = [System.IO.Path]::GetFullPath($Path)
        backup = [System.IO.Path]::GetFullPath($backupPath)
        sha256 = Get-UatFileHash $backupPath
        bytes = $bytes.Length
        attributes = [string]$item.Attributes
        creationUtc = $item.CreationTimeUtc.ToString('o')
        lastWriteUtc = $item.LastWriteTimeUtc.ToString('o')
        ownerDaclSddl = Get-UatAclSddl -Path $Path
        streams = @($streams.ToArray())
    }
}

function Restore-UatFileFixtureState {
    param(
        [Parameter(Mandatory = $true)]$State,
        [Parameter(Mandatory = $true)]$RecoveryRecord
    )
    if (-not (Test-UatNativeRecoverySafe -RecoveryName ([string]$RecoveryRecord.name) -RecoveryRecord $RecoveryRecord)) {
        return [bool]$false
    }
    try {
        $path = [System.IO.Path]::GetFullPath([string]$State.path)
        Assert-NoReparseComponents -Path $path
        Assert-Uat ((Get-UatFileHash $State.backup) -eq [string]$State.sha256) ('Fixture recovery backup hash changed: {0}' -f $State.backup)
        $current = Get-Item -LiteralPath $path -Force
        if (($current.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { throw ('Fixture restore refused a reparse path: {0}' -f $path) }
        [System.IO.File]::SetAttributes($path, [System.IO.FileAttributes]::Normal)
        [System.IO.File]::WriteAllBytes($path, [System.IO.File]::ReadAllBytes([string]$State.backup))
        foreach ($stream in @($State.streams)) {
            Assert-Uat ((Get-UatFileHash $stream.path) -eq [string]$stream.sha256) ('Stream recovery backup hash changed: {0}' -f $stream.path)
            [System.IO.File]::WriteAllBytes(($path + ':' + [string]$stream.name), [System.IO.File]::ReadAllBytes([string]$stream.path))
        }
        $acl = Get-Acl -LiteralPath $path
        $acl.SetSecurityDescriptorSddlForm([string]$State.ownerDaclSddl, [System.Security.AccessControl.AccessControlSections]::All)
        [void](Set-Acl -LiteralPath $path -AclObject $acl)
        [System.IO.File]::SetCreationTimeUtc($path, [DateTime]::Parse([string]$State.creationUtc).ToUniversalTime())
        [System.IO.File]::SetLastWriteTimeUtc($path, [DateTime]::Parse([string]$State.lastWriteUtc).ToUniversalTime())
        $restoredAttributes = [System.IO.FileAttributes][Enum]::Parse([System.IO.FileAttributes], [string]$State.attributes)
        [System.IO.File]::SetAttributes($path, $restoredAttributes)
        Assert-Uat ((Get-UatFileHash $path) -eq [string]$State.sha256) ('Fixture bytes did not restore exactly: {0}' -f $path)
        $RecoveryRecord.status = 'recovered'
        $RecoveryRecord.reason = $null
        $RecoveryRecord.completedUtc = [DateTime]::UtcNow.ToString('o')
    }
    catch {
        $RecoveryRecord.status = 'recoverypending'
        $RecoveryRecord.reason = 'file fixture recovery failed'
        $RecoveryRecord.cleanupError = $_
        throw
    }
    return [bool]$true
}

function Get-UatUninstallerPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($script:registryKey, $false)
    if ($null -eq $key) { throw 'The expected HKCU test uninstall entry is missing.' }
    try {
        $value = [string]$key.GetValue('UninstallString', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    }
    finally { $key.Dispose() }
    $match = [Regex]::Match($value, '^\s*"(?<path>[^"]+unins000\.exe)"(?:\s|$)|^\s*(?<path>\S+unins000\.exe)(?:\s|$)', [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)
    Assert-Uat $match.Success 'Registered UninstallString did not resolve to unins000.exe.'
    $path = [System.IO.Path]::GetFullPath($match.Groups['path'].Value)
    Assert-Uat ([string]::Equals($path, [System.IO.Path]::GetFullPath((Join-Path $script:testRoot 'unins000.exe')), [System.StringComparison]::OrdinalIgnoreCase)) 'Registered UninstallString points outside the owned test root.'
    return $path
}

function Start-UatUninstallerGui {
    param([Parameter(Mandatory = $true)][string]$Name, [int]$TimeoutSeconds = 30, $CompletionBudget = $null)
    $image = Get-UatUninstallerPath
    $log = Join-Path $providedEvidencePath ($Name + '.uninstall.log')
    $budget = $CompletionBudget
    if ($null -eq $budget) {
        $budget = New-UatCompletionBudget -TotalMilliseconds ([long]$TimeoutSeconds * [long]1000) -Origin ($Name + ' registered uninstaller GUI launch and window')
    }
    $process = Start-UatProcess -Executable $image -Argument @('/LOG=' + $log) -WorkingDirectory $script:testRoot -Name $Name -TrackNativeEngines -NativeAppRoot $script:testRoot -CompletionBudget $budget
    $window = Get-UatOwnedWindow -ProcessEntry $process -CompletionBudget $budget
    $summary = Get-UatWindowSummaryText -Window $window
    Assert-Uat ($summary -match 'SDIR|제거|삭제|uninstall|remove') 'The actual registered uninstaller did not display its owned confirmation UI.'
    $screenshot = Save-UatWindowEvidence -Window $window -Name ($Name + '-confirmation')
    return [pscustomobject]@{ process = $process; window = $window; logPath = $log; screenshot = $screenshot; imagePath = $image }
}

function Resolve-UatUninstallerConfirmation {
    param([Parameter(Mandatory = $true)]$Uninstaller, [Parameter(Mandatory = $true)][ValidateSet('cancel', 'remove')][string]$Choice)
    $entry = $Uninstaller.process
    $completionBudget = $entry.completionBudget
    $window = $Uninstaller.window
    $identity = $window.nativeIdentity
    if ($null -eq $identity -or $window.nativeWindowRole -cne 'main' -or $window.ownerWindowHandle -ne [IntPtr]::Zero -or
        -not [object]::ReferenceEquals($window.nativeProcessEntry, $entry) -or
        [object]::ReferenceEquals($identity, $entry.nativeLauncher) -or
        $identity.parentProcessId -ne $entry.nativeLauncher.processId -or
        $identity.parentStartTicks -ne $entry.nativeLauncher.startTicks -or
        [IO.Path]::GetFileName($identity.imagePath) -ine '_unins.tmp' -or
        -not [string]::Equals($entry.nativeLauncher.imagePath, $Uninstaller.imagePath, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The uninstall confirmation does not belong to the pinned second-phase registered uninstaller.'
    }
    $api = [Sdir.Installer.Uat.NativeWindowApi]
    $role = if ($Choice -eq 'cancel') { 'No' } else { 'Yes' }
    $id = if ($role -ceq 'No') { 7 } else { 6 }
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    $target = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        $remaining = Get-UatCompletionRemainingMilliseconds -CompletionBudget $completionBudget
        if ($remaining -le 0) { break }
        Assert-UatOwnedWindow -Window $window -ProcessEntry $entry
        $choices = @($api::OwnedInnoUninstallChoices($window.handle, $identity.safeHandle,
            [uint32]$window.processId, [long]$window.startTicks, [string]$window.imagePath))
        if ($choices.Count -eq 2) {
            $target = $choices[$id - 6]
            break
        }
        Start-Sleep -Milliseconds ([int][Math]::Min(150, $remaining))
    }
    if ($null -eq $target) { throw 'Timed out waiting for exact native Yes and No controls on the owned uninstall confirmation.' }
    Assert-UatOwnedWindow -Window $window -ProcessEntry $entry
    if (-not $api::PostOwnedInnoUninstallChoice($window.handle, $target.Handle, $identity.safeHandle,
            [uint32]$window.processId, [long]$window.startTicks, [string]$window.imagePath, $role, $id, $target.Title)) {
        throw ('The owned uninstall confirmation {0} control changed before native input or the click could not be queued.' -f $role)
    }
    Add-UatEvent -Kind 'native-ui-input' -Name 'uninstall confirmation' -Data @{
        processId = $window.processId; processStartTicks = $window.startTicks; imagePath = $window.imagePath
        windowHandle = $window.handle.ToInt64(); ownerWindowHandle = $window.ownerWindowHandle.ToInt64()
        caption = $window.window.Title; mechanism = 'PostMessage(WM_LBUTTONDOWN/UP)'
        role = $role; id = $id; targetHandle = $target.Handle.ToInt64(); targetCaption = $target.Title
    }
    if ($Choice -eq 'cancel') {
        return Wait-UatProcess -Entry $Uninstaller.process -TimeoutSeconds 90 -WaitReason 'uninstaller confirmation cancellation' -CompletionBudget $completionBudget
    }
    while ((Get-UatCompletionRemainingMilliseconds -CompletionBudget $completionBudget) -gt 0) {
        try {
            if (Test-UatProcessComplete -Entry $entry) { break }
            $engine = Get-UatNativeProcessObservation -Record $identity -ExpectedImagePath $identity.imagePath
            if ($engine.Signaled) { break }
            $dialog = $api::FindOwnedInnoUninstallCompletion($identity.safeHandle, [uint32]$identity.processId,
                [long]$identity.startTicks, [string]$identity.imagePath)
            if ($null -ne $dialog -and (Get-UatCompletionRemainingMilliseconds -CompletionBudget $completionBudget) -gt 0 -and
                $api::PostOwnedInnoUninstallCompletion($dialog, $identity.safeHandle, [uint32]$identity.processId,
                    [long]$identity.startTicks, [string]$identity.imagePath)) {
                Add-UatEvent -Kind 'native-ui-input' -Name 'uninstall completion' -Data @{
                    processId = $identity.processId; processStartTicks = $identity.startTicks; imagePath = $identity.imagePath
                    windowHandle = $dialog.DialogHandle.ToInt64(); ownerWindowHandle = $dialog.OwnerHandle.ToInt64()
                    caption = $dialog.Caption; message = $dialog.Message; messageHandle = $dialog.MessageHandle.ToInt64()
                    mechanism = 'PostMessage(WM_LBUTTONDOWN/UP)'; role = 'OK'; id = 1
                    targetHandle = $dialog.ButtonHandle.ToInt64(); targetCaption = $dialog.ButtonCaption
                }
                break
            }
        }
        catch {
            $completionFailure = $_.Exception.ToString()
            try { Save-UatProcessPartial -Entry $Uninstaller.process -Diagnostic $completionFailure }
            catch { throw ('Uninstaller completion observation failed; partial save also failed: {0}; original: {1}' -f $_.Exception.ToString(), $completionFailure) }
            throw ('Uninstaller completion observation failed: {0}' -f $completionFailure)
        }
        $remainingMilliseconds = Get-UatCompletionRemainingMilliseconds -CompletionBudget $completionBudget
        if ($remainingMilliseconds -gt 0) { Start-Sleep -Milliseconds ([int][Math]::Min(200, $remainingMilliseconds)) }
    }
    return Wait-UatProcess -Entry $Uninstaller.process -TimeoutSeconds 90 -WaitReason 'registered native uninstall completion' -CompletionBudget $completionBudget
}

function Assert-UatNoRunningTestApp {
    param([Parameter(Mandatory = $true)][string]$Root)
    $image = Join-Path $Root 'SDIR.exe'
    $remaining = @(Get-Process -Name 'SDIR' -ErrorAction SilentlyContinue | ForEach-Object {
        $id = [int]$_.Id
        try { [Sdir.Installer.Uat.NativeWindowApi]::ProcessImagePath($id) }
        catch { '' }
        finally { $_.Dispose() }
    } | Where-Object { [string]::Equals([System.IO.Path]::GetFullPath($_), [System.IO.Path]::GetFullPath($image), [System.StringComparison]::OrdinalIgnoreCase) })
    Assert-Uat ($remaining.Count -eq 0) ('A test SDIR process remains active at the owned install root: {0}' -f $Root)
}

function Assert-UatExpectedRefusal {
    param([Parameter(Mandatory = $true)]$Result, [Parameter(Mandatory = $true)][string]$Name)
    Assert-Uat ([int]$Result.exitCode -ne 0) ('Expected refusal/fault did not return a nonzero process exit code: {0}' -f $Name)
    Add-UatEvent -Kind 'expected-nonzero-asserted' -Name $Name -Data @{ exitCode = [int]$Result.exitCode; commandId = $Result.capture.ProcessId; expectedFaultIsCasePass = $true }
}

function Test-UatReceiptHistoricalEntry {
    param([Parameter(Mandatory = $true)]$Candidate, [Parameter(Mandatory = $true)][string]$Root)
    $expectedHash = 'ea35e4c2c9646c4b489140707c1b87963d7f7912c85a6fc2e914b490fa818607'
    if (-not [bool]$Candidate.testBaseline -or -not [bool]$Candidate.receipt.testBuild -or -not [string]::Equals([string]$Candidate.receipt.activeInventoryId, 'test-baselineb158-v1', [System.StringComparison]::Ordinal)) { return $false }
    $candidateEntries = @($Candidate.receipt.payloadInventory | Where-Object { [string]::Equals([string]$_.destination, 'legacy-test-only.txt', [System.StringComparison]::Ordinal) })
    if ($candidateEntries.Count -ne 1) { return $false }
    if ([long]$candidateEntries[0].bytes -ne 25 -or -not [string]::Equals([string]$candidateEntries[0].sha256, $expectedHash, [System.StringComparison]::Ordinal)) { return $false }
    $legacyPath = Join-Path $Root 'legacy-test-only.txt'
    $receiptPath = Join-Path $Root 'sdir.install-receipt'
    if (-not (Test-Path -LiteralPath $legacyPath -PathType Leaf) -or -not (Test-Path -LiteralPath $receiptPath -PathType Leaf)) { return $false }
    if ([long](Get-Item -LiteralPath $legacyPath).Length -ne 25) { return $false }
    $actualHash = Get-UatFileHash $legacyPath
    $receiptLines = [System.IO.File]::ReadAllLines($receiptPath)
    if ($receiptLines.Count -eq 0 -or -not [string]::Equals([string]$receiptLines[0], 'SDIR-RECEIPT-V2', [System.StringComparison]::Ordinal)) { return $false }
    $inventoryLines = @($receiptLines | Where-Object { $_ -match '^Inventory' })
    if ($inventoryLines.Count -ne 1) { return $false }
    $inventoryParts = $inventoryLines[0].Split('|')
    if ($inventoryParts.Count -ne 2 -or -not [string]::Equals([string]$inventoryParts[0], 'Inventory', [System.StringComparison]::Ordinal) -or -not [string]::Equals([string]$inventoryParts[1], 'test-baselineb158-v1', [System.StringComparison]::Ordinal)) { return $false }
    $historyLines = @($receiptLines | Where-Object { $_ -match '^History(\||$)' })
    if ($historyLines.Count -ne 0) { return $false }
    $legacyRecords = @($receiptLines | Where-Object { $_ -match '^Payload\|legacy-test-only\.txt(?:\||$)' })
    $expectedRecord = 'Payload|legacy-test-only.txt|' + $expectedHash
    return ([string]::Equals($actualHash, $expectedHash, [System.StringComparison]::Ordinal) -and $legacyRecords.Count -eq 1 -and [string]::Equals([string]$legacyRecords[0], $expectedRecord, [System.StringComparison]::Ordinal))
}

function Get-UatRuntimeRegistryEvidence {
    $guid = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    $subKey = 'Software\Microsoft\EdgeUpdate\Clients\' + $guid
    $rows = New-Object System.Collections.Generic.List[object]
    foreach ($hive in @([Microsoft.Win32.RegistryHive]::LocalMachine, [Microsoft.Win32.RegistryHive]::CurrentUser)) {
        foreach ($view in @([Microsoft.Win32.RegistryView]::Registry32, [Microsoft.Win32.RegistryView]::Registry64)) {
            $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, $view)
            try {
                $key = $base.OpenSubKey($subKey, $false)
                if ($null -eq $key) {
                    [void]$rows.Add([ordered]@{ hive = [string]$hive; view = [string]$view; exists = $false; version = $null; valid = $false })
                    continue
                }
                try {
                    $version = [string]$key.GetValue('pv', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                    $parts = $version.Split('.')
                    $valid = $parts.Count -eq 4
                    $nonZero = $false
                    foreach ($part in $parts) {
                        $number = 0
                        if ($part -notmatch '^\d+$' -or -not [int]::TryParse($part, [ref]$number) -or $number -lt 0 -or $number -gt 65535) { $valid = $false; break }
                        if ($number -gt 0) { $nonZero = $true }
                    }
                    $valid = $valid -and $nonZero
                    [void]$rows.Add([ordered]@{ hive = [string]$hive; view = [string]$view; exists = $true; version = $version; valid = $valid })
                }
                finally { $key.Dispose() }
            }
            finally { $base.Dispose() }
        }
    }
    $validRows = @($rows.ToArray() | Where-Object { $_.valid })
    return [ordered]@{ productGuid = $guid; registryPath = $subKey; present = ($validRows.Count -gt 0); matchingViews = $validRows; examinedViews = @($rows.ToArray()) }
}

function Set-UatCandidateExecuted {
    param([Parameter(Mandatory = $true)][string]$Name)
    foreach ($record in $script:candidateRecords) {
        if ($record.name -eq $Name) { $record.runtimeExecuted = $true; return }
    }
}

function Test-UatMissingRuntimeBranch {
    $candidate = $script:builds['missing']
    Assert-Uat ($candidate.testMissingRuntime -and $candidate.receipt.testBuild) 'Missing-runtime candidate was not compile-time test-only.'
    $beforeRoot = Get-UatTreeSnapshot -Path $script:testRoot
    $beforeRegistry = Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey
    $beforeStart = Get-UatTreeSnapshot -Path $script:startMenuShortcut
    $beforeDesktop = Get-UatTreeSnapshot -Path $script:desktopShortcut
    $beforeProfile = Get-UatTreeSnapshot -Path $script:profileRoot
    $logPath = Join-Path $providedEvidencePath 'missing-runtime-branch.setup.log'
    $budget = New-UatCompletionBudget -TotalMilliseconds ([long]90000) -Origin 'missing-runtime refusal'
    $process = Start-UatProcess -Executable $candidate.path -Argument @('/LOG=' + $logPath) -WorkingDirectory $repoRoot -Name 'missing-runtime-branch-gui' -TrackNativeEngines -NativeAppRoot $candidate.installRoot -CompletionBudget $budget
    Set-UatCandidateExecuted -Name 'stable-missing-runtime'
    $window = Get-UatOwnedWindow -ProcessEntry $process -CompletionBudget $budget
    $messageWindow = $window
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    $visibleText = ''
    while ([DateTime]::UtcNow -lt $deadline -and (Get-UatCompletionRemainingMilliseconds -CompletionBudget $budget) -gt 0) {
        $modal = Get-UatOwnedModalWindow -MainWindow $window -ProcessEntry $process -TimeoutSeconds 1 -CompletionBudget $budget
        if ($modal) { $messageWindow = $modal }
        $visibleText = Get-UatWindowSummaryText -Window $messageWindow
        if ($visibleText -match 'WebView2|WebView 2|웹뷰|Microsoft Edge') { break }
        Start-Sleep -Milliseconds 150
    }
    Assert-Uat ($visibleText -match 'WebView2|WebView 2|Microsoft Edge') 'The missing-runtime branch did not display its actual WebView2 prerequisite message.'
    Assert-Uat ($visibleText -match '필요|설치|안내|runtime|Runtime') 'The missing-runtime message did not explain the prerequisite.'
    $screenshot = Save-UatWindowEvidence -Window $messageWindow -Name 'missing-runtime-branch-message'
    $uiaPath = Join-Path $providedEvidencePath 'missing-runtime-branch-uia.json'
    Write-UatJsonFile -Path $uiaPath -Value (Get-UatWindowSummary -Window $messageWindow)
    $confirmButtons = @(Find-UatWindowButton -Window $messageWindow -NamePattern '^(확인|OK|닫기|Close)')
    if ($confirmButtons.Count -eq 1) { [void](Invoke-UatWindowButton -Window $messageWindow -NamePattern '^(확인|OK|닫기|Close)') }
    $run = Wait-UatProcess -Entry $process -TimeoutSeconds 45 -WaitReason 'missing-runtime refusal' -CompletionBudget $budget
    Assert-UatExpectedRefusal -Result $run -Name 'missing-runtime-branch'
    Test-UatSnapshotEqual -Expected $beforeRoot -Actual (Get-UatTreeSnapshot -Path $script:testRoot) -Message 'Missing-runtime setup changed the test installation root.'
    Test-UatSnapshotEqual -Expected $beforeRegistry -Actual (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey) -Message 'Missing-runtime setup changed the test HKCU registration.'
    Test-UatSnapshotEqual -Expected $beforeStart -Actual (Get-UatTreeSnapshot -Path $script:startMenuShortcut) -Message 'Missing-runtime setup changed the Start Menu shortcut.'
    Test-UatSnapshotEqual -Expected $beforeDesktop -Actual (Get-UatTreeSnapshot -Path $script:desktopShortcut) -Message 'Missing-runtime setup changed the desktop shortcut.'
    Test-UatSnapshotEqual -Expected $beforeProfile -Actual (Get-UatTreeSnapshot -Path $script:profileRoot) -Message 'Missing-runtime setup changed the isolated profile.'
    $nativeLog = Read-UatRawLog -Path $logPath
    Assert-Uat ($nativeLog.text.Contains('SDIR_WEBVIEW2_MISSING')) 'The missing-runtime case did not produce the source event marker.'
    return @($screenshot, $uiaPath, $logPath, $run.evidence.directory)
}

function Test-UatA7BaselineUpgrade {
    $baseline = $script:builds['baseline']
    $stable = $script:builds['stable']
    Assert-Uat (-not (Test-Path -LiteralPath $baseline.installRoot)) 'The unique B158 baseline root already exists.'
    $profileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    $install = Invoke-UatWizardInstall -Candidate $baseline -Name 'A7-b158-baseline-install' -DesktopShortcut:$false
    Assert-Uat ($install.setupExitCode -eq 0) 'The real B158 TestBuild baseline installer did not finish successfully.'
    $baselineReceipt = Assert-UatInstalledPayload -Candidate $baseline -Root $baseline.installRoot -DesktopExpected:$false
    Test-UatSnapshotEqual -Expected $profileBefore -Actual (Get-UatTreeSnapshot -Path $script:profileRoot) -Message 'The B158 setup changed profile bytes before first app launch.'
    $baselineEvidence = @($install.evidence) + @($install.logPath) + @($install.installEvidence) + @((Join-Path $baseline.installRoot 'sdir.install-receipt'))
    if (-not (Test-UatReceiptHistoricalEntry -Candidate $baseline -Root $baseline.installRoot)) {
        $gap = 'The current TestBaseline B158 installer did not install legacy-test-only.txt with a matching Payload receipt record. The harness did not synthesize a file or forge receipt ownership; A7 stopped before attempting the stable upgrade.'
        [void]$script:sourceGaps.Add($gap)
        Add-UatEvent -Kind 'source-testability-gap' -Name 'A7' -Data @{ gap = $gap; testBaselineCandidate = $baseline.path; installRoot = $baseline.installRoot; receipt = (Join-Path $baseline.installRoot 'sdir.install-receipt'); legacyPath = (Join-Path $baseline.installRoot 'legacy-test-only.txt') }
        Set-UatCase -Id 'A7' -Status 'pending' -Evidence $baselineEvidence -Details @($gap)
        throw ('SOURCE_GAP: ' + $gap)
    }
    $legacyPath = Join-Path $baseline.installRoot 'legacy-test-only.txt'
    $script:legacyBeforeUpgradeHash = Get-UatFileHash $legacyPath
    $script:baselineRootMarker = Write-UatRootOwnerMarker -Root $baseline.installRoot -Purpose 'baseline-upgrade-test-root'
    $script:unknownSentinelPath = Join-Path $baseline.installRoot 'uat-unknown-sentinel.bin'
    $script:dataSentinelPath = Join-Path $baseline.installRoot 'data\uat-user-data-sentinel.bin'
    Assert-Uat (-not (Test-Path -LiteralPath $script:unknownSentinelPath) -and -not (Test-Path -LiteralPath $script:dataSentinelPath)) 'A7 sentinel path unexpectedly exists.'
    New-Item -ItemType Directory -Path (Split-Path -Parent $script:dataSentinelPath) | Out-Null
    $sentinelBytes = [System.Text.Encoding]::UTF8.GetBytes('SDIR-UAT-UNKNOWN-' + $script:runId)
    $dataBytes = [System.Text.Encoding]::UTF8.GetBytes('SDIR-UAT-DATA-' + $script:runId)
    [System.IO.File]::WriteAllBytes($script:unknownSentinelPath, $sentinelBytes)
    [System.IO.File]::WriteAllBytes($script:dataSentinelPath, $dataBytes)
    $unknownHash = Get-UatFileHash $script:unknownSentinelPath
    $dataHash = Get-UatFileHash $script:dataSentinelPath
    $preUpgrade = Get-UatInstallSnapshot -Root $baseline.installRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    $profileBeforeUpgrade = Get-UatTreeSnapshot -Path $script:profileRoot
    $historicalLock = New-Object System.IO.FileStream -ArgumentList @($legacyPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
    try {
        $lockRefusal = Invoke-UatSilentExpectedRefusal -Candidate $stable -Name 'A9-exclusive-lock-historical-owned-file' -ExpectedLogPattern 'SDIR_PREFLIGHT_REFUSED'
        Test-UatSnapshotEqual -Expected $preUpgrade -Actual (Get-UatInstallSnapshot -Root $baseline.installRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message 'The obsolete-owned-file lock refusal changed the baseline installation.'
    }
    finally { $historicalLock.Dispose() }
    $upgrade = Invoke-UatWizardInstall -Candidate $stable -Name 'A7-stable-upgrade' -DesktopShortcut:$false
    Assert-Uat ($upgrade.setupExitCode -eq 0) 'Stable TestBuild upgrade from the real B158 install did not finish successfully.'
    $stableReceipt = Assert-UatInstalledPayload -Candidate $stable -Root $baseline.installRoot -DesktopExpected:$false
    Assert-Uat (-not (Test-Path -LiteralPath $legacyPath)) 'The obsolete historically owned test file remains after the stable upgrade.'
    Assert-Uat ((Get-UatFileHash $script:unknownSentinelPath) -eq $unknownHash) 'The unknown non-owned sentinel changed during upgrade.'
    Assert-Uat ((Get-UatFileHash $script:dataSentinelPath) -eq $dataHash) 'The user-data sentinel changed during upgrade.'
    Test-UatSnapshotEqual -Expected $profileBeforeUpgrade -Actual (Get-UatTreeSnapshot -Path $script:profileRoot) -Message 'The stable upgrade modified isolated profile bytes.'
    $upgradeLog = Read-UatRawLog -Path $upgrade.logPath
    $quarantineNeedle = 'SDIR_QUARANTINED: program\legacy-test-only.txt'
    $quarantineIndex = $upgradeLog.text.IndexOf($quarantineNeedle, [System.StringComparison]::Ordinal)
    $replacementMatch = [Regex]::Match($upgradeLog.text, '(?im)^.*(?:Installing|Extracting)(?: file)?:?.*SDIR\.exe.*$')
    Assert-Uat ($quarantineIndex -ge 0 -and $replacementMatch.Success -and $quarantineIndex -lt $replacementMatch.Index) 'The native setup log did not prove historical quarantine preceded replacement; final hashes alone are insufficient.'
    $orderEvidence = Join-Path $providedEvidencePath 'A7-upgrade-order.json'
    Write-UatJsonFile -Path $orderEvidence -Value ([ordered]@{
        baselineCandidate = $baseline.path
        baselineEmbeddedAppSha256 = $baseline.receipt.embeddedAppSha256
        historicalPath = $legacyPath
        historicalSha256BeforeUpgrade = $script:legacyBeforeUpgradeHash
        baselineReceiptEvidence = (Join-Path $baseline.installRoot 'sdir.install-receipt')
        quarantineLogIndex = $quarantineIndex
        firstSdirExeInstallLogIndex = $replacementMatch.Index
        obsoletePathAbsentAfterUpgrade = $true
        stableInstallEvidence = $stableReceipt
        unknownSentinelSha256 = $unknownHash
        dataSentinelSha256 = $dataHash
        priorRootSnapshot = $preUpgrade
        innoLogSha256 = $upgradeLog.sha256
        innoLogEncoding = $upgradeLog.encoding
    })
    $script:testRoot = $baseline.installRoot
    $script:testRootName = $baseline.rootName
    if ($script:caseResults['A9'].status -eq 'pending' -and $script:a9CoreVerified) {
        Set-UatCase -Id 'A9' -Status 'verified' -Evidence (@($script:a9Evidence) + @($lockRefusal.evidence) + @($upgrade.evidence) + @($upgrade.logPath)) -Details @('A9 external locks on executable, document, obsolete receipt-owned historical file, and native uninstall metadata each refused safely; release allowed retry and upgrade.')
    }
    Set-UatCase -Id 'A7' -Status 'verified' -Evidence (@($baselineEvidence) + @($upgrade.evidence) + @($upgrade.logPath, $orderEvidence, $script:baselineRootMarker, $lockRefusal.evidence)) -Details @('B158 source remained test-only; the baseline receipt owned the historical path; the file was exclusively locked and refused before retry; final stable payload hashes, obsolete removal, and user sentinels were checked.')
    return @($baselineEvidence) + @($upgrade.evidence) + @($upgrade.logPath, $orderEvidence)
}

function Test-UatA2FreshCancel {
    $candidate = $script:builds['fresh']
    Assert-Uat (-not (Test-Path -LiteralPath $candidate.installRoot)) 'A2 cancellation target was not a fresh owned test root.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A2 cancellation requires no active test registration.'
    $profileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    $cancel = Invoke-UatWizardInstall -Candidate $candidate -Name 'A2-fresh-desktop-off-cancel' -DesktopShortcut:$false -CancelAtReady
    Assert-Uat ($cancel.cancelled -and $cancel.phase -eq 'before-mutation') 'The setup wizard did not cancel at the ready page before installation.'
    Assert-Uat (-not (Test-Path -LiteralPath $candidate.installRoot)) 'Fresh cancellation created a test install root.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'Fresh cancellation left a test uninstall registration.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'Fresh cancellation left a test shortcut.'
    Test-UatSnapshotEqual -Expected $profileBefore -Actual (Get-UatTreeSnapshot -Path $script:profileRoot) -Message 'Fresh cancellation modified the isolated profile.'
    if ($cancel.exitCode -eq 0) {
        Add-UatEvent -Kind 'user-cancel-exit' -Name 'A2-cancel-before-mutation' -Data @{ exitCode = 0; actualRootAbsent = $true; registrationAbsent = $true; shortcutsAbsent = $true }
    }
    $script:a2CancelEvidence = @($cancel.evidence)
    return $script:a2CancelEvidence
}

function Test-UatA3FreshInstallAndA4A5Launches {
    $candidate = $script:builds['fresh']
    Assert-Uat (-not (Test-Path -LiteralPath $candidate.installRoot)) 'A3 fresh install target is not absent after cancellation.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A3 fresh install must begin without a test registry entry.'
    $profileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    $deferred = Invoke-UatWizardInstall -Candidate $candidate -Name 'A3-fresh-desktop-on-install' -DesktopShortcut:$true -DeferFinish
    Assert-Uat $deferred.finishedPage 'Fresh install did not reach the actual finish page.'
    Test-UatSnapshotEqual -Expected $profileBefore -Actual (Get-UatTreeSnapshot -Path $script:profileRoot) -Message 'Setup modified raw profile bytes before the first app launch.'
    $installed = Assert-UatInstalledPayload -Candidate $candidate -Root $candidate.installRoot -DesktopExpected:$true
    $rootMarker = Write-UatRootOwnerMarker -Root $candidate.installRoot -Purpose 'fresh-installed-app-journeys'
    $installedEvidence = Join-Path $providedEvidencePath 'A3-fresh-installed-state.json'
    Write-UatJsonFile -Path $installedEvidence -Value $installed
    $launcherPath = Join-Path $candidate.installRoot 'sdir-installer-test-launcher.cmd'
    $launcherText = [System.IO.File]::ReadAllText($launcherPath, [System.Text.Encoding]::ASCII)
    Assert-Uat ($launcherText.Contains(('set "SDIR_PROFILE_DIR={0}"' -f $script:profileRoot)) -and $launcherText.Contains(('set "SDIR_DEBUG_PORT={0}"' -f $script:debugPort))) 'Installed shortcuts do not enforce the isolated profile and debug port.'
    Assert-Uat ($launcherText.Contains('"%~dp0SDIR.exe" %*')) 'Installed test launcher does not invoke the adjacent test executable.'
    $finish = Complete-UatSetupFinish -Wizard $deferred.wizard -Prefix 'A3-fresh-desktop-on-install' -Candidate $candidate -LaunchAfterInstall:$true
    Assert-Uat ($finish.setupExitCode -eq 0) 'Fresh installation did not exit successfully after the Finish action.'
    $script:testRoot = $candidate.installRoot
    $script:testRootName = $candidate.rootName
    $wizardApp = $finish.app
    $wizardEvidence = Test-UatLaunchRoute -App $wizardApp -Name 'A4-wizard-launch'

    $directApp = Start-UatDirectApp -Root $candidate.installRoot -Name 'A4-direct-launch'
    $directEvidence = Test-UatLaunchRoute -App $directApp -Name 'A4-direct-launch'

    $startApp = Start-UatAppFromShortcut -Root $candidate.installRoot -ShortcutPath $script:startMenuShortcut -Name 'A4-start-menu-launch'
    $startEvidence = Test-UatLaunchRoute -App $startApp -Name 'A4-start-menu-launch'

    $desktopApp = Start-UatAppFromShortcut -Root $candidate.installRoot -ShortcutPath $script:desktopShortcut -Name 'A4-desktop-launch'
    $desktopEvidence = Test-UatLaunchRoute -App $desktopApp -Name 'A4-desktop-launch'

    $a2Evidence = @($script:a2CancelEvidence) + @($deferred.evidence) + @($finish.finishScreenshot) + @($installedEvidence)
    $a3Evidence = @($deferred.evidence) + @($finish.finishScreenshot, $deferred.logPath, $installedEvidence)
    $allLaunchEvidence = @($wizardEvidence) + @($directEvidence) + @($startEvidence) + @($desktopEvidence)
    $script:a4Evidence = @($allLaunchEvidence)
    Set-UatCase -Id 'A2' -Status 'verified' -Evidence $a2Evidence -Details @('Desktop shortcut was verified unchecked during fresh cancellation and checked on a real fresh installation; wizard controls and visible screenshots were captured; the setup process token stayed unelevated.')
    Set-UatCase -Id 'A3' -Status 'verified' -Evidence (@($a3Evidence) + @($rootMarker)) -Details @('All stable payload bytes, ownership receipt, single HKCU registration, and Start Menu/desktop shortcut targets and working directories were inspected.')
    Set-UatCase -Id 'A4' -Status 'verified' -Evidence $allLaunchEvidence -Details @('Wizard, direct ProcessStartInfo with explicit SDIR_PROFILE_DIR, Start Menu Shell invocation, and desktop Shell invocation each opened the installed test app, exposed fixture listings, and persisted profile settings after WM_CLOSE.')
    Set-UatCase -Id 'A5' -Status 'verified' -Evidence $allLaunchEvidence -Details @('Raw profile bytes remained unchanged through setup; actual UI and persisted JSON checked two-pane layout, tabs, group names, workspace, theme, selected folder, list state, and fixture rows.')
    return @($a2Evidence) + @($a3Evidence) + @($rootMarker) + @($allLaunchEvidence)
}

function Write-UatRootOwnerMarker {
    param([Parameter(Mandatory = $true)][string]$Root, [Parameter(Mandatory = $true)][string]$Purpose)
    Assert-NoReparseComponents -Path $Root
    $markerPath = Join-Path $Root 'sdir.uat-owner.json'
    Assert-Uat (-not (Test-Path -LiteralPath $markerPath)) ('A UAT ownership marker already exists: {0}' -f $markerPath)
    Write-UatJsonFile -Path $markerPath -Value ([ordered]@{
        marker = 'SDIR-INSTALLER-UAT-ROOT-V1'
        runId = $script:runId
        purpose = $Purpose
        root = [System.IO.Path]::GetFullPath($Root)
        createdUtc = [DateTime]::UtcNow.ToString('o')
    })
    return [System.IO.Path]::GetFullPath($markerPath)
}

function Assert-UatRootOwnerMarker {
    param([Parameter(Mandatory = $true)][string]$Root)
    $markerPath = Join-Path $Root 'sdir.uat-owner.json'
    Assert-Uat (Test-Path -LiteralPath $markerPath -PathType Leaf) ('Owned root marker is missing: {0}' -f $Root)
    $marker = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
    Assert-Uat ($marker.marker -eq 'SDIR-INSTALLER-UAT-ROOT-V1' -and $marker.runId -eq $script:runId) ('Root marker does not belong to this harness run: {0}' -f $Root)
    Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$marker.root), [System.IO.Path]::GetFullPath($Root), [System.StringComparison]::OrdinalIgnoreCase)) ('Root marker points at a different path: {0}' -f $Root)
    return $markerPath
}

function Invoke-UatSilentSetup {
    param(
        [Parameter(Mandatory = $true)]$Candidate,
        [Parameter(Mandatory = $true)][string]$Name,
        [string[]]$AdditionalArgument = @(),
        [int]$TimeoutSeconds = 240
    )
    $logPath = Join-Path $providedEvidencePath ($Name + '.setup.log')
    $arguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', ('/LOG=' + $logPath)) + @($AdditionalArgument)
    $budget = New-UatCompletionBudget -TotalMilliseconds ([long]$TimeoutSeconds * [long]1000) -Origin ($Name + ' silent setup')
    $run = Invoke-UatCommand -Executable $Candidate.path -Argument $arguments -WorkingDirectory $repoRoot -Name $Name -TimeoutSeconds $TimeoutSeconds -TrackNativeEngines -NativeAppRoot $Candidate.installRoot -CompletionBudget $budget
    return [pscustomobject]@{ exitCode = $run.exitCode; evidence = $run.evidence.directory; logPath = $logPath; command = $run }
}

function Invoke-UatSilentUninstall {
    param([Parameter(Mandatory = $true)][string]$Name, [int]$TimeoutSeconds = 180)
    $uninstaller = Get-UatUninstallerPath
    $logPath = Join-Path $providedEvidencePath ($Name + '.uninstall.log')
    $budget = New-UatCompletionBudget -TotalMilliseconds ([long]$TimeoutSeconds * [long]1000) -Origin ($Name + ' silent uninstall')
    $run = Invoke-UatCommand -Executable $uninstaller -Argument @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', ('/LOG=' + $logPath)) -WorkingDirectory $script:testRoot -Name $Name -TimeoutSeconds $TimeoutSeconds -TrackNativeEngines -NativeAppRoot $script:testRoot -CompletionBudget $budget
    return [pscustomobject]@{ exitCode = $run.exitCode; evidence = $run.evidence.directory; logPath = $logPath; command = $run }
}

function Invoke-UatGuiExpectedRefusal {
    param(
        [Parameter(Mandatory = $true)]$Candidate,
        [Parameter(Mandatory = $true)][string]$Name,
        [string]$ExpectedTextPattern = '설치|SDIR|잠금|실행|경로|파일|거부|portable|WebView2',
        [int]$TimeoutSeconds = 25,
        [string[]]$AdditionalArgument = @()
    )
    $logPath = Join-Path $providedEvidencePath ($Name + '.setup.log')
    $refusalBudget = New-UatCompletionBudget -TotalMilliseconds ([long]$TimeoutSeconds * [long]1000) -Origin ($Name + ' setup refusal route')
    $wizard = Start-UatSetupGui -Candidate $Candidate -Name $Name -AdditionalArgument $AdditionalArgument -TimeoutSeconds $TimeoutSeconds -CompletionBudget $refusalBudget
    $entry = $wizard.process
    $window = $wizard.window
    $evidence = New-Object System.Collections.Generic.List[string]
    [void]$evidence.Add((Save-UatWindowEvidence -Window $window -Name ($Name + '-owned-window')))
    foreach ($path in (Move-UatWizardToOptions -Window $window -Prefix $Name)) { [void]$evidence.Add($path) }
    [void](Set-UatWindowCheckBox -Window $window -NamePattern '바탕 화면 바로 가기 추가' -Checked $false)
    foreach ($path in (Move-UatWizardToReady -Window $window -Prefix $Name)) { [void]$evidence.Add($path) }
    [void](Invoke-UatWindowButton -Window $window -NamePattern '^(설치|Install)' -InnoRole Install -AsyncPostMessage)
    $refusalText = ''
    $refusalObserved = $false
    while ((Get-UatCompletionRemainingMilliseconds -CompletionBudget $refusalBudget) -gt 0) {
        $complete = $false
        try { $complete = Test-UatProcessComplete -Entry $entry }
        catch {
            $completionFailure = $_.Exception.ToString()
            try { Save-UatProcessPartial -Entry $entry -Diagnostic $completionFailure }
            catch { throw ('Expected setup refusal observation failed; partial save also failed: {0}; original: {1}' -f $_.Exception.ToString(), $completionFailure) }
            throw ('Expected setup refusal observation failed: {0}' -f $completionFailure)
        }
        if ($complete) {
            $completed = Wait-UatProcess -Entry $entry -TimeoutSeconds $TimeoutSeconds -CompletionBudget $refusalBudget -WaitReason ($Name + ' refusal before completion')
            throw ('Expected refusal dialog did not appear before setup completed; exit={0}, evidence={1}' -f $completed.exitCode, $completed.evidence.directory)
        }
        $modal = Get-UatOwnedModalWindow -MainWindow $window -ProcessEntry $entry -TimeoutSeconds 1 -CompletionBudget $refusalBudget
        if ($modal) {
            $refusalText = Get-UatWindowSummaryText -Window $modal
            [void]$evidence.Add((Save-UatWindowEvidence -Window $modal -Name ($Name + '-refusal-dialog')))
            $uiaPath = Join-Path $providedEvidencePath ($Name + '-refusal-uia.json')
            Write-UatJsonFile -Path $uiaPath -Value (Get-UatWindowSummary -Window $modal)
            [void]$evidence.Add($uiaPath)
            Assert-Uat ($refusalText -match $ExpectedTextPattern) ('Owned refusal dialog did not explain the expected safety refusal: {0}' -f $Name)
            $ok = @(Find-UatWindowButton -Window $modal -NamePattern '^(확인|OK|닫기|Close)')
            if ($ok.Count -ne 1) { throw ('Refusal dialog has no unique owned confirmation button: {0}' -f $Name) }
            [void](Invoke-UatWindowButton -Window $modal -NamePattern '^(확인|OK|닫기|Close)')
            $refusalObserved = $true
            break
        }
        if (@(Find-UatWindowButton -Window $window -NamePattern '^(마침|완료|Finish)' -InnoRole Finish).Count -eq 1) {
            throw ('Installer reached Finish instead of refusing before mutation: {0}' -f $Name)
        }
        $remainingMilliseconds = Get-UatCompletionRemainingMilliseconds -CompletionBudget $refusalBudget
        if ($remainingMilliseconds -gt 0) { Start-Sleep -Milliseconds ([int][Math]::Min(120, $remainingMilliseconds)) }
    }
    if (-not $refusalObserved) {
        $completed = Wait-UatProcess -Entry $entry -TimeoutSeconds $TimeoutSeconds -CompletionBudget $refusalBudget -WaitReason ($Name + ' refusal completion')
        throw ('Expected refusal dialog was not observed before process completion; exit={0}, evidence={1}' -f $completed.exitCode, $completed.evidence.directory)
    }
    $run = Wait-UatProcess -Entry $entry -TimeoutSeconds $TimeoutSeconds -CompletionBudget $refusalBudget -WaitReason ($Name + ' refusal completion')
    Assert-UatExpectedRefusal -Result $run -Name $Name
    $native = Read-UatRawLog -Path $logPath
    $nativeEvidence = Join-Path $providedEvidencePath ($Name + '-native-log-summary.json')
    Write-UatJsonFile -Path $nativeEvidence -Value @{ sha256 = $native.sha256; encoding = $native.encoding; bytes = $native.bytes; text = $native.text }
    [void]$evidence.Add($logPath)
    [void]$evidence.Add($nativeEvidence)
    Add-UatEvent -Kind 'native-refusal-asserted' -Name $Name -Data @{ processId = $entry.processId; exitCode = $run.exitCode; visibleText = $refusalText; logSha256 = $native.sha256 }
    return [pscustomobject]@{ exitCode = $run.exitCode; evidence = @($evidence.ToArray()) + @($run.evidence.directory); logPath = $logPath; refusalText = $refusalText }
}

function Invoke-UatSilentExpectedRefusal {
    param(
        [Parameter(Mandatory = $true)]$Candidate,
        [Parameter(Mandatory = $true)][string]$Name,
        [string]$ExpectedLogPattern = 'SDIR_PREFLIGHT_REFUSED|SDIR_SETUP_MUTEX_REFUSED|SDIR_WEBVIEW2_MISSING|SDIR_UNINSTALL_REGISTRY_PREFLIGHT_REFUSED',
        [string[]]$AdditionalArgument = @()
    )
    $run = Invoke-UatSilentSetup -Candidate $Candidate -Name $Name -AdditionalArgument $AdditionalArgument
    Assert-UatExpectedRefusal -Result $run.command -Name $Name
    $native = Read-UatRawLog -Path $run.logPath
    Assert-Uat ($native.text -match $ExpectedLogPattern) ('Silent refusal log did not contain a source event matching {0}: {1}' -f $ExpectedLogPattern, $Name)
    $summary = Join-Path $providedEvidencePath ($Name + '-native-log-summary.json')
    Write-UatJsonFile -Path $summary -Value @{ sha256 = $native.sha256; encoding = $native.encoding; bytes = $native.bytes; matchedMarker = ([Regex]::Match($native.text, $ExpectedLogPattern).Value) }
    return [pscustomobject]@{ exitCode = $run.exitCode; evidence = @($run.evidence, $run.logPath, $summary); logPath = $run.logPath; nativeLog = $native }
}

function Invoke-UatGuiUninstallExpectedRefusal {
    param([Parameter(Mandatory = $true)][string]$Name, [int]$TimeoutSeconds = 20)
    $image = Get-UatUninstallerPath
    $logPath = Join-Path $providedEvidencePath ($Name + '.uninstall.log')
    $launchBudget = New-UatCompletionBudget -TotalMilliseconds ([long]$TimeoutSeconds * [long]1000) -Origin ($Name + ' registered uninstaller launch and window')
    $entry = Start-UatProcess -Executable $image -Argument @('/LOG=' + $logPath) -WorkingDirectory $script:testRoot -Name $Name -TrackNativeEngines -NativeAppRoot $script:testRoot -CompletionBudget $launchBudget
    $window = Get-UatOwnedWindow -ProcessEntry $entry -CompletionBudget $launchBudget
    $evidence = New-Object System.Collections.Generic.List[string]
    [void]$evidence.Add((Save-UatWindowEvidence -Window $window -Name ($Name + '-owned-window')))
    $confirm = @(Find-UatWindowButton -Window $window -NamePattern '^(예|제거|삭제|Yes|Uninstall|Remove)')
    $refusalBudget = $entry.completionBudget
    if ($confirm.Count -eq 1) { [void](Invoke-UatWindowButton -Window $window -NamePattern '^(예|제거|삭제|Yes|Uninstall|Remove)') }
    $message = ''
    while ((Get-UatCompletionRemainingMilliseconds -CompletionBudget $refusalBudget) -gt 0) {
        $complete = $false
        try { $complete = Test-UatProcessComplete -Entry $entry }
        catch {
            $completionFailure = $_.Exception.ToString()
            try { Save-UatProcessPartial -Entry $entry -Diagnostic $completionFailure }
            catch { throw ('Uninstaller refusal observation failed; partial save also failed: {0}; original: {1}' -f $_.Exception.ToString(), $completionFailure) }
            throw ('Uninstaller refusal observation failed: {0}' -f $completionFailure)
        }
        if ($complete) { break }
        $modal = Get-UatOwnedModalWindow -MainWindow $window -ProcessEntry $entry -TimeoutSeconds 1 -CompletionBudget $refusalBudget
        if ($modal) {
            $message = Get-UatWindowSummaryText -Window $modal
            [void]$evidence.Add((Save-UatWindowEvidence -Window $modal -Name ($Name + '-refusal-dialog')))
            $uiaPath = Join-Path $providedEvidencePath ($Name + '-refusal-uia.json')
            Write-UatJsonFile -Path $uiaPath -Value (Get-UatWindowSummary -Window $modal)
            [void]$evidence.Add($uiaPath)
            $button = @(Find-UatWindowButton -Window $modal -NamePattern '^(확인|OK|닫기|Close)')
            if ($button.Count -gt 1) { throw 'Native uninstaller refusal modal has multiple matching confirmation buttons.' }
            if ($button.Count -eq 1) { [void](Invoke-UatWindowButton -Window $modal -NamePattern '^(확인|OK|닫기|Close)') }
            break
        }
        $remainingMilliseconds = Get-UatCompletionRemainingMilliseconds -CompletionBudget $refusalBudget
        if ($remainingMilliseconds -gt 0) { Start-Sleep -Milliseconds ([int][Math]::Min(120, $remainingMilliseconds)) }
    }
    $run = Wait-UatProcess -Entry $entry -TimeoutSeconds $TimeoutSeconds -CompletionBudget $refusalBudget -WaitReason ($Name + ' uninstall refusal completion')
    Assert-UatExpectedRefusal -Result $run -Name $Name
    $native = Read-UatRawLog -Path $logPath
    Assert-Uat (($message -match '실행|잠금|파일|SDIR|설치|제거|uninstall') -or ($native.text -match 'SDIR_UNINSTALL|SDIR_PREFLIGHT|lock|잠금|running|실행')) ('Uninstall refusal lacked owned UI text or a native source log marker: {0}' -f $Name)
    [void]$evidence.Add($logPath)
    [void]$evidence.Add($run.evidence.directory)
    Add-UatEvent -Kind 'native-uninstall-refusal-asserted' -Name $Name -Data @{ processId = $entry.processId; exitCode = $run.exitCode; visibleText = $message; logSha256 = $native.sha256 }
    return [pscustomobject]@{ exitCode = $run.exitCode; evidence = @($evidence.ToArray()); logPath = $logPath; message = $message }
}

function Get-UatOwnedProcessState {
    param([Parameter(Mandatory = $true)][int[]]$ProcessIds, [Parameter(Mandatory = $true)][hashtable]$ExpectedIdentity)
    $rows = New-Object System.Collections.Generic.List[object]
    foreach ($processId in $ProcessIds) {
        $identity = Get-UatProcessIdentity -ProcessId $processId
        Assert-Uat ($ExpectedIdentity.ContainsKey([string]$processId)) ('No expected identity is recorded for owned PID {0}.' -f $processId)
        $expected = $ExpectedIdentity[[string]$processId]
        Assert-Uat ($identity.startTicks -eq [long]$expected.startTicks -and [string]::Equals($identity.imagePath, [string]$expected.imagePath, [System.StringComparison]::OrdinalIgnoreCase)) ('Owned process identity changed for PID {0}.' -f $processId)
        $windows = @([Sdir.Installer.Uat.NativeWindowApi]::TopLevelWindows([uint32]$processId, $true) | ForEach-Object { [ordered]@{ handle = $_.Handle.ToInt64(); title = $_.Title; visible = $_.Visible; className = $_.ClassName } })
        [void]$rows.Add([ordered]@{ processId = $processId; startTicks = $identity.startTicks; imagePath = $identity.imagePath; windows = $windows })
    }
    return @($rows.ToArray())
}

function Get-UatNativePathUnits {
    param([Parameter(Mandatory = $true)][string]$Path)
    $units = New-Object System.Collections.Generic.List[int]
    foreach ($character in $Path.ToCharArray()) { [void]$units.Add([int][char]$character) }
    return @($units.ToArray())
}

function Start-UatIdleWorker {
    param([Parameter(Mandatory = $true)][string]$WatchPath)
    $image = Join-Path $script:testRoot 'SDIR.exe'
    $request = [ordered]@{ type = 'watch'; pane = 0; id = 918273; path = @(Get-UatNativePathUnits -Path $WatchPath) }
    $json = ConvertTo-Json -InputObject $request -Depth 8 -Compress
    $entry = Start-UatProcess -Executable $image -Argument @('--worker') -WorkingDirectory $script:testRoot -Environment @{ SDIR_PROFILE_DIR = $script:profileRoot; SDIR_DEBUG_PORT = [string]$script:debugPort } -Name 'held-idle-watch-worker'
    $entry.child.WriteInput($json + "`n")
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    $readyEvidence = Join-Path $providedEvidencePath 'A8-worker-ready-output.bin'
    while ([DateTime]::UtcNow -lt $deadline -and -not $entry.child.HasExited) {
        $capture = $entry.child.CaptureWhileRunning()
        $decoded = Get-UatStrictDecodedText -Bytes ([byte[]]$capture.StandardOutput)
        foreach ($line in @($decoded.text -split "`r?`n" | Where-Object { $_ })) {
            try {
                $event = ConvertFrom-Json -InputObject $line
            }
            catch { continue }
            if ($event.type -eq 'watch_ready' -and [int]$event.pane -eq 0 -and [long]$event.id -eq 918273) {
                [System.IO.File]::WriteAllBytes($readyEvidence, [byte[]]$capture.StandardOutput)
                $entry.workerReadyEvidence = $readyEvidence
                $entry.workerWatchPath = [System.IO.Path]::GetFullPath($WatchPath)
                Add-UatEvent -Kind 'worker-watch-ready' -Name 'held-idle-watch-worker' -Data @{ processId = $entry.processId; request = $request; outputEvidence = $readyEvidence; stdinRemainsOpen = $true }
                return $entry
            }
        }
        Start-Sleep -Milliseconds 100
    }
    if (-not $entry.child.HasExited) { Save-UatProcessPartial -Entry $entry }
    throw 'The real --worker Watch request did not emit its expected watch_ready event.'
}

function Close-UatIdleWorkerNormally {
    param([Parameter(Mandatory = $true)]$Entry)
    Assert-Uat (-not $Entry.child.HasExited) 'The held idle worker exited before the harness sent normal EOF.'
    $Entry.child.CloseInput()
    $run = Wait-UatProcess -Entry $Entry -TimeoutSeconds 20 -WaitReason 'normal worker EOF shutdown'
    Assert-Uat ($run.exitCode -eq 0) ('The source-defined normal worker EOF path returned {0}.' -f $run.exitCode)
    Add-UatEvent -Kind 'worker-normal-eof-exit' -Name $Entry.name -Data @{ processId = $Entry.processId; exitCode = $run.exitCode; watchPath = $Entry.workerWatchPath; evidence = $run.evidence.directory }
    return $run
}

function Add-UatTestSentinel {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Text)
    Assert-NoReparseComponents -Path (Split-Path -Parent $Path)
    Assert-Uat (-not (Test-Path -LiteralPath $Path)) ('Sentinel path already exists: {0}' -f $Path)
    [System.IO.File]::WriteAllBytes($Path, [System.Text.Encoding]::UTF8.GetBytes($Text))
    return Get-UatFileHash $Path
}

function Remove-UatOwnedFixtureRoot {
    param([Parameter(Mandatory = $true)][string]$Root)
    if (-not (Test-Path -LiteralPath $Root -PathType Container)) { return $true }
    Assert-UatRootOwnerMarker -Root $Root | Out-Null
    Assert-NoReparseComponents -Path $Root
    $snapshotPath = Join-Path $providedEvidencePath ('cleanup-' + [System.IO.Path]::GetFileName($Root) + '-before.json')
    Write-UatJsonFile -Path $snapshotPath -Value (Get-UatTreeSnapshot -Path $Root)
    Remove-Item -LiteralPath $Root -Recurse -Force
    Assert-Uat (-not (Test-Path -LiteralPath $Root)) ('Owned test root did not disappear after safe cleanup: {0}' -f $Root)
    Add-UatEvent -Kind 'owned-fixture-root-removed' -Name ([System.IO.Path]::GetFileName($Root)) -Data @{ root = $Root; beforeSnapshot = $snapshotPath; method = 'exact canonical path after run marker and no-reparse checks' }
    return $true
}

function Assert-UatA6PortableRefusalStart {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [AllowEmptyCollection()][string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [hashtable]$Environment = @{},
        [Parameter(Mandatory = $true)][string]$Name,
        [bool]$TrackNativeEngines = $false,
        [string]$NativeAppRoot,
        [Parameter(Mandatory = $true)][string]$PortableDestinationPath
    )
    Assert-Uat ($script:selectedCases -is [array] -and $script:selectedCases.Count -eq 1 -and
        $script:selectedCases[0] -ceq 'A6' -and $Name -ceq 'A6-portable-destination-refusal' -and
        -not $TrackNativeEngines -and [string]::IsNullOrEmpty($NativeAppRoot) -and
        $script:builds.Contains('portable')) 'Root-only setup capture is restricted to the owned A6 portable preflight refusal.'
    Assert-Uat ([string]$script:runId -cmatch '^[0-9a-f]{32}$') 'A6 root-only capture requires the current run identity.'
    $candidate = $script:builds['portable']
    $fullExe = [System.IO.Path]::GetFullPath($Executable)
    $candidatePath = [System.IO.Path]::GetFullPath([string]$candidate.path)
    $fullWorkingDirectory = [System.IO.Path]::GetFullPath($WorkingDirectory)
    $fullRepoRoot = [System.IO.Path]::GetFullPath([string]$repoRoot)
    Assert-Uat ([string]::Equals($fullExe, $candidatePath, [System.StringComparison]::OrdinalIgnoreCase) -and
        [string]::Equals($fullWorkingDirectory, $fullRepoRoot, [System.StringComparison]::OrdinalIgnoreCase)) 'A6 root-only capture candidate or working directory changed.'
    Assert-Uat ($Environment.Count -eq 0) 'A6 root-only refusal cannot use environment overrides.'
    $expectedLogPath = [System.IO.Path]::GetFullPath((Join-Path $providedEvidencePath 'A6-portable-destination-refusal.setup.log'))
    $expectedFlagPath = [System.IO.Path]::GetFullPath((Join-Path $candidate.installRoot 'portable.flag'))
    $fullFlagPath = [System.IO.Path]::GetFullPath($PortableDestinationPath)
    Assert-Uat ([string]::Equals($fullFlagPath, $expectedFlagPath, [System.StringComparison]::OrdinalIgnoreCase)) 'A6 root-only capture destination is not the current portable.flag.'
    $expectedArguments = [string[]]@('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', ('/LOG=' + $expectedLogPath))
    $argumentsMatch = $Arguments.Count -eq $expectedArguments.Count
    if ($argumentsMatch) {
        for ($index = 0; $index -lt $expectedArguments.Count; $index++) {
            if ([string]$Arguments[$index] -cne $expectedArguments[$index]) { $argumentsMatch = $false; break }
        }
    }
    Assert-Uat $argumentsMatch 'A6 root-only capture requires the exact default silent no-mutation refusal arguments.'
    [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
    [void](Assert-UatRootOwnerMarker -Root $candidate.installRoot)
    Assert-Uat ([string]$script:runOwner.runId -ceq [string]$script:runId -and
        [string]::Equals([System.IO.Path]::GetFullPath([string]$script:runOwner.evidenceDirectory),
            [System.IO.Path]::GetFullPath($providedEvidencePath), [System.StringComparison]::OrdinalIgnoreCase)) 'A6 root-only capture owner does not belong to this run and evidence directory.'
    Assert-NoReparseComponents -Path $expectedFlagPath
    Assert-NoReparseComponents -Path $expectedLogPath
    Assert-Uat (Test-Path -LiteralPath $expectedFlagPath -PathType Leaf) 'A6 root-only capture lost its owned portable.flag destination.'
    Assert-Uat (-not (Test-Path -LiteralPath $expectedLogPath) -and
        @($script:activeProcesses | Where-Object { $_.name -ceq $Name }).Count -eq 0) 'A6 root-only capture requires one unique fresh log and launch.'
    $candidateOwner = $script:runOwner['candidateOwners']['portable']
    return [ordered]@{
        scope = 'ROOT_ONLY'; purpose = 'A6_PORTABLE_PREFLIGHT_EXPECTED_REFUSAL'
        nativeChildObservation = 'NOT_COLLECTED'; nativeChildExit = 'UNKNOWN'
        runId = [string]$script:runId; commandId = $null; processId = $null; processStartedUtc = $null
        candidatePath = $candidatePath; candidateSha256 = [string]$candidate.sha256
        candidateReceiptPath = [string]$candidate.receiptPath; candidateReceiptSha256 = [string]$candidate.receiptSha256
        candidateProvenancePath = [string]$candidateOwner.provenancePath; candidateProvenanceSha256 = [string]$candidateOwner.provenanceSha256
        aggregateProvenancePath = [string]$script:runOwner.candidateProvenancePath
        aggregateProvenanceSha256 = [string]$script:runOwner.candidateProvenanceSha256
        runOwnerPath = [System.IO.Path]::GetFullPath([string]$script:runOwnerPath)
        runOwnerSha256 = Get-UatFileHash $script:runOwnerPath
        portableFlagPath = $expectedFlagPath; logPath = $expectedLogPath
        argumentVector = $expectedArguments; workingDirectory = $fullWorkingDirectory
    }
}

function Assert-UatA6PortableRefusalLog {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text,
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)][string]$LogPath,
        [Parameter(Mandatory = $true)][DateTime]$CompletionObservedUtc
    )
    $intent = $Entry.captureIntent
    Assert-Uat ($null -ne $intent -and $intent.scope -ceq 'ROOT_ONLY' -and
        $intent.purpose -ceq 'A6_PORTABLE_PREFLIGHT_EXPECTED_REFUSAL' -and
        $intent.nativeChildObservation -ceq 'NOT_COLLECTED' -and $intent.nativeChildExit -ceq 'UNKNOWN') 'A6 setup log has no valid root-only capture intent.'
    Assert-Uat ([string]$intent.runId -ceq [string]$script:runId -and [string]$Entry.sessionId -ceq [string]$intent.runId -and
        @($script:activeProcesses | Where-Object { [object]::ReferenceEquals($_, $Entry) }).Count -eq 1 -and
        [string]$Entry.commandId -ceq [string]$intent.commandId -and [int]$Entry.processId -eq [int]$intent.processId -and
        [string]$Entry.startedUtc -ceq [string]$intent.processStartedUtc -and
        [string]::Equals([System.IO.Path]::GetFullPath([string]$Entry.executable), [string]$intent.candidatePath, [System.StringComparison]::OrdinalIgnoreCase) -and
        [string]::Equals([System.IO.Path]::GetFullPath($LogPath), [string]$intent.logPath, [System.StringComparison]::OrdinalIgnoreCase)) 'A6 setup log is not bound to the current run, command, candidate, and fresh log path.'
    $expectedArgumentLine = (@($intent.argumentVector | ForEach-Object { ConvertTo-WindowsCommandArgument ([string]$_) }) -join ' ')
    Assert-Uat ([string]$Entry.arguments -ceq $expectedArgumentLine -and
        [string]::Equals([System.IO.Path]::GetFullPath([string]$Entry.workingDirectory), [string]$intent.workingDirectory, [System.StringComparison]::OrdinalIgnoreCase)) 'A6 setup log command does not match the owned current process arguments.'
    Assert-Uat ($intent.argumentVector -is [array] -and $intent.argumentVector.Count -eq 5 -and
        $intent.argumentVector[0] -ceq '/VERYSILENT' -and $intent.argumentVector[1] -ceq '/SUPPRESSMSGBOXES' -and
        $intent.argumentVector[2] -ceq '/NORESTART' -and $intent.argumentVector[3] -ceq '/SP-' -and
        $intent.argumentVector[4] -ceq ('/LOG=' + $intent.logPath)) 'A6 setup log is not bound to the exact default silent no-mutation refusal arguments.'
    $prefix = '^[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}[.][0-9]{3} +'
    $reason = 'A portable.flag destination is never modified by the installer.'
    $marker = 'SDIR_PREFLIGHT_REFUSED: ' + $reason
    if ($Text.Length -gt 0 -and $Text[0] -eq [char]0xfeff) { $Text = $Text.Substring(1) }
    $lines = @($Text.TrimEnd([char[]]@(13, 10)) -split '\r?\n')
    $openMatch = [Regex]::Match($lines[0], $prefix + 'Log opened[.] \(Time zone: UTC(?<offset>[+-][0-9]{2}:[0-9]{2})\)$')
    Assert-Uat ($lines.Count -ge 8 -and $openMatch.Success -and
        $lines[1] -cmatch ($prefix + 'Setup version: Inno Setup version [^\r\n]+$') -and
        $lines[$lines.Count - 1] -cmatch ($prefix + 'Log closed[.]$')) 'A6 requires a complete Inno setup log from open to close.'
    $culture = [System.Globalization.CultureInfo]::InvariantCulture
    $offset = $openMatch.Groups['offset'].Value
    $logTimeFormat = 'yyyy-MM-dd HH:mm:ss.fffzzz'
    $opened = [DateTimeOffset]::ParseExact(($lines[0].Substring(0, 23) + $offset), $logTimeFormat, $culture)
    $closed = [DateTimeOffset]::ParseExact(($lines[$lines.Count - 1].Substring(0, 23) + $offset), $logTimeFormat, $culture)
    $started = [DateTimeOffset]::ParseExact([string]$Entry.startedUtc, 'o', $culture)
    # Inno's local clock has lower resolution than the process creation timestamp.
    Assert-Uat ($opened -ge $started.AddSeconds(-1) -and $closed -ge $opened -and
        $closed.UtcDateTime -le $CompletionObservedUtc.AddSeconds(1)) 'A6 setup log timestamps fall outside the current command window.'
    $originalMatch = [Regex]::Match($lines[2], $prefix + 'Original Setup EXE: (?<path>.+)$')
    Assert-Uat ($originalMatch.Success -and [string]::Equals([System.IO.Path]::GetFullPath($originalMatch.Groups['path'].Value),
        [string]$intent.candidatePath, [System.StringComparison]::OrdinalIgnoreCase)) 'A6 Inno log original executable does not match the current candidate.'
    $commandMatch = [Regex]::Match($lines[3], $prefix + 'Setup command line: /SL5="\$[0-9A-Fa-f]+,[0-9]+,[0-9]+,(?<loaderPath>[^"]+)" (?<arguments>"[^"]*"(?: "[^"]*")*)$')
    Assert-Uat $commandMatch.Success 'A6 Inno log command line identity is missing or malformed.'
    Assert-Uat ([string]::Equals($commandMatch.Groups['loaderPath'].Value,
        [string]$intent.candidatePath, [System.StringComparison]::OrdinalIgnoreCase)) 'A6 Inno log loader identity does not match the current candidate.'
    $expectedLoggedArguments = (@($intent.argumentVector | ForEach-Object { '"' + [string]$_ + '"' }) -join ' ')
    Assert-Uat ($commandMatch.Groups['arguments'].Value -ceq $expectedLoggedArguments) 'A6 Inno log command line does not match the exact current argv and log path.'
    $markerIndex = -1
    $prepareIndex = -1
    $restartIndex = -1
    for ($i = 4; $i -lt $lines.Count - 1; $i++) {
        $line = $lines[$i]
        if ($line -match ($prefix + '(?:(?:Original Setup EXE|Setup command line|Setup version):|Log (?:opened|closed)[.])')) { throw 'A6 Inno log contains conflicting duplicate execution identity or boundary.' }
        if ($line -cmatch ($prefix + 'SDIR_PREFLIGHT_REFUSED:')) {
            Assert-Uat ($markerIndex -eq -1 -and $line -cmatch ($prefix + [Regex]::Escape($marker) + '$')) 'A6 observed an unexpected or duplicate preflight refusal.'
            $markerIndex = $i
        }
        if ($line -cmatch ($prefix + 'PrepareToInstall failed:')) {
            Assert-Uat ($prepareIndex -eq -1 -and $line -cmatch ($prefix + [Regex]::Escape('PrepareToInstall failed: ' + $reason) + '$')) 'A6 observed an unexpected or duplicate PrepareToInstall failure.'
            $prepareIndex = $i
        }
        if ($line -cmatch ($prefix + 'Need to restart Windows\?')) {
            Assert-Uat ($restartIndex -eq -1 -and $line -cmatch ($prefix + 'Need to restart Windows\? No$')) 'A6 observed an unexpected restart outcome.'
            $restartIndex = $i
        }
        Assert-Uat ($line -cnotmatch ($prefix + 'SDIR_(?:PREFLIGHT_OK|SETUP_MUTEX_REFUSED|WEBVIEW2_MISSING|UNINSTALL_REGISTRY_PREFLIGHT_REFUSED|SNAPSHOT_|QUARANTINE_|NATIVE_COMMIT|RECEIPT_MISSING_AFTER_COMMIT|RECOVERY_BACKUP_RETAINED|DEINITIALIZE_COMPENSATION_)')) 'A6 log contains a different outcome or installation transaction activity.'
    }
    Assert-Uat ($markerIndex -gt 3 -and $prepareIndex -gt $markerIndex -and $restartIndex -gt $prepareIndex) 'A6 portable preflight refusal, matching PrepareToInstall failure, and NeedsRestart=False were not observed in order.'
    return [pscustomobject]@{
        marker = $marker; originalSetupExe = $originalMatch.Groups['path'].Value
        setupCommandLine = $lines[3]; setupArguments = [string[]]$intent.argumentVector
        runId = [string]$intent.runId; commandId = [string]$intent.commandId
        processId = [int]$intent.processId; candidateSha256 = [string]$intent.candidateSha256
        logPath = [string]$intent.logPath; closed = $true
        logOpenedUtc = $opened.UtcDateTime.ToString('o'); logClosedUtc = $closed.UtcDateTime.ToString('o')
        completionObservedUtc = $CompletionObservedUtc.ToString('o')
    }
}

function Assert-UatA6PortableRefusalResult {
    param([Parameter(Mandatory = $true)]$Entry, [Parameter(Mandatory = $true)]$Run)
    Assert-UatTrackedProcessResultDto -Entry $Entry -CaseId 'A6'
    $intent = $Entry.captureIntent
    Assert-Uat ($null -ne $intent -and $intent.scope -ceq 'ROOT_ONLY' -and
        $intent.nativeChildObservation -ceq 'NOT_COLLECTED' -and $intent.nativeChildExit -ceq 'UNKNOWN' -and
        -not $Entry.nativeTracking) 'A6 refusal must retain ROOT_ONLY with native child observation NOT_COLLECTED and exit UNKNOWN.'
    Assert-Uat ($Run.nativeEngineExitCodes -is [array] -and $Run.nativeEngineExitCodes.Count -eq 0 -and
        $null -eq $Run.launchHandoff) 'A6 refusal unexpectedly claimed native child collection or launcher handoff.'
    Assert-Uat ([int]$Entry.completedResult.exitCode -eq 7 -and [int]$Entry.completedResult.launcherExitCode -eq 7 -and
        [int]$Run.launcherExitCode -eq 7) 'A6 portable.flag PrepareToInstall refusal requires actual owned ROOT exit code 7.'
    return $true
}

function Assert-UatA6PortableRefusalProductionProof {
    param(
        [Parameter(Mandatory = $true)]$Capture,
        [Parameter(Mandatory = $true)][string]$BeforePath,
        [Parameter(Mandatory = $true)][string]$AfterPath
    )
    Assert-Uat ($null -ne $Capture.productionProtectionBefore -and $null -ne $Capture.productionProtectionAfter) 'A6 refusal production protection comparison is incomplete.'
    Test-UatSnapshotEqual -Expected $Capture.productionProtectionBefore -Actual $Capture.productionProtectionAfter -Message 'A6 portable refusal changed the existing production protection scope.'
    $expectedBefore = [System.IO.Path]::GetFullPath((Join-Path $providedEvidencePath 'A6-production-before-portable-refusal.json'))
    $expectedAfter = [System.IO.Path]::GetFullPath((Join-Path $providedEvidencePath 'A6-production-after-portable-refusal.json'))
    Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath($BeforePath), $expectedBefore, [System.StringComparison]::OrdinalIgnoreCase) -and
        [string]::Equals([System.IO.Path]::GetFullPath($AfterPath), $expectedAfter, [System.StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $expectedBefore -PathType Leaf) -and (Test-Path -LiteralPath $expectedAfter -PathType Leaf)) 'A6 refusal production snapshots are not the current owned evidence files.'
    $entry = $Capture.entry
    $intent = $entry.captureIntent
    Assert-Uat ([string]$entry.sessionId -ceq [string]$intent.runId -and [string]$entry.commandId -ceq [string]$intent.commandId -and
        [string]$Capture.logPath -ceq [string]$intent.logPath) 'A6 production scope comparison is not connected to the current refusal command.'
    return [ordered]@{
        scope = 'Get-UatProductionSnapshot'; unchanged = $true
        runId = [string]$entry.sessionId; commandId = [string]$entry.commandId
        candidatePath = [string]$intent.candidatePath; candidateSha256 = [string]$intent.candidateSha256
        logPath = [string]$Capture.logPath; logSha256 = [string]$Capture.logSha256
        before = [ordered]@{ path = $expectedBefore; sha256 = Get-UatFileHash $expectedBefore }
        after = [ordered]@{ path = $expectedAfter; sha256 = Get-UatFileHash $expectedAfter }
    }
}

function Start-UatA6PortableRefusalCapture {
    $candidate = $script:builds['portable']
    $name = 'A6-portable-destination-refusal'
    $logPath = Join-Path $providedEvidencePath ($name + '.setup.log')
    $arguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', ('/LOG=' + $logPath))
    $budget = New-UatCompletionBudget -TotalMilliseconds ([long]240000) -Origin $name
    $portableFlagPath = Join-Path $candidate.installRoot 'portable.flag'
    $entry = Start-UatProcess -Executable $candidate.path -Argument $arguments -WorkingDirectory $repoRoot -Name $name -CompletionBudget $budget -A6PortableRefusal -A6PortableDestinationPath $portableFlagPath
    $run = Wait-UatProcess -Entry $entry -WaitReason $name -CompletionBudget $budget
    $completionObservedUtc = [DateTime]::UtcNow
    $productionRegistryKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $script:productionAppId + '_is1'
    $productionProtectionAfter = Get-UatProductionSnapshot -ProductionRegistryKey $productionRegistryKey
    [void](Assert-UatA6PortableRefusalResult -Entry $entry -Run $run)
    Assert-NoReparseComponents -Path $logPath
    $logBytes = [System.IO.File]::ReadAllBytes($logPath)
    $native = Get-UatStrictDecodedText -Bytes $logBytes
    $logIdentity = Assert-UatA6PortableRefusalLog -Text $native.text -Entry $entry -LogPath $logPath -CompletionObservedUtc $completionObservedUtc
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try { $logSha256 = [BitConverter]::ToString($sha.ComputeHash($logBytes)).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
    return [pscustomobject]@{
        entry = $entry; logPath = $logPath; marker = $logIdentity.marker; logIdentity = $logIdentity; logBytes = $logBytes.Length
        logSha256 = $logSha256; logEncoding = $native.encoding; evidence = @($run.evidence.directory, $logPath)
        productionProtectionBefore = $entry.productionProtectionBefore; productionProtectionAfter = $productionProtectionAfter
    }
}

function Test-UatA6PortableAndSeparateInstall {
    $portableCandidate = $script:builds['portable']
    $separateCandidate = $script:builds['separate']
    $script:testRoot = [System.IO.Path]::GetFullPath([string]$script:initialTestRoot)
    $script:testRootName = [string]$script:initialTestRootName
    Assert-Uat (-not (Test-Path -LiteralPath $portableCandidate.installRoot)) 'The portable destination is not a fresh owned fixture path.'
    Assert-Uat (-not (Test-Path -LiteralPath $separateCandidate.installRoot)) 'The separate A6 install root is not fresh.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A6 cannot start while the fixed TEST AppID is registered.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A6 cannot start while a TEST shortcut exists.'
    $portableArchiveShaBefore = Get-UatFileHash $script:portablePath
    Assert-Uat ($portableArchiveShaBefore -ceq $script:stablePortableExpected) 'The source portable archive changed before A6.'
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $portableRoot = [System.IO.Path]::GetFullPath($portableCandidate.installRoot)
    New-Item -ItemType Directory -Path $portableRoot | Out-Null
    $ownerMarker = Write-UatRootOwnerMarker -Root $portableRoot -Purpose 'portable-archive-refusal-fixture'
    $archive = [System.IO.Compression.ZipFile]::OpenRead($script:portablePath)
    try {
        $seenFiles = New-Object System.Collections.Generic.List[string]
        foreach ($entry in $archive.Entries) {
            $normalized = ([string]$entry.FullName).Replace([char]92, '/').TrimStart('/')
            Assert-Uat (-not [System.IO.Path]::IsPathRooted($normalized) -and $normalized -notmatch '(^|/)[.][.]?(/|$)' -and $normalized -notmatch '^[A-Za-z]:') ('Portable archive contains an unsafe entry: {0}' -f $entry.FullName)
            if (-not $normalized) { continue }
            $parts = @($normalized.Split('/') | Where-Object { $_ })
            if ($parts.Count -eq 1 -and $parts[0] -eq 'SDIR' -and ($entry.FullName.EndsWith('/') -or $entry.FullName.EndsWith([string][char]92))) { continue }
            Assert-Uat ($parts.Count -ge 2 -and $parts[0] -eq 'SDIR') ('Portable ZIP entry is not under its reviewed SDIR/ root: {0}' -f $entry.FullName)
            $relative = @($parts | Select-Object -Skip 1) -join [System.IO.Path]::DirectorySeparatorChar
            if (-not $relative) { continue }
            $target = [System.IO.Path]::GetFullPath((Join-Path $portableRoot $relative))
            $prefix = $portableRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
            Assert-Uat ($target.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) ('Portable extraction escaped the owned fixture root: {0}' -f $entry.FullName)
            if ($entry.FullName.EndsWith('/') -or $entry.FullName.EndsWith([string][char]92)) {
                if (-not (Test-Path -LiteralPath $target -PathType Container)) { New-Item -ItemType Directory -Path $target -Force | Out-Null }
                continue
            }
            $parent = Split-Path -Parent $target
            if (-not (Test-Path -LiteralPath $parent -PathType Container)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
            $sourceStream = $entry.Open()
            $targetStream = New-Object System.IO.FileStream -ArgumentList @($target, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
            try { $sourceStream.CopyTo($targetStream) }
            finally { $targetStream.Dispose(); $sourceStream.Dispose() }
            [void]$seenFiles.Add($relative.Replace([System.IO.Path]::DirectorySeparatorChar, '/'))
        }
        Assert-Uat ($seenFiles -contains 'portable.flag' -and $seenFiles -contains 'SDIR.exe') 'The normalized portable archive did not contain portable.flag and SDIR.exe under its SDIR/ root.'
        Assert-Uat (-not ($seenFiles -contains 'data/settings.json')) 'The normalized portable archive unexpectedly contained data/settings.json under its SDIR/ root.'
    }
    finally { $archive.Dispose() }
    Assert-UatRootOwnerMarker -Root $portableRoot | Out-Null
    $portableMarkerPath = Join-Path $portableRoot 'portable.flag'
    Assert-Uat (Test-Path -LiteralPath $portableMarkerPath -PathType Leaf) 'The extracted existing portable destination lacks portable.flag.'
    Assert-Uat ($null -ne $script:profileFixture) 'A6 requires the current owned New-UatProfileFixture settings source.'
    $profileFixtureRoot = [System.IO.Path]::GetFullPath([string]$script:profileFixture.root)
    $currentProfileRoot = [System.IO.Path]::GetFullPath([string]$script:profileRoot)
    Assert-Uat ([string]::Equals($profileFixtureRoot, $currentProfileRoot, [System.StringComparison]::OrdinalIgnoreCase)) 'A6 profile settings source is not from the current owned profile fixture.'
    $profileSettingsSource = [System.IO.Path]::GetFullPath([string]$script:profileFixture.settings)
    $expectedProfileSettingsSource = [System.IO.Path]::GetFullPath((Join-Path $profileFixtureRoot 'settings.json'))
    Assert-Uat ([string]::Equals($profileSettingsSource, $expectedProfileSettingsSource, [System.StringComparison]::OrdinalIgnoreCase)) 'A6 profile settings source is outside the current owned profile fixture.'
    Assert-Uat (Test-Path -LiteralPath $profileSettingsSource -PathType Leaf) 'The current owned profile fixture settings.json is missing.'
    $portableDataRoot = [System.IO.Path]::GetFullPath((Join-Path $portableRoot 'data'))
    $portableRootPrefix = $portableRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    Assert-Uat ($portableDataRoot.StartsWith($portableRootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) 'A6 synthetic settings data directory escaped the owned portable root.'
    if (-not (Test-Path -LiteralPath $portableDataRoot -PathType Container)) {
        Assert-Uat (-not (Test-Path -LiteralPath $portableDataRoot)) 'A6 synthetic settings data path exists but is not a directory.'
        New-Item -ItemType Directory -Path $portableDataRoot | Out-Null
    }
    Assert-NoReparseComponents -Path $portableDataRoot
    $syntheticSettingsPath = [System.IO.Path]::GetFullPath((Join-Path $portableDataRoot 'settings.json'))
    Assert-Uat ($syntheticSettingsPath.StartsWith($portableRootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) 'A6 synthetic settings target escaped the owned portable root.'
    Assert-Uat (-not (Test-Path -LiteralPath $syntheticSettingsPath)) 'A6 synthetic settings target already exists.'
    [System.IO.File]::Copy($profileSettingsSource, $syntheticSettingsPath, $false)
    $profileSettingsFile = Get-Item -LiteralPath $profileSettingsSource -ErrorAction Stop
    $syntheticSettingsFile = Get-Item -LiteralPath $syntheticSettingsPath -ErrorAction Stop
    $profileSettingsSha256 = Get-UatFileHash $profileSettingsSource
    $syntheticSettingsSha256 = Get-UatFileHash $syntheticSettingsPath
    Assert-Uat ([long]$profileSettingsFile.Length -eq [long]$syntheticSettingsFile.Length -and $profileSettingsSha256 -ceq $syntheticSettingsSha256) 'A6 synthetic portable settings differ from the current owned profile fixture bytes.'
    $syntheticSettings = [ordered]@{
        origin = 'current-owned-New-UatProfileFixture'
        sourcePath = $profileSettingsSource
        sourceBytes = [long]$profileSettingsFile.Length
        sourceSha256 = $profileSettingsSha256
        destinationPath = $syntheticSettingsPath
        destinationBytes = [long]$syntheticSettingsFile.Length
        destinationSha256 = $syntheticSettingsSha256
    }
    $portableBefore = Get-UatTreeSnapshot -Path $portableRoot
    $refusalProfileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    $registryBefore = Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey
    $startBefore = Get-UatShortcutSnapshot -Path $script:startMenuShortcut
    $desktopBefore = Get-UatShortcutSnapshot -Path $script:desktopShortcut
    $startDirectory = Split-Path -Parent $script:startMenuShortcut
    $startDirectoryBefore = Get-UatTreeSnapshot -Path $startDirectory
    $separateRootBefore = Get-UatTreeSnapshot -Path $separateCandidate.installRoot
    Assert-Uat ($portableBefore.exists -and $refusalProfileBefore.exists -and -not $registryBefore.exists -and
        -not $startBefore.exists -and -not $desktopBefore.exists -and -not $separateRootBefore.exists) 'A6 portable refusal fixture or its isolated pre-launch state is invalid.'
    $portableBeforePath = Join-Path $providedEvidencePath 'A6-portable-before-refusal.json'
    Write-UatJsonFile -Path $portableBeforePath -Value ([ordered]@{
        portableArchivePath = $script:portablePath
        portableArchiveSha256 = $portableArchiveShaBefore
        portableRoot = $portableRoot
        portableMarkerPath = $ownerMarker
        portableMarkerSha256 = Get-UatFileHash $ownerMarker
        portableFlagPath = $portableMarkerPath
        portableFlagSha256 = Get-UatFileHash $portableMarkerPath
        syntheticSettings = $syntheticSettings
        rootSnapshot = $portableBefore
        profileSnapshot = $refusalProfileBefore
        registry = $registryBefore
        startMenuShortcut = $startBefore
        desktopShortcut = $desktopBefore
        startMenuDirectory = $startDirectoryBefore
        separateRoot = $separateRootBefore
    })
    $portableRefusal = Start-UatA6PortableRefusalCapture
    $portableAfter = Get-UatTreeSnapshot -Path $portableRoot
    $refusalProfileAfter = Get-UatTreeSnapshot -Path $script:profileRoot
    Test-UatSnapshotEqual -Expected $refusalProfileBefore -Actual $refusalProfileAfter -Message 'Portable refusal changed the isolated profile.'
    $registryAfter = Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey
    $startAfter = Get-UatShortcutSnapshot -Path $script:startMenuShortcut
    $desktopAfter = Get-UatShortcutSnapshot -Path $script:desktopShortcut
    Test-UatSnapshotEqual -Expected $portableBefore -Actual $portableAfter -Message 'Portable destination changed after its actual installer refusal.'
    Test-UatSnapshotEqual -Expected $registryBefore -Actual $registryAfter -Message 'Portable refusal changed the test registry.'
    Test-UatSnapshotEqual -Expected $startBefore -Actual $startAfter -Message 'Portable refusal changed the test Start Menu shortcut.'
    Test-UatSnapshotEqual -Expected $desktopBefore -Actual $desktopAfter -Message 'Portable refusal changed the test desktop shortcut.'
    $startDirectoryAfter = Get-UatTreeSnapshot -Path $startDirectory
    $separateRootAfter = Get-UatTreeSnapshot -Path $separateCandidate.installRoot
    Test-UatSnapshotEqual -Expected $startDirectoryBefore -Actual $startDirectoryAfter -Message 'Portable refusal changed the test Start Menu directory.'
    Test-UatSnapshotEqual -Expected $separateRootBefore -Actual $separateRootAfter -Message 'Portable refusal changed the separate install root.'
    $portableArchiveShaAfter = Get-UatFileHash $script:portablePath
    Assert-Uat ($portableArchiveShaAfter -ceq $portableArchiveShaBefore -and $portableArchiveShaAfter -ceq $script:stablePortableExpected) 'A6 portable refusal changed the source ZIP bytes.'
    $portableAfterPath = Join-Path $providedEvidencePath 'A6-portable-after-refusal.json'
    Write-UatJsonFile -Path $portableAfterPath -Value ([ordered]@{
        portableArchivePath = $script:portablePath
        portableArchiveSha256Before = $portableArchiveShaBefore
        portableArchiveSha256After = $portableArchiveShaAfter
        portableRoot = $portableRoot
        portableMarkerPath = $ownerMarker
        portableMarkerSha256 = Get-UatFileHash $ownerMarker
        portableFlagPath = $portableMarkerPath
        portableFlagSha256 = Get-UatFileHash $portableMarkerPath
        rootSnapshot = $portableAfter
        profileSnapshot = $refusalProfileAfter
        registry = $registryAfter
        startMenuShortcut = $startAfter
        desktopShortcut = $desktopAfter
        startMenuDirectory = $startDirectoryAfter
        separateRoot = $separateRootAfter
        unchanged = $true
    })
    [void](Assert-UatTrackedProcessResults -CaseId 'A6')
    [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
    Assert-Uat ((Get-UatFileHash $portableRefusal.logPath) -ceq $portableRefusal.logSha256) 'A6 setup log changed after refusal capture.'
    $productionBeforePath = Join-Path $providedEvidencePath 'A6-production-before-portable-refusal.json'
    $productionAfterPath = Join-Path $providedEvidencePath 'A6-production-after-portable-refusal.json'
    Write-UatJsonFile -Path $productionBeforePath -Value $portableRefusal.productionProtectionBefore
    Write-UatJsonFile -Path $productionAfterPath -Value $portableRefusal.productionProtectionAfter
    $productionProof = Assert-UatA6PortableRefusalProductionProof -Capture $portableRefusal -BeforePath $productionBeforePath -AfterPath $productionAfterPath
    $refusalQualification = Join-Path $providedEvidencePath 'A6-portable-refusal-qualification.json'
    Write-UatJsonFile -Path $refusalQualification -Value ([ordered]@{
        schemaVersion = 1; caseId = 'A6'; outcome = 'EXPECTED_REFUSAL_WITH_PRESERVED_STATE'
        runId = $script:runId; name = $portableRefusal.entry.name; commandId = $portableRefusal.entry.commandId
        launcherProcessId = $portableRefusal.entry.processId; launcherExitCode = $portableRefusal.entry.completedResult.launcherExitCode
        captureIntent = $portableRefusal.entry.captureIntent
        candidate = [ordered]@{
            path = $portableCandidate.path; setupSha256 = $portableCandidate.sha256
            receiptPath = $portableCandidate.receiptPath; receiptSha256 = $portableCandidate.receiptSha256
        }
        setupLog = [ordered]@{
            path = $portableRefusal.logPath; bytes = $portableRefusal.logBytes; sha256 = $portableRefusal.logSha256
            encoding = $portableRefusal.logEncoding; marker = $portableRefusal.marker; identity = $portableRefusal.logIdentity
        }
        productionProtection = $productionProof
        before = [ordered]@{ path = $portableBeforePath; sha256 = Get-UatFileHash $portableBeforePath }
        after = [ordered]@{ path = $portableAfterPath; sha256 = Get-UatFileHash $portableAfterPath }
    })
    $portableRefusal.entry.portableRefusalQualified = $true
    $installProfileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    $installProfileBeforePath = Join-Path $providedEvidencePath 'A6-profile-before-separate-install.json'
    Write-UatJsonFile -Path $installProfileBeforePath -Value $installProfileBefore
    [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
    $separate = Invoke-UatWizardInstall -Candidate $separateCandidate -Name 'A6-separate-install' -DesktopShortcut:$false
    Assert-Uat ($separate.setupExitCode -eq 0) 'A separate non-portable destination did not install successfully.'
    $installProfileAfter = Get-UatTreeSnapshot -Path $script:profileRoot
    Test-UatSnapshotEqual -Expected $installProfileBefore -Actual $installProfileAfter -Message 'A6 separate install changed the isolated settings profile.'
    $installProfileAfterPath = Join-Path $providedEvidencePath 'A6-profile-after-separate-install.json'
    Write-UatJsonFile -Path $installProfileAfterPath -Value $installProfileAfter
    $separatePayload = Assert-UatInstalledPayload -Candidate $separateCandidate -Root $separateCandidate.installRoot -DesktopExpected:$false
    $manualDestinationRoot = Join-Path $script:profileRoot 'manual-transfer'
    New-Item -ItemType Directory -Path $manualDestinationRoot | Out-Null
    $sourceSettings = [string]$syntheticSettings.destinationPath
    $destinationSettings = Join-Path $manualDestinationRoot 'settings.json'
    $sourceSettingsHash = Get-UatFileHash $sourceSettings
    Assert-Uat (-not (Test-Path -LiteralPath $destinationSettings)) 'The owned manual-transfer target was not empty.'
    [System.IO.File]::Copy($sourceSettings, $destinationSettings, $false)
    Assert-Uat ((Get-UatFileHash $destinationSettings) -eq $sourceSettingsHash) 'Settings-only manual transfer changed source bytes.'
    $firstTransferHash = Get-UatFileHash $destinationSettings
    $overwriteRefused = $false
    try { [System.IO.File]::Copy($sourceSettings, $destinationSettings, $false) }
    catch [System.IO.IOException] { $overwriteRefused = $true }
    Assert-Uat $overwriteRefused 'Settings-only manual transfer unexpectedly overwrote an existing destination.'
    Assert-Uat ((Get-UatFileHash $destinationSettings) -eq $firstTransferHash -and (Get-UatFileHash $sourceSettings) -eq $sourceSettingsHash) 'Manual transfer no-overwrite check did not preserve both settings files.'
    $uninstallProfileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    $separateBefore = Get-UatInstallSnapshot -Root $separateCandidate.installRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    $separateBeforePath = Join-Path $providedEvidencePath 'A6-separate-before-remove.json'
    Write-UatJsonFile -Path $separateBeforePath -Value ([ordered]@{
        installSnapshot = $separateBefore
        installedPayload = $separatePayload
        candidateName = [string]$separateCandidate.name
        candidateRoot = [string]$separateCandidate.installRoot
        candidateExePath = [string]$separateCandidate.path
        candidateExeSha256 = [string]$separateCandidate.sha256
        candidateReceiptPath = [string]$separateCandidate.receiptPath
        candidateReceiptSha256 = [string]$separateCandidate.receiptSha256
        profileBaseline = $uninstallProfileBefore
        settingsSource = $sourceSettings
        settingsSourceSha256 = $sourceSettingsHash
        settingsDestination = $destinationSettings
        settingsDestinationSha256 = $firstTransferHash
        overwriteAttemptRefused = $overwriteRefused
    })
    $script:testRoot = [System.IO.Path]::GetFullPath([string]$separateCandidate.installRoot)
    $script:testRootName = [string]$separateCandidate.rootName
    [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
    [void](Assert-UatTrackedProcessResults -CaseId 'A6')
    $a6UninstallBudget = New-UatCompletionBudget -TotalMilliseconds ([long]90000) -Origin 'A6 separate install uninstall route'
    $uninstaller = Start-UatUninstallerGui -Name 'A6-separate-install-remove' -CompletionBudget $a6UninstallBudget
    $removed = Resolve-UatUninstallerConfirmation -Uninstaller $uninstaller -Choice 'remove'
    Assert-Uat ($removed.exitCode -eq 0) 'The A6 separate install could not be removed by its actual registered native uninstaller.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A6 native uninstall left the test registration.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A6 native uninstall left a shortcut.'
    Assert-Uat (-not (Test-Path -LiteralPath $separateCandidate.installRoot)) 'A6 registered uninstaller left owned program files in its separate test root.'
    $profileAfterRemove = Get-UatTreeSnapshot -Path $script:profileRoot
    Test-UatSnapshotEqual -Expected $uninstallProfileBefore -Actual $profileAfterRemove -Message 'A6 separate uninstall changed the isolated settings profile.'
    Test-UatSnapshotEqual -Expected $portableBefore -Actual (Get-UatTreeSnapshot -Path $portableRoot) -Message 'A6 later lifecycle changed the portable fixture.'
    Assert-Uat ((Get-UatFileHash $sourceSettings) -ceq $sourceSettingsHash -and (Get-UatFileHash $destinationSettings) -ceq $firstTransferHash) 'A6 later lifecycle changed portable or transferred settings.'
    $separateAfter = Get-UatInstallSnapshot -Root $separateCandidate.installRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    $separateAfterPath = Join-Path $providedEvidencePath 'A6-separate-after-remove.json'
    Write-UatJsonFile -Path $separateAfterPath -Value ([ordered]@{
        installSnapshot = $separateAfter
        candidateRoot = [string]$separateCandidate.installRoot
        nativeExitCode = [int]$removed.exitCode
        registrationAbsent = -not [bool]$separateAfter.registry.exists
        linksAbsent = -not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)
        profileAfter = $profileAfterRemove
    })
    $manualEvidence = Join-Path $providedEvidencePath 'A6-settings-only-manual-transfer.json'
    Write-UatJsonFile -Path $manualEvidence -Value ([ordered]@{
        source = $sourceSettings; sourceSha256 = $sourceSettingsHash
        destination = $destinationSettings; destinationSha256 = $firstTransferHash
        overwriteAttemptRefused = $overwriteRefused; onlySettingsCopied = $true
        profileBaselineAfterTransfer = $uninstallProfileBefore
        profileAfterRemove = $profileAfterRemove
        sourcePortableArchiveSha256 = $portableArchiveShaAfter
    })
    $script:a6Evidence = @($portableRefusal.evidence) + @($refusalQualification, $productionBeforePath, $productionAfterPath, $portableBeforePath, $portableAfterPath, $installProfileBeforePath, $installProfileAfterPath) + @($separate.evidence) + @($separate.logPath, $separateBeforePath, $uninstaller.screenshot, $uninstaller.logPath, $removed.evidence.directory, $separateAfterPath, $manualEvidence, $ownerMarker)
    return $script:a6Evidence
}

function Test-UatA8RunningAppAndWorkerBlockChanges {
    $candidate = $script:builds['fresh']
    $script:testRoot = $candidate.installRoot
    $app = Start-UatDirectApp -Root $script:testRoot -Name 'A8-running-app-direct'
    $worker = Start-UatIdleWorker -WatchPath $script:testRoot
    $expectedProcesses = @{}
    $expectedProcesses[[string]$app.processId] = @{ startTicks = $app.startTicks; imagePath = $app.imagePath }
    $expectedProcesses[[string]$worker.processId] = @{ startTicks = $worker.child.StartTimeUtc.Ticks; imagePath = $worker.executable }
    $rootBefore = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    $ownedStateBefore = Get-UatOwnedProcessState -ProcessIds @($app.processId, $worker.processId) -ExpectedIdentity $expectedProcesses
    $evidence = New-Object System.Collections.Generic.List[string]
    foreach ($case in @(
        @{ name = 'A8-gui-reinstall-running-app-worker'; kind = 'setup-gui' },
        @{ name = 'A8-silent-reinstall-running-app-worker'; kind = 'setup-silent' },
        @{ name = 'A8-gui-uninstall-running-app-worker'; kind = 'uninstall-gui' },
        @{ name = 'A8-silent-uninstall-running-app-worker'; kind = 'uninstall-silent' }
    )) {
        if ($case.kind -eq 'setup-gui') {
            $result = Invoke-UatGuiExpectedRefusal -Candidate $candidate -Name $case.name -ExpectedTextPattern '실행|잠금|SDIR|설치|프로그램'
            foreach ($path in $result.evidence) { [void]$evidence.Add($path) }
        }
        elseif ($case.kind -eq 'setup-silent') {
            $result = Invoke-UatSilentExpectedRefusal -Candidate $candidate -Name $case.name -ExpectedLogPattern 'SDIR_PREFLIGHT_REFUSED'
            foreach ($path in $result.evidence) { [void]$evidence.Add($path) }
        }
        elseif ($case.kind -eq 'uninstall-gui') {
            $result = Invoke-UatGuiUninstallExpectedRefusal -Name $case.name
            foreach ($path in $result.evidence) { [void]$evidence.Add($path) }
        }
        else {
            $logPath = Join-Path $providedEvidencePath ($case.name + '.uninstall.log')
            $a8UninstallBudget = New-UatCompletionBudget -TotalMilliseconds ([long]240000) -Origin ($case.name + ' native silent uninstall route')
            $run = Invoke-UatCommand -Executable (Get-UatUninstallerPath) -Argument @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', ('/LOG=' + $logPath)) -WorkingDirectory $script:testRoot -Name $case.name -TimeoutSeconds 240 -TrackNativeEngines -NativeAppRoot $script:testRoot -CompletionBudget $a8UninstallBudget
            Assert-UatExpectedRefusal -Result $run -Name $case.name
            $native = Read-UatRawLog -Path $logPath
            Assert-Uat ($native.text -match 'SDIR_UNINSTALL|running|실행|잠금|SDIR.exe') 'Silent uninstall did not emit a source refusal marker for the running app/worker.'
            [void]$evidence.Add($logPath); [void]$evidence.Add($run.evidence.directory)
        }
        Test-UatSnapshotEqual -Expected $rootBefore -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message ('A8 refusal changed installation state: ' + $case.name)
        $ownedStateAfter = Get-UatOwnedProcessState -ProcessIds @($app.processId, $worker.processId) -ExpectedIdentity $expectedProcesses
        Test-UatSnapshotEqual -Expected $ownedStateBefore -Actual $ownedStateAfter -Message ('A8 refusal changed an owned process identity or window: ' + $case.name)
    }
    $appClose = Close-UatAppNormally -App $app -Name 'A8-app-normal-close'
    $workerExit = Close-UatIdleWorkerNormally -Entry $worker
    Test-UatSnapshotEqual -Expected $rootBefore -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message 'A8 normal app/worker closure changed installation bytes.'
    Set-UatCase -Id 'A8' -Status 'verified' -Evidence (@($evidence.ToArray()) + @($worker.workerReadyEvidence, $workerExit.evidence.directory, $script:eventsPath)) -Details @('A real installed SDIR.exe and a source-protocol --worker Watch process remained alive with unchanged PID/start time/window and full installation snapshot through GUI and silent reinstall/uninstall refusals; the app received WM_CLOSE and the worker exited only after normal stdin EOF.')
    return @($evidence.ToArray()) + @($worker.workerReadyEvidence, $workerExit.evidence.directory)
}

function Test-UatA9ExternalLocksAndRetry {
    $candidate = $script:builds['fresh']
    $script:testRoot = $candidate.installRoot
    $obsoletePath = Join-Path $script:testRoot 'legacy-test-only.txt'
    $lockPaths = @(
        (Join-Path $script:testRoot 'SDIR.exe'),
        (Join-Path $script:testRoot 'docs\distribution-portable-reference.md'),
        (Join-Path $script:testRoot 'unins000.dat')
    )
    $obsoleteAvailable = Test-Path -LiteralPath $obsoletePath -PathType Leaf
    if ($obsoleteAvailable) { $lockPaths += $obsoletePath }
    $evidence = New-Object System.Collections.Generic.List[string]
    $preRoot = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    foreach ($path in $lockPaths) {
        Assert-Uat (Test-Path -LiteralPath $path -PathType Leaf) ('A9 lock fixture is missing: {0}' -f $path)
        $stream = New-Object System.IO.FileStream -ArgumentList @($path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
        try {
            $name = 'A9-exclusive-lock-' + ([System.IO.Path]::GetFileName($path) -replace '[^A-Za-z0-9._-]', '-')
            $result = Invoke-UatSilentExpectedRefusal -Candidate $candidate -Name $name -ExpectedLogPattern 'SDIR_PREFLIGHT_REFUSED'
            foreach ($item in $result.evidence) { [void]$evidence.Add($item) }
            Test-UatSnapshotEqual -Expected $preRoot -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message ('A9 external lock changed installation state: ' + $path)
        }
        finally { $stream.Dispose() }
    }
    $retry = Invoke-UatWizardInstall -Candidate $candidate -Name 'A9-release-locks-retry' -DesktopShortcut:$false
    Assert-Uat ($retry.setupExitCode -eq 0) 'The retry after releasing real external locks did not succeed.'
    $retryPayload = Assert-UatInstalledPayload -Candidate $candidate -Root $script:testRoot -DesktopExpected:$false
    $obsoleteDetails = if ($obsoleteAvailable) { 'The real compiled historical path was exclusively locked and refused safely.' } else { 'A9 could not exercise the obsolete-file lock because this current fresh stable installation contains no baseline-owned historical file; that subcase remains pending pending the A7 baseline evidence.' }
    $status = if ($obsoleteAvailable) { 'verified' } else { 'pending' }
    $script:a9CoreVerified = $true
    $script:a9Evidence = @($evidence.ToArray()) + @($retry.evidence) + @($retry.logPath)
    Set-UatCase -Id 'A9' -Status $status -Evidence (@($evidence.ToArray()) + @($retry.evidence) + @($retry.logPath)) -Details @('External exclusive locks on SDIR.exe, the staged distribution document, and native uninstall metadata each caused a source-logged nonzero refusal; all full root/registry/shortcut snapshots matched after exit; releasing locks allowed a real setup retry.', $obsoleteDetails)
    return @($evidence.ToArray()) + @($retry.evidence) + @($retry.logPath)
}

function Test-UatA10CancellationRollback {
    $candidate = $script:builds['fresh']
    $script:testRoot = $candidate.installRoot
    $beforeProfile = Get-UatTreeSnapshot -Path $script:profileRoot
    $beforeInstall = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    $beforePath = Join-Path $providedEvidencePath 'A10-before-cancel-before-mutation.json'
    Write-UatJsonFile -Path $beforePath -Value $beforeInstall
    $beforeCancel = Invoke-UatWizardInstall -Candidate $candidate -Name 'A10-cancel-before-mutation' -DesktopShortcut:$false -CancelAtReady
    Assert-Uat ($beforeCancel.cancelled -and $beforeCancel.phase -eq 'before-mutation') 'A10 did not send actual native cancel input at the ready page.'
    Test-UatSnapshotEqual -Expected $beforeInstall -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message 'A10 cancellation before mutation changed install/metadata/registry/shortcuts.'
    Test-UatSnapshotEqual -Expected $beforeProfile -Actual (Get-UatTreeSnapshot -Path $script:profileRoot) -Message 'A10 ready-page cancellation changed the isolated profile.'
    $afterCancelPath = Join-Path $providedEvidencePath 'A10-after-cancel-before-mutation.json'
    Write-UatJsonFile -Path $afterCancelPath -Value (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut)

    $duringBefore = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    $duringBeforePath = Join-Path $providedEvidencePath 'A10-before-cancel-during-copy.json'
    Write-UatJsonFile -Path $duringBeforePath -Value $duringBefore
    $during = Invoke-UatWizardInstall -Candidate $candidate -Name 'A10-cancel-during-real-copy' -DesktopShortcut:$false -CancelDuringInstall -TimeoutSeconds 240
    Assert-Uat ($during.cancelled -and $during.phase -eq 'after-quarantine-partial-copy' -and $during.partialObserved) 'A10 did not cancel after actual quarantine and a partial payload copy.'
    Assert-Uat ($during.exitCode -ne $null) 'A10 cancellation process exit code was not observed after process exit.'
    $native = Read-UatRawLog -Path $during.logPath
    Assert-Uat ($native.text.Contains('SDIR_QUARANTINED:')) 'A10 cancellation log did not prove a real quarantine had begun.'
    $duringAfter = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    Test-UatSnapshotEqual -Expected $duringBefore -Actual $duringAfter -Message 'A10 post-copy cancellation failed exact bytes, NTFS metadata, stream, ACL, directory, receipt, registry, or shortcut restoration.'
    Test-UatSnapshotEqual -Expected $beforeProfile -Actual (Get-UatTreeSnapshot -Path $script:profileRoot) -Message 'A10 cancellation changed isolated profile bytes.'
    $duringAfterPath = Join-Path $providedEvidencePath 'A10-after-cancel-during-copy.json'
    Write-UatJsonFile -Path $duringAfterPath -Value $duringAfter
    $evidence = @($beforeCancel.evidence) + @($during.evidence) + @($during.logPath, $beforePath, $afterCancelPath, $duringBeforePath, $duringAfterPath)
    Set-UatCase -Id 'A10' -Status 'verified' -Evidence $evidence -Details @('The real owned wizard received Cancel before mutation and again after the setup log and root inventory proved quarantine plus partial payload copy; both comparisons ran only after the setup process exited and matched complete file bytes, defined NTFS metadata/streams/owner-DACL, directory metadata, receipt, registration, and shortcuts.')
    return $evidence
}

function Test-UatA11FaultRollback {
    $candidate = $script:builds['fresh']
    $script:testRoot = $candidate.installRoot
    $activeRootSnapshot = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    Assert-Uat $activeRootSnapshot.root.exists 'A11 candidate install root is missing before rollback checks.'
    Assert-Uat $activeRootSnapshot.registry.exists 'A11 active test registration is missing before rollback checks.'
    $installLocationEntries = @($activeRootSnapshot.registry.values | Where-Object { [string]::Equals([string]$_.name, 'InstallLocation', [System.StringComparison]::OrdinalIgnoreCase) })
    Assert-Uat ($installLocationEntries.Count -eq 1) 'A11 active HKCU registration must contain exactly one InstallLocation.'
    $registeredUninstallerPath = Get-UatUninstallerPath
    $expectedRoot = [System.IO.Path]::GetFullPath([string]$candidate.installRoot).TrimEnd('\')
    $expectedUninstallerPath = [System.IO.Path]::GetFullPath((Join-Path $candidate.installRoot 'unins000.exe'))
    Assert-Uat ([string]::Equals($registeredUninstallerPath, $expectedUninstallerPath, [System.StringComparison]::OrdinalIgnoreCase)) 'A11 registered native uninstaller does not match the candidate root.'
    $rootPaths = @([string]$candidate.installRoot, [string]$script:testRoot, [string]$activeRootSnapshot.root.path, [string]$installLocationEntries[0].value, (Split-Path -Parent $registeredUninstallerPath))
    foreach ($rootPath in $rootPaths) {
        $resolvedRoot = [System.IO.Path]::GetFullPath([string]$rootPath).TrimEnd('\')
        Assert-Uat ([string]::Equals($resolvedRoot, $expectedRoot, [System.StringComparison]::OrdinalIgnoreCase)) ('A11 candidate, snapshot, active registration, and native uninstaller roots do not agree: {0}' -f $rootPath)
    }
    $beforeProfile = Get-UatTreeSnapshot -Path $script:profileRoot
    $evidence = New-Object System.Collections.Generic.List[string]
    foreach ($fault in @('Backup', 'Partial', 'Late')) {
        $faultCandidate = $script:builds[('fault-' + $fault.ToLowerInvariant())]
        Assert-Uat ($faultCandidate.testFault -eq $fault -and $faultCandidate.receipt.testBuild) ('Fault candidate identity is not the expected TEST_BUILD define: {0}' -f $fault)
        $before = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
        $beforePath = Join-Path $providedEvidencePath ('A11-' + $fault.ToLowerInvariant() + '-before.json')
        Write-UatJsonFile -Path $beforePath -Value $before
        $run = Invoke-UatSilentSetup -Candidate $faultCandidate -Name ('A11-fault-' + $fault.ToLowerInvariant())
        Assert-UatExpectedRefusal -Result $run.command -Name ('A11-fault-' + $fault.ToLowerInvariant())
        $native = Read-UatRawLog -Path $run.logPath
        $expectedMarker = switch ($fault) {
            'Backup' { 'SDIR_TEST_FAULT_BACKUP_AFTER_FIRST_MOVE' }
            'Partial' { 'SDIR_TEST_FAULT_PARTIAL_AFTER_FIRST_PAYLOAD' }
            'Late' { 'SDIR_TEST_FAULT_LATE_BEFORE_FINALIZATION' }
        }
        Assert-Uat ($native.text.Contains($expectedMarker)) ('Fault candidate did not execute its compiled-only hook: {0}' -f $fault)
        $after = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
        Test-UatSnapshotEqual -Expected $before -Actual $after -Message ('A11 rollback failed full restoration after the ' + $fault + ' fault.')
        Test-UatSnapshotEqual -Expected $beforeProfile -Actual (Get-UatTreeSnapshot -Path $script:profileRoot) -Message ('A11 ' + $fault + ' fault changed isolated user profile bytes.')
        $afterPath = Join-Path $providedEvidencePath ('A11-' + $fault.ToLowerInvariant() + '-after.json')
        Write-UatJsonFile -Path $afterPath -Value $after
        $summary = Join-Path $providedEvidencePath ('A11-' + $fault.ToLowerInvariant() + '-assertion.json')
        Write-UatJsonFile -Path $summary -Value @{ fault = $fault; exitCode = $run.exitCode; expectedNonzero = $true; faultMarker = $expectedMarker; markerFound = $true; restoredSnapshotEqual = $true; rawLogSha256 = $native.sha256; rawLogEncoding = $native.encoding; processEvidence = $run.evidence }
        foreach ($path in @($run.evidence, $run.logPath, $beforePath, $afterPath, $summary)) { [void]$evidence.Add($path) }
    }
    Set-UatCase -Id 'A11' -Status 'verified' -Evidence @($evidence.ToArray()) -Details @('Backup-after-first-move, partial-after-first-payload, and late-before-finalization were separate TEST_BUILD candidates; each returned a verified nonzero process exit, emitted its exact source marker, and restored the complete actual filesystem/NTFS/receipt/registry/shortcut snapshot after process exit.')
    return @($evidence.ToArray())
}

function Test-UatA13PreflightRefusals {
    $candidate = $script:builds['fresh']
    $script:testRoot = $candidate.installRoot
    $evidence = New-Object System.Collections.Generic.List[string]
    $baseline = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut

    $wrongRoot = Join-Path (Join-Path $env:LOCALAPPDATA 'Programs') ($script:testRootName + '-WRONG-DIR')
    Assert-Uat (-not (Test-Path -LiteralPath $wrongRoot)) 'The wrong-/DIR refusal fixture already exists.'
    $wrongDir = Invoke-UatSilentExpectedRefusal -Candidate $candidate -Name 'A13-wrong-DIR-refusal' -ExpectedLogPattern 'SDIR_PREFLIGHT_REFUSED' -AdditionalArgument @('/DIR=' + $wrongRoot)
    Assert-Uat (-not (Test-Path -LiteralPath $wrongRoot)) 'The forbidden /DIR override created the requested alternate path.'
    Test-UatSnapshotEqual -Expected $baseline -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message 'Wrong /DIR refusal changed installed state.'
    foreach ($path in $wrongDir.evidence) { [void]$evidence.Add($path) }

    $receiptPath = Join-Path $script:testRoot 'sdir.install-receipt'
    $receiptRecovery = [pscustomobject][ordered]@{
        name = 'A13-malformed-receipt'
        kind = 'RestoreFile'
        path = $receiptPath
        fileState = $null
        backupPath = (Join-Path $providedEvidencePath 'A13-malformed-receipt.fixture-backup.bin')
        junctionTarget = $null
        beforeSnapshot = $null
        stream = $null
        streamState = 'not-open'
        mutationStarted = $false
        backupMoved = $false
        status = 'active'
        commandId = $null
        reason = $null
        observationError = $null
        originalError = $null
        cleanupError = $null
        evidenceError = $null
        createdUtc = [DateTime]::UtcNow.ToString('o')
        completedUtc = $null
    }
    [void]$script:retainedRecoveryResources.Add($receiptRecovery)
    try {
        $receiptBackup = Save-UatFileFixtureState -Path $receiptPath -Name 'A13-malformed-receipt'
        if ($null -eq $receiptBackup) { throw 'Malformed receipt fixture backup was not acquired.' }
        $receiptRecovery.fileState = $receiptBackup
        $receiptRecovery.backupPath = [string]$receiptBackup.backup
        $receiptRecovery.mutationStarted = $true
        [System.IO.File]::WriteAllBytes($receiptPath, [System.Text.Encoding]::ASCII.GetBytes(('NOT-A-SDIR-RECEIPT' + [char]13 + [char]10)))
        $malformedState = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
        $receiptRecovery.beforeSnapshot = $malformedState
        $result = Invoke-UatSilentExpectedRefusal -Candidate $candidate -Name 'A13-malformed-receipt-refusal' -ExpectedLogPattern 'SDIR_PREFLIGHT_REFUSED'
        Test-UatSnapshotEqual -Expected $malformedState -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message 'Malformed receipt refusal changed other install state.'
        foreach ($path in $result.evidence) { [void]$evidence.Add($path) }
    }
    catch {
        $receiptRecovery.originalError = $_
        throw
    }
    finally {
        if ($null -eq $receiptRecovery.fileState) {
            $receiptRecovery.status = 'recoverypending'
            $receiptRecovery.reason = 'fixture backup was not acquired; original file state cannot be restored'
            if ($null -eq $receiptRecovery.originalError) { throw ('A13 malformed receipt recovery is pending; backup unavailable: {0}' -f $receiptRecovery.path) }
        }
        else {
            try {
                $receiptRestored = Restore-UatFileFixtureState -State $receiptRecovery.fileState -RecoveryRecord $receiptRecovery
                if (-not $receiptRestored -and $null -eq $receiptRecovery.originalError) { throw ('A13 malformed receipt recovery is pending for {0}; backup retained.' -f $receiptRecovery.path) }
            }
            catch {
                $receiptRecovery.cleanupError = $_
                if ($null -eq $receiptRecovery.originalError) { throw }
            }
        }
    }
    Test-UatSnapshotEqual -Expected $baseline -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message 'Restoring the malformed receipt fixture did not recover its exact original metadata and bytes.'

    $ownedPath = Join-Path $script:testRoot 'docs\verification.md'
    $ownedRecovery = [pscustomobject][ordered]@{
        name = 'A13-modified-owned-file'
        kind = 'RestoreFile'
        path = $ownedPath
        fileState = $null
        backupPath = (Join-Path $providedEvidencePath 'A13-modified-owned-file.fixture-backup.bin')
        junctionTarget = $null
        beforeSnapshot = $null
        stream = $null
        streamState = 'not-open'
        mutationStarted = $false
        backupMoved = $false
        status = 'active'
        commandId = $null
        reason = $null
        observationError = $null
        originalError = $null
        cleanupError = $null
        evidenceError = $null
        createdUtc = [DateTime]::UtcNow.ToString('o')
        completedUtc = $null
    }
    [void]$script:retainedRecoveryResources.Add($ownedRecovery)
    try {
        $ownedBackup = Save-UatFileFixtureState -Path $ownedPath -Name 'A13-modified-owned-file'
        if ($null -eq $ownedBackup) { throw 'Modified owned file fixture backup was not acquired.' }
        $ownedRecovery.fileState = $ownedBackup
        $ownedRecovery.backupPath = [string]$ownedBackup.backup
        $ownedRecovery.mutationStarted = $true
        [System.IO.File]::AppendAllText($ownedPath, ([string][char]13 + [string][char]10 + 'UAT-MODIFIED-OWNED-FOOTER' + [string][char]13 + [string][char]10), [System.Text.Encoding]::ASCII)
        $modifiedState = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
        $ownedRecovery.beforeSnapshot = $modifiedState
        $result = Invoke-UatSilentExpectedRefusal -Candidate $candidate -Name 'A13-modified-owned-file-refusal' -ExpectedLogPattern 'SDIR_PREFLIGHT_REFUSED'
        Test-UatSnapshotEqual -Expected $modifiedState -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message 'Modified-owned-file refusal changed bytes or owned installation metadata.'
        foreach ($path in $result.evidence) { [void]$evidence.Add($path) }
    }
    catch {
        $ownedRecovery.originalError = $_
        throw
    }
    finally {
        if ($null -eq $ownedRecovery.fileState) {
            $ownedRecovery.status = 'recoverypending'
            $ownedRecovery.reason = 'fixture backup was not acquired; original file state cannot be restored'
            if ($null -eq $ownedRecovery.originalError) { throw ('A13 modified owned file recovery is pending; backup unavailable: {0}' -f $ownedRecovery.path) }
        }
        else {
            try {
                $ownedRestored = Restore-UatFileFixtureState -State $ownedRecovery.fileState -RecoveryRecord $ownedRecovery
                if (-not $ownedRestored -and $null -eq $ownedRecovery.originalError) { throw ('A13 modified owned file recovery is pending for {0}; backup retained.' -f $ownedRecovery.path) }
            }
            catch {
                $ownedRecovery.cleanupError = $_
                if ($null -eq $ownedRecovery.originalError) { throw }
            }
        }
    }
    Test-UatSnapshotEqual -Expected $baseline -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message 'Restoring the modified-owned-file fixture did not recover exact original bytes and metadata.'

    $docsPath = Join-Path $script:testRoot 'docs'
    $docsBackup = Join-Path $providedEvidencePath 'A13-docs-original-directory'
    Assert-Uat (-not (Test-Path -LiteralPath $docsBackup)) 'The owned docs recovery directory already exists.'
    $docsBefore = Get-UatTreeSnapshot -Path $docsPath
    [System.IO.Directory]::Move($docsPath, $docsBackup)
    try {
        New-Item -ItemType Junction -Path $docsPath -Target $script:profileRoot | Out-Null
        $reparseState = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut -AllowReparse
        $result = Invoke-UatSilentExpectedRefusal -Candidate $candidate -Name 'A13-reparse-traversal-refusal' -ExpectedLogPattern 'SDIR_PREFLIGHT_REFUSED'
        Test-UatSnapshotEqual -Expected $reparseState -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut -AllowReparse) -Message 'Reparse refusal changed the owned install root or followed the profile target.'
        foreach ($path in $result.evidence) { [void]$evidence.Add($path) }
    }
    finally {
        if (Test-Path -LiteralPath $docsPath) {
            $docsItem = Get-Item -LiteralPath $docsPath -Force
            if (($docsItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { [System.IO.Directory]::Delete($docsPath, $false) }
            elseif (Test-Path -LiteralPath $docsPath -PathType Container) { throw 'The installer changed the reparse fixture into a non-owned directory; recovery source retained for manual recovery.' }
        }
        if ((Test-Path -LiteralPath $docsBackup -PathType Container) -and -not (Test-Path -LiteralPath $docsPath)) { [System.IO.Directory]::Move($docsBackup, $docsPath) }
    }
    Test-UatSnapshotEqual -Expected $docsBefore -Actual (Get-UatTreeSnapshot -Path $docsPath) -Message 'A13 reparse fixture restoration changed the original docs directory.'
    Test-UatSnapshotEqual -Expected $baseline -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message 'Restoring the reparse fixture did not recover the original installation snapshot.'

    $collisionCandidate = $script:builds['silent']
    $collisionRoot = $collisionCandidate.installRoot
    Assert-Uat (-not (Test-Path -LiteralPath $collisionRoot)) 'Unowned/collision test root was not fresh.'
    New-Item -ItemType Directory -Path $collisionRoot | Out-Null
    $marker = Write-UatRootOwnerMarker -Root $collisionRoot -Purpose 'unowned-collision-refusal-fixture'
    $fakeApp = Join-Path $collisionRoot 'SDIR.exe'
    [System.IO.File]::WriteAllBytes($fakeApp, [byte[]](0x55, 0x41, 0x54, 0x2D, 0x4E, 0x4F, 0x54, 0x2D, 0x53, 0x44, 0x49, 0x52))
    $unknown = Join-Path $collisionRoot 'unknown-collision.bin'
    [void](Add-UatTestSentinel -Path $unknown -Text ('SDIR-UAT-UNKNOWN-COLLISION-' + $script:runId))
    $collisionBefore = Get-UatTreeSnapshot -Path $collisionRoot
    $collisionResult = Invoke-UatSilentExpectedRefusal -Candidate $collisionCandidate -Name 'A13-unowned-root-collision-refusal' -ExpectedLogPattern 'SDIR_PREFLIGHT_REFUSED'
    Test-UatSnapshotEqual -Expected $collisionBefore -Actual (Get-UatTreeSnapshot -Path $collisionRoot) -Message 'Unowned/colliding root refusal modified its preexisting fixture.'
    foreach ($path in $collisionResult.evidence) { [void]$evidence.Add($path) }
    $collisionEvidence = Join-Path $providedEvidencePath 'A13-unowned-collision-fixture.json'
    Write-UatJsonFile -Path $collisionEvidence -Value @{ ownerMarker = $marker; fakeExecutable = $fakeApp; fakeExecutableSha256 = Get-UatFileHash $fakeApp; unknownCollision = $unknown; rootSnapshot = $collisionBefore; unchangedAfterRefusal = $true }
    [void]$evidence.Add($collisionEvidence)

    Set-UatCase -Id 'A13' -Status 'verified' -Evidence @($evidence.ToArray()) -Details @('Forbidden /DIR override, malformed receipt, modified owned file, actual docs-to-owned-profile junction, portable.flag destination, and an owned unrecognized root with SDIR.exe/unknown-file collisions all refused with nonzero source-logged results and exact before/after snapshots; temporary corruption and junction fixtures were restored from exact recovery data.')
    return @($evidence.ToArray())
}

function Test-UatA15LockedUninstallAndReinstall {
    param([switch]$RestoreOnly)
    $candidate = $script:builds['fresh']
    $script:testRoot = [System.IO.Path]::GetFullPath([string]$candidate.installRoot)
    $script:testRootName = [string]$candidate.rootName
    Assert-Uat $RestoreOnly.IsPresent 'A15 requires the reviewed RestoreOnly route.'
    Assert-Uat (Test-Path -LiteralPath $script:testRoot -PathType Container) 'A15 RestoreOnly requires the completed fresh install root.'
    Assert-UatRootOwnerMarker -Root $script:testRoot | Out-Null
    Add-UatEvent -Kind 'a15-restore-only-started' -Name 'A15' -Data @{
        lockRefusalPerformed = $false; lockRefusalClaimed = $false
        scope = 'normal close, first registered removal, reinstall, restored UI and persisted profile, terminal registered removal'
    }
    [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
    $app = Start-UatDirectApp -Root $script:testRoot -Name 'A15-save-ui-before-uninstall'
    $routeEvidence = Test-UatLaunchRoute -App $app -Name 'A15-saved-ui-state'
    $settingsPath = [string]$script:profileFixture.settings
    $savedSettingsHash = Get-UatFileHash $settingsPath
    [void](Assert-UatA15RegisteredRemoval -Candidate $candidate)
    $markerPath = Assert-UatRootOwnerMarker -Root $script:testRoot
    $markerSha256 = Get-UatFileHash $markerPath
    $uninstallProfileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    $firstRemoveBefore = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    $firstRemoveBeforePath = Join-Path $providedEvidencePath 'A15-first-remove-before.json'
    Write-UatJsonFile -Path $firstRemoveBeforePath -Value ([ordered]@{
        installSnapshot = $firstRemoveBefore
        root = $script:testRoot
        profileSnapshot = $uninstallProfileBefore
        settingsPath = $settingsPath
        settingsSha256 = $savedSettingsHash
        rootOwnerMarkerPath = $markerPath
        rootOwnerMarkerSha256 = $markerSha256
        markerRemovalIntent = 'remove the owned marker before the first registered uninstall so the native uninstaller can remove its test program root'
        normalCloseEvidence = @($routeEvidence)
    })
    Add-UatEvent -Kind 'root-marker-removal-intended' -Name 'A15-uninstall-for-reinstall' -Data @{ path = $markerPath; sha256 = $markerSha256; preRemovalEvidence = $firstRemoveBeforePath }
    Assert-NoReparseComponents -Path $markerPath
    Assert-Uat ((Get-UatFileHash $markerPath) -ceq $markerSha256) 'The A15 owner marker changed before removal.'
    [System.IO.File]::Delete($markerPath)
    $a15UninstallBudget = New-UatCompletionBudget -TotalMilliseconds ([long]90000) -Origin 'A15 uninstall before reinstall route'
    $uninstaller = Start-UatUninstallerGui -Name 'A15-uninstall-for-reinstall' -CompletionBudget $a15UninstallBudget
    $removed = Resolve-UatUninstallerConfirmation -Uninstaller $uninstaller -Choice 'remove'
    Assert-Uat ($removed.exitCode -eq 0) 'A15 actual registered uninstall failed after the app saved its UI state.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:testRoot)) 'A15 first uninstall did not remove its owned program root.'
    $firstRemoveAfter = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    Assert-Uat (-not $firstRemoveAfter.registry.exists) 'A15 first uninstall left its registration before reinstall.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A15 first uninstall left a shortcut before reinstall.'
    Assert-Uat ((Get-UatFileHash $settingsPath) -ceq $savedSettingsHash) 'A15 first uninstall changed persisted UI settings.'
    Test-UatSnapshotEqual -Expected $uninstallProfileBefore -Actual (Get-UatTreeSnapshot -Path $script:profileRoot) -Message 'A15 first uninstall changed the isolated user profile.'
    $firstRemoveAfterPath = Join-Path $providedEvidencePath 'A15-first-remove-after.json'
    Write-UatJsonFile -Path $firstRemoveAfterPath -Value ([ordered]@{
        installSnapshot = $firstRemoveAfter
        rootAbsent = -not (Test-Path -LiteralPath $script:testRoot)
        registrationAbsent = -not [bool]$firstRemoveAfter.registry.exists
        linksAbsent = -not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)
        profileSnapshot = Get-UatTreeSnapshot -Path $script:profileRoot
        settingsPath = $settingsPath
        settingsSha256 = Get-UatFileHash $settingsPath
        firstRemoveExitCode = [int]$removed.exitCode
        firstRemoveCommandEvidence = $removed.evidence.directory
    })
    $reinstallProfileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    $reinstallProfileBeforePath = Join-Path $providedEvidencePath 'A15-reinstall-profile-before.json'
    Write-UatJsonFile -Path $reinstallProfileBeforePath -Value $reinstallProfileBefore
    [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
    $reinstall = Invoke-UatWizardInstall -Candidate $candidate -Name 'A15-reinstall-restored-ui' -DesktopShortcut:$false -LaunchAfterInstall:$false
    Assert-Uat ($reinstall.setupExitCode -eq 0) 'A15 stable reinstall failed after a genuine registered uninstall.'
    Assert-Uat ($null -eq $reinstall.app) 'A15 restore-only reinstall unexpectedly launched the app from setup.'
    $reinstalledPayload = Assert-UatInstalledPayload -Candidate $candidate -Root $script:testRoot
    $rootMarker = Write-UatRootOwnerMarker -Root $script:testRoot -Purpose 'uninstall-reinstall-ui-restoration'
    $reinstallProfileAfter = Get-UatTreeSnapshot -Path $script:profileRoot
    Test-UatSnapshotEqual -Expected $reinstallProfileBefore -Actual $reinstallProfileAfter -Message 'A15 reinstall modified saved profile bytes before app launch.'
    Assert-Uat ((Get-UatFileHash $settingsPath) -ceq $savedSettingsHash) 'A15 reinstall changed the persisted settings JSON before app launch.'
    $reinstallProfileAfterPath = Join-Path $providedEvidencePath 'A15-reinstall-profile-after.json'
    Write-UatJsonFile -Path $reinstallProfileAfterPath -Value $reinstallProfileAfter
    [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
    $reinstallApp = Start-UatDirectApp -Root $script:testRoot -Name 'A15-verify-restored-ui'
    $restoreEvidence = Test-UatLaunchRoute -App $reinstallApp -Name 'A15-restored-ui-after-reinstall'
    $restoredProfile = Get-UatTreeSnapshot -Path $script:profileRoot
    $restoredSettingsHash = Get-UatFileHash $settingsPath
    Assert-Uat ($restoredSettingsHash -ceq $savedSettingsHash) 'A15 restored UI did not preserve the exact persisted settings JSON bytes.'
    $savedContentIdentity = Get-UatTreeContentIdentity -Snapshot $uninstallProfileBefore
    $restoredContentIdentity = Get-UatTreeContentIdentity -Snapshot $restoredProfile
    Assert-UatEqual $savedContentIdentity $restoredContentIdentity 'A15 restored UI changed the saved profile file or stream hashes.'
    $restoredProfilePath = Join-Path $providedEvidencePath 'A15-restored-profile-after-close.json'
    Write-UatJsonFile -Path $restoredProfilePath -Value $restoredProfile
    $restoreRecordPath = Join-Path $providedEvidencePath 'A15-restore-only-measurement.json'
    Write-UatJsonFile -Path $restoreRecordPath -Value ([ordered]@{
        caseId = 'A15'
        restoreOnly = [bool]$RestoreOnly
        lockRefusalPerformed = $false
        lockRefusalClaimed = $false
        persistedContentComparisonExcludes = @('webview')
        firstRegisteredRemoveExitCode = [int]$removed.exitCode
        firstRegisteredRemoveCommandEvidence = $removed.evidence.directory
        reinstallExitCode = [int]$reinstall.setupExitCode
        candidateName = [string]$candidate.name
        candidateRoot = [string]$candidate.installRoot
        candidateExePath = [string]$candidate.path
        candidateExeSha256 = [string]$candidate.sha256
        candidateReceiptPath = [string]$candidate.receiptPath
        candidateReceiptSha256 = [string]$candidate.receiptSha256
        reinstalledPayload = $reinstalledPayload
        firstRemoveBeforeEvidence = $firstRemoveBeforePath
        firstRemoveAfterEvidence = $firstRemoveAfterPath
        profileBeforeFirstRemove = $uninstallProfileBefore
        profileBeforeReinstall = $reinstallProfileBefore
        profileAfterReinstallBeforeLaunch = $reinstallProfileAfter
        profileAfterRestoredUiClose = $restoredProfile
        settingsPath = $settingsPath
        savedSettingsSha256 = $savedSettingsHash
        restoredSettingsSha256 = $restoredSettingsHash
        rootMarkerPath = $rootMarker
        rootMarkerSha256 = Get-UatFileHash $rootMarker
        normalCloseEvidence = @($routeEvidence) + @($restoreEvidence)
        measuredScope = 'fresh wizard install, normal app close and save, first registered removal, reinstall, restored UI and persisted JSON'
        unmeasuredScope = 'external lock refusal, ordinary running app refusal, A14'
    })
    $evidence = @($routeEvidence) + @($firstRemoveBeforePath, $uninstaller.screenshot, $uninstaller.logPath, $removed.evidence.directory, $firstRemoveAfterPath, $reinstallProfileBeforePath) + @($reinstall.evidence) + @($reinstall.logPath, $reinstallProfileAfterPath, $rootMarker) + @($restoreEvidence, $restoredProfilePath, $restoreRecordPath)
    Set-UatCase -Id 'A15' -Status 'pending' -Evidence $evidence -Details @('RestoreOnly measured the fresh install, normal UI save/close, first registered removal, reinstall, restored UI and persisted profile, and normal close. No lock refusal or ordinary running-app refusal was executed; A15 remains pending for those acceptance conditions.') -DeferResultWrite
    return $evidence
}

function Invoke-UatA15FreshRestoreOnlyEntry {
    $candidate = $script:builds['fresh']
    $script:testRoot = [System.IO.Path]::GetFullPath([string]$candidate.installRoot)
    $script:testRootName = [string]$candidate.rootName
    Assert-Uat (-not (Test-Path -LiteralPath $script:testRoot)) 'A15 fresh candidate root is not new.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A15 fresh install cannot start while the fixed TEST AppID is registered.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A15 fresh install cannot start while a TEST shortcut exists.'
    $profileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    $profileBeforePath = Join-Path $providedEvidencePath 'A15-fresh-install-profile-before.json'
    Write-UatJsonFile -Path $profileBeforePath -Value $profileBefore
    [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
    $fresh = Invoke-UatWizardInstall -Candidate $candidate -Name 'A15-diagnostic-fresh-install' -DesktopShortcut:$false -LaunchAfterInstall:$false
    Assert-Uat ($fresh.setupExitCode -eq 0) 'A15 fresh wizard install did not return actual exit code zero.'
    Assert-Uat ($null -eq $fresh.app) 'A15 fresh wizard install unexpectedly launched the app.'
    $payload = Assert-UatInstalledPayload -Candidate $candidate -Root $script:testRoot
    $profileAfter = Get-UatTreeSnapshot -Path $script:profileRoot
    Test-UatSnapshotEqual -Expected $profileBefore -Actual $profileAfter -Message 'A15 fresh wizard install changed the isolated profile before app launch.'
    $rootMarker = Write-UatRootOwnerMarker -Root $script:testRoot -Purpose 'a15-diagnostic-fresh-install'
    $profileAfterPath = Join-Path $providedEvidencePath 'A15-fresh-install-profile-after.json'
    Write-UatJsonFile -Path $profileAfterPath -Value $profileAfter
    $setupEvidencePath = Join-Path $providedEvidencePath 'A15-fresh-install-measurement.json'
    Write-UatJsonFile -Path $setupEvidencePath -Value ([ordered]@{
        caseId = 'A15'; candidateName = [string]$candidate.name
        candidateRoot = [string]$candidate.installRoot
        candidateExePath = [string]$candidate.path
        candidateExeSha256 = [string]$candidate.sha256
        candidateReceiptPath = [string]$candidate.receiptPath
        candidateReceiptSha256 = [string]$candidate.receiptSha256
        setupExitCode = [int]$fresh.setupExitCode
        desktopShortcutEnabled = $false
        launchAfterInstall = $false
        installedPayload = $payload
        profileBefore = $profileBefore
        profileAfter = $profileAfter
        rootMarkerPath = $rootMarker
        rootMarkerSha256 = Get-UatFileHash $rootMarker
        setupEvidence = @($fresh.evidence)
        setupLogPath = $fresh.logPath
    })
    $script:a15Evidence = @($profileBeforePath, $profileAfterPath) + @($fresh.evidence) + @($fresh.logPath, $setupEvidencePath)
    $restoreEvidence = Test-UatA15LockedUninstallAndReinstall -RestoreOnly
    $script:a15Evidence = @($script:a15Evidence) + @($restoreEvidence)
    return $script:a15Evidence
}

function Assert-UatA15RegisteredRemoval {
    param([Parameter(Mandatory = $true)]$Candidate)
    [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
    $expectedRoot = [System.IO.Path]::GetFullPath([string]$Candidate.installRoot)
    Assert-Uat ([string]::Equals($script:testRoot, $expectedRoot, [System.StringComparison]::OrdinalIgnoreCase) -and $script:testRootName -ceq [string]$Candidate.rootName) 'A15 registered removal root did not match the selected fresh candidate.'
    Assert-NoReparseComponents -Path $expectedRoot
    Assert-UatRootOwnerMarker -Root $expectedRoot | Out-Null
    Assert-UatNoRunningTestApp -Root $expectedRoot
    [void](Assert-UatInstalledPayload -Candidate $Candidate -Root $expectedRoot)
    $registration = Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey
    Assert-Uat $registration.exists 'A15 final remove requires the second fresh registration.'
    $installLocations = @($registration.values | Where-Object { [string]::Equals([string]$_.name, 'InstallLocation', [System.StringComparison]::OrdinalIgnoreCase) })
    Assert-Uat ($installLocations.Count -eq 1) 'A15 final registration must contain exactly one InstallLocation.'
    Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$installLocations[0].value), $expectedRoot, [System.StringComparison]::OrdinalIgnoreCase)) 'A15 final InstallLocation did not match the exact owned candidate root.'
    $uninstallerPath = [System.IO.Path]::GetFullPath((Get-UatUninstallerPath))
    $expectedUninstaller = [System.IO.Path]::GetFullPath((Join-Path $expectedRoot 'unins000.exe'))
    Assert-Uat ([string]::Equals($uninstallerPath, $expectedUninstaller, [System.StringComparison]::OrdinalIgnoreCase)) 'A15 final registered uninstaller path did not match the owned candidate.'
    Assert-NoReparseComponents -Path $uninstallerPath
    $uninstallerSha256 = Get-UatFileHash $uninstallerPath
    $startTarget = Get-UatTestShortcutTarget -Path $script:startMenuShortcut
    $expectedLauncher = [System.IO.Path]::GetFullPath((Join-Path $expectedRoot 'sdir-installer-test-launcher.cmd'))
    Assert-Uat ([string]::Equals($startTarget.targetPath, $expectedLauncher, [System.StringComparison]::OrdinalIgnoreCase)) 'A15 final Start Menu link target did not match the candidate launcher.'
    Assert-Uat ([string]::Equals($startTarget.workingDirectory, $expectedRoot, [System.StringComparison]::OrdinalIgnoreCase)) 'A15 final Start Menu link working directory did not match the candidate root.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:desktopShortcut)) 'A15 final removal found an unexpected desktop link.'
    [void](Assert-UatTrackedProcessResults -CaseId 'A15')
    $unrecovered = @($script:retainedRecoveryResources | Where-Object { $_.status -ne 'recovered' })
    Assert-Uat ($unrecovered.Count -eq 0) 'A15 final removal found an unrecovered fixture.'
    return [pscustomobject]@{
        root = $expectedRoot; registration = $registration; installLocation = [string]$installLocations[0].value
        uninstallerPath = $uninstallerPath; uninstallerSha256 = $uninstallerSha256; startMenuTarget = $startTarget
    }
}

function Invoke-UatA15DiagnosticFinalRemove {
    $candidate = $script:builds['fresh']
    $identity = Assert-UatA15RegisteredRemoval -Candidate $candidate
    $expectedRoot = $identity.root
    $profileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    $markerPath = Assert-UatRootOwnerMarker -Root $expectedRoot
    $markerSha256 = Get-UatFileHash $markerPath
    $installSnapshotBefore = Get-UatInstallSnapshot -Root $expectedRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    $beforePath = Join-Path $providedEvidencePath 'A15-final-remove-before.json'
    Write-UatJsonFile -Path $beforePath -Value ([ordered]@{
        candidateRoot = $expectedRoot
        candidateName = [string]$candidate.name
        runOwnerPath = $script:runOwnerPath
        rootOwnerMarkerPath = $markerPath
        rootOwnerMarkerSha256 = $markerSha256
        candidateReceiptPath = [string]$candidate.receiptPath
        candidateReceiptSha256 = [string]$candidate.receiptSha256
        registration = $identity.registration
        installLocation = $identity.installLocation
        uninstallerPath = $identity.uninstallerPath
        uninstallerSha256 = $identity.uninstallerSha256
        startMenuTarget = $identity.startMenuTarget
        desktopLinkAbsent = $true
        installSnapshot = $installSnapshotBefore
        profileSnapshot = $profileBefore
        priorProcessesComplete = $true
        unrecoveredFixtureCount = 0
        removalIntent = 'remove the second registered installation exactly once through its existing GUI confirmation'
    })
    $removeBudget = New-UatCompletionBudget -TotalMilliseconds ([long]90000) -Origin 'A15 diagnostic final registered remove'
    $uninstaller = Start-UatUninstallerGui -Name 'A15-diagnostic-final-remove' -CompletionBudget $removeBudget
    $removed = Resolve-UatUninstallerConfirmation -Uninstaller $uninstaller -Choice 'remove'
    Assert-Uat ($removed.exitCode -eq 0) 'A15 final registered remove did not return actual exit code zero.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A15 final remove left the fixed TEST registration.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A15 final remove left a TEST link.'
    Assert-UatNoRunningTestApp -Root $expectedRoot
    Assert-Uat (Test-Path -LiteralPath $expectedRoot -PathType Container) 'A15 final remove did not preserve the owned marker fixture root.'
    $finalMarkerPath = Assert-UatRootOwnerMarker -Root $expectedRoot
    Assert-Uat ([string]::Equals($finalMarkerPath, $markerPath, [System.StringComparison]::OrdinalIgnoreCase)) 'A15 final remove changed the owned marker fixture path.'
    Assert-Uat ((Get-UatFileHash $finalMarkerPath) -ceq $markerSha256) 'A15 final remove changed the marker fixture bytes.'
    foreach ($owned in $candidate.receipt.payloadInventory) {
        $ownedPath = Join-Path $expectedRoot ([string]$owned.destination -replace '/', [string][char]92)
        Assert-Uat (-not (Test-Path -LiteralPath $ownedPath)) ('A15 terminal remove left installed payload: {0}.' -f $owned.destination)
    }
    $rootAfter = Get-UatTreeSnapshot -Path $expectedRoot
    $profileAfter = Get-UatTreeSnapshot -Path $script:profileRoot
    Test-UatSnapshotEqual -Expected $profileBefore -Actual $profileAfter -Message 'A15 final registered remove changed the isolated profile.'
    $afterPath = Join-Path $providedEvidencePath 'A15-final-remove-after.json'
    Write-UatJsonFile -Path $afterPath -Value ([ordered]@{
        candidateRoot = $expectedRoot; finalRootSnapshot = $rootAfter
        rootOwnerMarkerPath = $finalMarkerPath; rootOwnerMarkerSha256 = Get-UatFileHash $finalMarkerPath
        finalRegistration = Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey
        startMenuLinkAbsent = -not (Test-Path -LiteralPath $script:startMenuShortcut)
        desktopLinkAbsent = -not (Test-Path -LiteralPath $script:desktopShortcut)
        profileSnapshot = $profileAfter; finalRemoveExitCode = [int]$removed.exitCode
        finalRemoveScreenshot = $uninstaller.screenshot
        finalRemoveCommandEvidence = $removed.evidence.directory
        markerFixturePreserved = $true; noRecursiveDeleteOrA14A18 = $true
    })
    return @($beforePath, $uninstaller.screenshot, $uninstaller.logPath, $removed.evidence.directory, $afterPath, $finalMarkerPath)
}

function Test-UatA14RegisteredUninstallAndRetention {
    $candidate = $script:builds['fresh']
    $script:testRoot = $candidate.installRoot
    $markerPath = Assert-UatRootOwnerMarker -Root $script:testRoot
    $unknownPath = Join-Path $script:testRoot 'uat-user-unknown-sentinel.bin'
    $dataDirectory = Join-Path $script:testRoot 'data'
    if (-not (Test-Path -LiteralPath $dataDirectory -PathType Container)) { New-Item -ItemType Directory -Path $dataDirectory | Out-Null }
    $dataPath = Join-Path $dataDirectory 'uat-user-data-sentinel.bin'
    $unknownHash = Add-UatTestSentinel -Path $unknownPath -Text ('SDIR-UAT-UNKNOWN-RETAIN-' + $script:runId)
    $dataHash = Add-UatTestSentinel -Path $dataPath -Text ('SDIR-UAT-DATA-RETAIN-' + $script:runId)
    $settingsPath = [string]$script:profileFixture.settings
    $settingsHash = Get-UatFileHash $settingsPath
    $before = Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut
    $cancelUninstallBudget = New-UatCompletionBudget -TotalMilliseconds ([long]90000) -Origin 'A14 registered uninstall cancel route'
    $cancelRun = Start-UatUninstallerGui -Name 'A14-registered-uninstall-cancel' -CompletionBudget $cancelUninstallBudget
    $cancelled = Resolve-UatUninstallerConfirmation -Uninstaller $cancelRun -Choice 'cancel'
    Assert-Uat ($cancelled.exitCode -eq 0) 'Cancelling the actual registered uninstall did not exit cleanly.'
    Test-UatSnapshotEqual -Expected $before -Actual (Get-UatInstallSnapshot -Root $script:testRoot -RegistryKey $script:registryKey -StartMenuShortcut $script:startMenuShortcut -DesktopShortcut $script:desktopShortcut) -Message 'Cancelling registered uninstall changed the installation, registry, or shortcuts.'

    $removeUninstallBudget = New-UatCompletionBudget -TotalMilliseconds ([long]90000) -Origin 'A14 registered uninstall remove route'
    $removeRun = Start-UatUninstallerGui -Name 'A14-registered-uninstall-remove' -CompletionBudget $removeUninstallBudget
    $removed = Resolve-UatUninstallerConfirmation -Uninstaller $removeRun -Choice 'remove'
    Assert-Uat ($removed.exitCode -eq 0) 'The actual registered uninstaller failed to complete.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A14 actual uninstaller left the test registration.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A14 actual uninstaller left test shortcuts.'
    Assert-Uat (Test-Path -LiteralPath $script:testRoot -PathType Container) 'A14 retention fixture root was unexpectedly removed despite user data and unknown files.'
    Assert-Uat ((Get-UatFileHash $unknownPath) -eq $unknownHash -and (Get-UatFileHash $dataPath) -eq $dataHash) 'A14 uninstall changed or removed user data/unknown sentinels.'
    Assert-UatRootOwnerMarker -Root $script:testRoot | Out-Null
    Assert-Uat ((Get-UatFileHash $settingsPath) -eq $settingsHash) 'A14 uninstall changed saved application settings.'
    Assert-UatNoRunningTestApp -Root $script:testRoot
    $after = Get-UatTreeSnapshot -Path $script:testRoot
    $afterPath = Join-Path $providedEvidencePath 'A14-after-uninstall-retention.json'
    Write-UatJsonFile -Path $afterPath -Value $after
    Set-UatCase -Id 'A14' -Status 'verified' -Evidence @($cancelRun.screenshot, $cancelled.evidence.directory, $removeRun.screenshot, $removed.evidence.directory, $afterPath, $markerPath) -Details @('The actual registered native uninstall confirmation was cancelled with a byte-for-byte unchanged root/registry/shortcut snapshot, then accepted; the HKCU entry and shortcuts disappeared while the profile settings, user data, unknown file, and UAT ownership marker remained.')
    return @($cancelRun.screenshot, $cancelled.evidence.directory, $removeRun.screenshot, $removed.evidence.directory, $afterPath, $markerPath)
}

function Save-UatA16ProfileStage {
    param([Parameter(Mandatory = $true)][string]$Stage, [Parameter(Mandatory = $true)]$Before, [Parameter(Mandatory = $true)]$After)
    $beforeSnapshotPath = Join-Path $providedEvidencePath ('A16-profile-{0}-before.json' -f $Stage)
    Write-UatJsonFile -Path $beforeSnapshotPath -Value $Before
    $snapshotPath = Join-Path $providedEvidencePath ('A16-profile-{0}.json' -f $Stage)
    Write-UatJsonFile -Path $snapshotPath -Value $After
    $changes = New-Object System.Collections.Generic.List[object]
    foreach ($field in @('path', 'exists')) {
        if ((ConvertTo-Json -InputObject $Before[$field] -Depth 30 -Compress) -cne (ConvertTo-Json -InputObject $After[$field] -Depth 30 -Compress)) {
            [void]$changes.Add([ordered]@{ scope = 'tree'; field = $field; before = $Before[$field]; after = $After[$field] })
        }
    }
    $beforeOrder = @($Before.nodes | ForEach-Object { @($_.relativePath, $_.kind) })
    $afterOrder = @($After.nodes | ForEach-Object { @($_.relativePath, $_.kind) })
    if ((ConvertTo-Json -InputObject $beforeOrder -Depth 30 -Compress) -cne (ConvertTo-Json -InputObject $afterOrder -Depth 30 -Compress)) {
        [void]$changes.Add([ordered]@{ scope = 'node-order-or-membership'; before = $beforeOrder; after = $afterOrder })
    }
    foreach ($side in @('before', 'after')) {
        $sourceNodes = if ($side -ceq 'before') { $Before.nodes } else { $After.nodes }
        $otherNodes = if ($side -ceq 'before') { $After.nodes } else { $Before.nodes }
        foreach ($node in $sourceNodes) {
            $matches = @($otherNodes | Where-Object { $_.relativePath -ceq $node.relativePath -and $_.kind -ceq $node.kind })
            if ($matches.Count -ne 1) {
                [void]$changes.Add([ordered]@{ scope = 'node-membership'; side = $side; relativePath = $node.relativePath; kind = $node.kind; otherMatchCount = $matches.Count; node = $node })
                continue
            }
            if ($side -ceq 'after') { continue }
            $other = $matches[0]
            $keys = @(@($node.Keys) + @($other.Keys) | Sort-Object -Unique -CaseSensitive)
            foreach ($key in $keys) {
                $beforeHas = $node.Contains($key)
                $afterHas = $other.Contains($key)
                if ($beforeHas -ne $afterHas -or (ConvertTo-Json -InputObject $node[$key] -Depth 30 -Compress) -cne (ConvertTo-Json -InputObject $other[$key] -Depth 30 -Compress)) {
                    [void]$changes.Add([ordered]@{ scope = 'node-field'; relativePath = $node.relativePath; kind = $node.kind; field = $key; beforeHasField = $beforeHas; afterHasField = $afterHas; before = $node[$key]; after = $other[$key] })
                }
            }
        }
    }
    $beforeJson = ConvertTo-Json -InputObject $Before -Depth 30 -Compress
    $afterJson = ConvertTo-Json -InputObject $After -Depth 30 -Compress
    $differencePath = Join-Path $providedEvidencePath ('A16-profile-{0}-difference.json' -f $Stage)
    Write-UatJsonFile -Path $differencePath -Value ([ordered]@{ stage = $Stage; exactJsonEqual = ($beforeJson -ceq $afterJson); changes = @($changes.ToArray()); beforeSnapshot = [System.IO.Path]::GetFileName($beforeSnapshotPath); afterSnapshot = [System.IO.Path]::GetFileName($snapshotPath) })
}

function Test-UatA16BoundedSilentWorkflow {
    Assert-Uat ($script:evidenceDirectoryCreated -and $script:runOwnerWritten -and $script:explicitDiagnostic) 'A16 requires a newly created and owned diagnostic evidence directory.'
    $candidate = $script:builds['silent']
    Assert-Uat ([string]::Equals([string]$candidate.name, 'stable-silent-root', [System.StringComparison]::Ordinal)) 'A16 candidate name did not identify the single silent candidate.'
    Assert-Uat ([string]::Equals([string]$script:runOwner['silentCandidate']['root'], [string]$candidate.installRoot, [System.StringComparison]::OrdinalIgnoreCase)) 'A16 candidate install root did not match the run-owner marker.'
    Assert-Uat ([string]::Equals([string]$script:runOwner['silentCandidate']['path'], [string]$candidate.path, [System.StringComparison]::OrdinalIgnoreCase)) 'A16 candidate EXE did not match the run-owner marker.'
    $script:testRoot = $candidate.installRoot
    Assert-Uat (-not (Test-Path -LiteralPath $script:testRoot)) 'A16 silent install root was not fresh.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A16 silent workflow requires no existing test registration.'
    $profileBefore = Get-UatTreeSnapshot -Path $script:profileRoot
    Write-UatJsonFile -Path (Join-Path $providedEvidencePath 'A16-profile-before.json') -Value $profileBefore
    $image = Join-Path $script:testRoot 'SDIR.exe'
    $first = Invoke-UatSilentSetup -Candidate $candidate -Name 'A16-silent-fresh-install' -TimeoutSeconds 120
    $profileAfterInstall = Get-UatTreeSnapshot -Path $script:profileRoot
    Save-UatA16ProfileStage -Stage 'after-install' -Before $profileBefore -After $profileAfterInstall
    Assert-Uat ($first.exitCode -eq 0) 'A16 bounded silent fresh install did not return exit code 0.'
    $payload1 = Assert-UatInstalledPayload -Candidate $candidate -Root $script:testRoot -DesktopExpected:$false
    $afterInstallProcesses = @(Find-UatAppProcess -ImagePath $image -NotBeforeUtc $script:startedUtcAsDate -CommandPattern $null)
    Assert-Uat ($afterInstallProcesses.Count -eq 0) 'A16 silent fresh install launched the app.'
    $second = Invoke-UatSilentSetup -Candidate $candidate -Name 'A16-silent-reinstall' -TimeoutSeconds 120
    $profileAfterReinstall = Get-UatTreeSnapshot -Path $script:profileRoot
    Save-UatA16ProfileStage -Stage 'after-reinstall' -Before $profileBefore -After $profileAfterReinstall
    Assert-Uat ($second.exitCode -eq 0) 'A16 bounded silent reinstall did not return exit code 0.'
    $payload2 = Assert-UatInstalledPayload -Candidate $candidate -Root $script:testRoot -DesktopExpected:$false
    $afterReinstallProcesses = @(Find-UatAppProcess -ImagePath $image -NotBeforeUtc $script:startedUtcAsDate -CommandPattern $null)
    Assert-Uat ($afterReinstallProcesses.Count -eq 0) 'A16 silent reinstall launched the app.'
    $remove = Invoke-UatSilentUninstall -Name 'A16-silent-remove' -TimeoutSeconds 120
    $profileAfterRemove = Get-UatTreeSnapshot -Path $script:profileRoot
    Save-UatA16ProfileStage -Stage 'after-remove' -Before $profileBefore -After $profileAfterRemove
    Assert-Uat ($remove.exitCode -eq 0) 'A16 silent registered uninstaller did not return exit code 0.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A16 silent uninstall left test registry values.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A16 silent uninstall left a test shortcut.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:testRoot)) 'A16 silent uninstall left test-owned program files.'
    $afterRemoveProcesses = @(Find-UatAppProcess -ImagePath $image -NotBeforeUtc $script:startedUtcAsDate -CommandPattern $null)
    Assert-Uat ($afterRemoveProcesses.Count -eq 0) 'A16 silent uninstall launched the app.'
    Test-UatSnapshotEqual -Expected $profileBefore -Actual $profileAfterRemove -Message 'A16 silent workflow changed isolated user settings.'
    $profileEvidence = @('before', 'after-install', 'after-reinstall', 'after-remove' | ForEach-Object { Join-Path $providedEvidencePath ('A16-profile-{0}.json' -f $_) }) + @('after-install', 'after-reinstall', 'after-remove' | ForEach-Object { Join-Path $providedEvidencePath ('A16-profile-{0}-difference.json' -f $_) }) + @('after-install', 'after-reinstall', 'after-remove' | ForEach-Object { Join-Path $providedEvidencePath ('A16-profile-{0}-before.json' -f $_) })
    $evidence = @($profileEvidence) + @($first.evidence, $first.logPath, $second.evidence, $second.logPath, $remove.evidence, $remove.logPath, $script:diagnosticCandidateEvidencePath, $script:runOwnerPath)
    $summary = Join-Path $providedEvidencePath 'A16-silent-workflow.json'
    Write-UatJsonFile -Path $summary -Value @{ candidatePath = $candidate.path; candidateSha256 = $candidate.sha256; candidateReceipt = $candidate.receiptPath; candidateReceiptSha256 = $candidate.receiptSha256; freshInstallExitCode = $first.exitCode; reinstallExitCode = $second.exitCode; removeExitCode = $remove.exitCode; launchedAppAfterFreshInstall = $false; launchedAppAfterReinstall = $false; launchedAppAfterRemove = $false; setupArguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-'); noRestartRequested = $true; freshPayloadReceiptSha256 = $payload1.receiptSha256; reinstallPayloadReceiptSha256 = $payload2.receiptSha256 }
    Set-UatCase -Id 'A16' -Status 'verified' -Evidence ($evidence + @($summary)) -Details @('A16 diagnostic only: fresh install, same-root reinstall, and actual registered silent uninstall completed within explicit timeouts with captured exit codes and output; no app process was observed in the three existing post-step snapshots, /NORESTART was passed, and the isolated profile remained byte-identical. These snapshots and existing native records do not exclude every transient app launch. This does not qualify the full suite.')
    return $evidence + @($summary)
}

function Test-UatA17RuntimeAndMissingRuntime {
    $script:testRoot = $script:builds['stable'].installRoot
    $runtime = Get-UatRuntimeRegistryEvidence
    $runtimePath = Join-Path $providedEvidencePath 'A17-webview2-runtime-registry.json'
    Write-UatJsonFile -Path $runtimePath -Value $runtime
    $missingEvidence = Test-UatMissingRuntimeBranch
    $details = @('The WebView2 Evergreen product GUID and pv version were inspected across HKLM/HKCU 32/64 views, and a dedicated TEST_BUILD forced-missing-runtime candidate produced an actual visible prerequisite refusal with unchanged root/registration/shortcuts/profile.', 'Installed app launch/profile persistence evidence is referenced from A4; no Settings app route is claimed.')
    $status = if ($runtime.present -and @($script:a4Evidence).Count -gt 0) { 'verified' } else { 'pending' }
    if (-not $runtime.present) { $details += 'The host did not have a valid actual runtime, so app launch with the host runtime remains pending; no runtime was installed or changed.' }
    Set-UatCase -Id 'A17' -Status $status -Evidence (@($runtimePath) + @($missingEvidence) + @($script:a4Evidence)) -Details $details
    return @($runtimePath) + @($missingEvidence) + @($script:a4Evidence)
}

function Test-UatA18ProductionIsolationAndCleanup {
    $productionRegistryKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $script:productionAppId + '_is1'
    $beforeProduction = $script:productionSnapshotBefore
    Assert-Uat ($null -ne $beforeProduction) 'A18 production baseline snapshot was not recorded at run start.'
    $beforeUnrelated = Get-UatTreeSnapshot -Path $script:unrelatedSentinelRoot
    Assert-Uat ((Get-UatFileHash $script:unrelatedSentinelPath) -eq $script:unrelatedSentinelHash) 'The unrelated AppData sentinel changed before cleanup.'
    $script:stableAppHashAfter = Get-UatFileHash (Join-Path $repoRoot 'dist\SDIR.exe')
    $script:stablePortableHashAfter = Get-UatFileHash $script:portablePath
    Assert-Uat ($script:stableAppHashAfter -eq $script:stableAppExpected -and $script:stablePortableHashAfter -eq $script:stablePortableExpected) 'Stable distribution app or portable ZIP changed during UAT.'
    Test-UatSnapshotEqual -Expected $beforeProduction -Actual (Get-UatProductionSnapshot -ProductionRegistryKey $productionRegistryKey) -Message 'Production registration, shortcuts, SDIR installation files, or actual profile changed during test-only UAT.'
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A18 cleanup began while a test registration remained active.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A18 cleanup began while a test shortcut remained registered.'
    foreach ($entry in $script:activeProcesses) {
        if ($null -ne $entry.PSObject.Properties['wrapperProcess'] -and $null -ne $entry.wrapperProcess) {
            Assert-Uat ([bool]$entry.wrapperFinished -and $null -ne $entry.wrapperExitCode) ('A18 cleanup found an incomplete handoff wrapper command: {0} PID {1}' -f $entry.name, $entry.wrapperIdentity.processId)
        }
        if ($entry.finished) { continue }
        if ($entry.kind -eq 'owned-child' -and $null -ne $entry.child) {
            Assert-Uat ($entry.child.HasExited) ('A18 cleanup refused while harness-owned process remains active: {0} PID {1}' -f $entry.name, $entry.processId)
        }
        elseif ($entry.kind -eq 'external-app' -and $null -ne $entry.process) {
            Assert-Uat ($entry.process.HasExited) ('A18 cleanup refused while harness-owned GUI process remains active: {0} PID {1}' -f $entry.name, $entry.processId)
        }
        else {
            throw 'A18 cleanup refused an unknown kind or missing process handle.'
        }
    }
    $primaryRoot = $script:builds['stable'].installRoot
    if (Test-Path -LiteralPath $primaryRoot -PathType Container) {
        $script:testRoot = $primaryRoot
        $ownerMarker = Assert-UatRootOwnerMarker -Root $primaryRoot
        $a18UninstallBudget = New-UatCompletionBudget -TotalMilliseconds ([long]90000) -Origin 'A18 final owned uninstall cleanup route'
        $uninstaller = Start-UatUninstallerGui -Name 'A18-final-owned-test-install-remove' -CompletionBudget $a18UninstallBudget
        $removed = Resolve-UatUninstallerConfirmation -Uninstaller $uninstaller -Choice 'remove'
        Assert-Uat ($removed.exitCode -eq 0) 'Final owned test installation could not be removed through its registered uninstaller.'
        Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'Final test uninstaller left registry values.'
        [void]$ownerMarker
    }
    Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A18 cleanup left a test registration.'
    Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A18 cleanup left a test shortcut.'
    foreach ($name in @('fresh', 'portable', 'separate', 'silent', 'stable')) {
        $root = [string]$script:builds[$name].installRoot
        if (Test-Path -LiteralPath $root -PathType Container) { Remove-UatOwnedFixtureRoot -Root $root | Out-Null }
    }
    $collisionRoot = [string]$script:builds['silent'].installRoot
    if (Test-Path -LiteralPath $collisionRoot -PathType Container) { throw 'A18 collision root cleanup order was invalid; an unexpected root remains.' }
    $profileMarkerPath = Join-Path $script:profileRoot $script:profileMarkerName
    Assert-Uat (Test-Path -LiteralPath $profileMarkerPath -PathType Leaf) 'A18 isolated profile cleanup marker is missing.'
    $profileMarkerText = [System.IO.File]::ReadAllText($profileMarkerPath, [System.Text.Encoding]::ASCII)
    Assert-Uat ($profileMarkerText -eq $script:profileMarkerValue) 'A18 isolated profile marker content changed.'
    Assert-NoReparseComponents -Path $script:profileRoot
    $profileBeforeCleanup = Get-UatTreeSnapshot -Path $script:profileRoot
    $profileEvidence = Join-Path $providedEvidencePath 'A18-isolated-profile-before-cleanup.json'
    Write-UatJsonFile -Path $profileEvidence -Value $profileBeforeCleanup
    Remove-Item -LiteralPath $script:profileRoot -Recurse -Force
    Assert-Uat (-not (Test-Path -LiteralPath $script:profileRoot)) 'A18 isolated profile fixture did not clean up.'
    $unrelatedOwner = Join-Path $script:unrelatedSentinelRoot '.sdir-installer-test-unrelated'
    Assert-Uat (Test-Path -LiteralPath $unrelatedOwner -PathType Leaf) 'A18 unrelated AppData sentinel ownership marker is missing.'
    Assert-Uat ([System.IO.File]::ReadAllText($unrelatedOwner, [System.Text.Encoding]::ASCII) -eq ('SDIR-INSTALLER-UAT-UNRELATED-V1|' + $script:runId)) 'A18 unrelated AppData marker does not belong to this run.'
    Test-UatSnapshotEqual -Expected $beforeUnrelated -Actual (Get-UatTreeSnapshot -Path $script:unrelatedSentinelRoot) -Message 'A18 unrelated AppData sentinel changed before its owned cleanup.'
    Assert-NoReparseComponents -Path $script:unrelatedSentinelRoot
    Remove-Item -LiteralPath $script:unrelatedSentinelRoot -Recurse -Force
    Assert-Uat (-not (Test-Path -LiteralPath $script:unrelatedSentinelRoot)) 'A18 unrelated AppData fixture did not clean up.'
    if (-not $script:startMenuDirectoryPreexisting) {
        $startDirectory = Split-Path -Parent $script:startMenuShortcut
        if ((Test-Path -LiteralPath $startDirectory -PathType Container) -and @(Get-ChildItem -LiteralPath $startDirectory -Force).Count -eq 0) {
            [System.IO.Directory]::Delete($startDirectory, $false)
        }
    }
    $afterProduction = Get-UatProductionSnapshot -ProductionRegistryKey $productionRegistryKey
    Test-UatSnapshotEqual -Expected $beforeProduction -Actual $afterProduction -Message 'Production state changed during owned fixture cleanup.'
    $afterHashes = [ordered]@{ app = Get-UatFileHash (Join-Path $repoRoot 'dist\SDIR.exe'); portable = Get-UatFileHash $script:portablePath }
    Assert-Uat ($afterHashes.app -eq $script:stableAppExpected -and $afterHashes.portable -eq $script:stablePortableExpected) 'Stable distribution hashes changed during cleanup.'
    $cleanupEvidence = Join-Path $providedEvidencePath 'A18-isolation-cleanup.json'
    Write-UatJsonFile -Path $cleanupEvidence -Value ([ordered]@{
        productionStateUnchanged = $true
        productionAppId = $script:productionAppId
        productionStartMenuDesktopProfileAndRootCompared = $true
        stableAppSha256Before = $script:stableAppHashBefore
        stableAppSha256After = $afterHashes.app
        stablePortableSha256Before = $script:stablePortableHashBefore
        stablePortableSha256After = $afterHashes.portable
        unrelatedSentinelBeforeCleanup = $beforeUnrelated
        isolatedProfileBeforeCleanup = $profileEvidence
        cleanupCompletedOnlyAfterAllPriorCasesPassed = $true
        activeHarnessChildren = 0
    })
    $script:cleanupVerified = $true
    Set-UatCase -Id 'A18' -Status 'verified' -Evidence @($profileEvidence, $cleanupEvidence, $script:runOwnerPath) -Details @('Production install/profile/registry/shortcuts and stable app/portable hashes matched their run-start baselines; each harness launch used the isolated profile; all active test children exited normally; only roots and profile/AppData fixtures with this run ID marker were removed after every mandatory prior case passed.')
    return @($profileEvidence, $cleanupEvidence, $script:runOwnerPath)
}

function Assert-UatDiagnosticStreamChunks {
    param($Capture, $Rows)
    Assert-Uat ($Rows -is [array] -and $null -ne $Capture.Chunks -and $null -ne $Capture.StandardOutput -and $null -ne $Capture.StandardError) 'The completed capture or JSON chunk array was missing.'
    $expected = @($Capture.Chunks | Sort-Object Sequence)
    Assert-Uat ($Rows.Count -eq $expected.Count) 'The saved chunk array did not match the collector chunk count.'
    $offsets = @{ stdout = [long]0; stderr = [long]0 }
    for ($sequence = 0; $sequence -lt $Rows.Count; $sequence++) {
        $row = $Rows[$sequence]
        $chunk = $expected[$sequence]
        Assert-Uat ($null -ne $row -and $null -ne $chunk) 'A saved chunk or collector chunk was null.'
        Assert-Uat ($row.sequence -eq $sequence -and $chunk.Sequence -eq $sequence) 'The native chunk sequence was not consecutive from zero.'
        Assert-Uat (($row.stream -ceq 'stdout' -or $row.stream -ceq 'stderr') -and $row.stream -ceq $chunk.StreamName) 'A native chunk stream name was invalid or altered.'
        Assert-Uat ($row.offset -eq $offsets[$row.stream] -and $row.offset -eq $chunk.Offset -and $row.length -gt 0 -and $row.length -eq $chunk.Length) 'The native chunk byte range differed from the collector or preceding chunk.'
        $offsets[$row.stream] += [long]$row.length
    }
    Assert-Uat ($offsets.stdout -eq $Capture.StandardOutput.Length -and $offsets.stderr -eq $Capture.StandardError.Length) 'The native chunk ranges did not account for both full output streams.'
}

function Assert-UatTrackedProcessResultDto {
    param($Entry, [string]$CaseId)
    Assert-Uat ([string]$Entry.sessionId -ceq [string]$script:runId -and -not [string]::IsNullOrWhiteSpace([string]$Entry.commandId)) ('{0} found a command without the current session and command identity.' -f $CaseId)
    Assert-Uat ([bool]$Entry.finished) ('{0} found an incomplete process: {1}.' -f $CaseId, $Entry.name)
    if ($Entry.kind -ceq 'owned-child') {
        Assert-Uat ($null -ne $Entry.completedResult) ('{0} found no completed owned-child result DTO: {1}.' -f $CaseId, $Entry.name)
        $result = $Entry.completedResult
        $capture = $result.capture
        Assert-Uat ($null -ne $capture -and $capture.ProcessExited -and $capture.HasExitCode -and $capture.OutputComplete -and $capture.IsFinal) ('{0} found incomplete final launcher output or exit evidence for {1}.' -f $CaseId, $Entry.name)
        Assert-Uat ($capture.ProcessId -eq $Entry.processId -and $null -ne $result.exitCode -and $null -ne $result.launcherExitCode -and $result.exitCode -eq $capture.ExitCode -and $result.launcherExitCode -eq $capture.ExitCode) ('{0} found an unknown or mismatched actual launcher exit for {1}.' -f $CaseId, $Entry.name)
        if ($Entry.nativeTracking) {
            Assert-Uat ($null -ne $result.nativeEngineExitCodes -and @($result.nativeEngineExitCodes | Where-Object { $null -eq $_.exitCode }).Count -eq 0) ('{0} found a pinned native engine with unknown actual exit for {1}.' -f $CaseId, $Entry.name)
        }
        if ($null -ne $Entry.launchIntent) {
            Assert-Uat ($Entry.launchIntent.state -ceq 'transferred') ('{0} found an incomplete launcher handoff for {1}.' -f $CaseId, $Entry.name)
        }
        return
    }
    Assert-Uat ($Entry.kind -ceq 'external-app') ('{0} encountered an unsupported process record kind: {1}.' -f $CaseId, $Entry.kind)
    Assert-Uat ($null -ne $Entry.appExitCode -and $Entry.appExitCode -eq 0 -and $Entry.outputCapture -match 'no redirected process stream') ('{0} found incomplete or nonzero GUI app evidence for {1}.' -f $CaseId, $Entry.name)
    if ($null -ne $Entry.wrapperIdentity) {
        Assert-Uat ($Entry.wrapperFinished -and $null -ne $Entry.wrapperExitCode -and $Entry.wrapperExitCode -eq 0 -and $null -ne $Entry.launchHandoff) ('{0} found incomplete or nonzero GUI handoff wrapper evidence for {1}.' -f $CaseId, $Entry.name)
        foreach ($descendant in @($Entry.launchHandoff.appDescendants)) {
            Assert-Uat ($descendant.nativeRecord.finished -and $null -ne $descendant.exitCode) ('{0} found an incomplete handoff descendant for {1} PID {2}.' -f $CaseId, $Entry.name, $descendant.processId)
        }
    }
}

function Read-UatDiagnosticCandidateOwner {
    param([switch]$RequireComplete)
    Assert-Uat ($script:evidenceDirectoryCreated -and $script:runOwnerWritten) 'No diagnostic run owner has been established.'
    Assert-NoReparseComponents -Path $script:runOwnerPath
    $owner = Get-Content -LiteralPath $script:runOwnerPath -Raw | ConvertFrom-Json
    Assert-UatDiagnosticCandidateOwner -Owner $owner -RequireComplete:$RequireComplete
}

function Assert-UatDiagnosticCandidateOwner {
    param([Parameter(Mandatory = $true)]$Owner, [switch]$RequireComplete)
    Assert-Uat ($Owner.marker -ceq 'SDIR-INSTALLER-UAT-RUN-V1' -and $Owner.runId -ceq $script:runId -and $Owner.explicitDiagnostic -eq $true) 'The diagnostic run-owner identity changed.'
    Assert-Uat (@($Owner.selectedCases).Count -eq 1 -and [string]$Owner.selectedCases[0] -ceq [string]$script:selectedCases[0]) 'The run-owner selected case did not match the requested standalone diagnostic.'
    Assert-Uat ($null -eq $Owner.silentCandidate) 'A6/A15 run-owner unexpectedly contains an A16 silent candidate.'
    foreach ($field in @('script', 'evidenceDirectory', 'testRoot', 'profileRoot')) {
        Assert-Uat ([string]::Equals([string]$Owner.$field, [string]$script:runOwner[$field], [System.StringComparison]::OrdinalIgnoreCase)) ('The diagnostic run-owner path changed: {0}.' -f $field)
    }
    Assert-Uat ([string]$Owner.rootName -ceq [string]$script:initialTestRootName) 'The immutable initial run-owner root name changed.'
    $ownerProperties = @($Owner.candidateOwners.PSObject.Properties)
    Assert-Uat ($ownerProperties.Count -eq $script:diagnosticCandidateKeys.Count) 'The run-owner candidate set changed.'
    foreach ($candidateKey in $script:diagnosticCandidateKeys) {
        $candidateProperty = $Owner.candidateOwners.PSObject.Properties[[string]$candidateKey]
        Assert-Uat ($null -ne $candidateProperty -and $candidateProperty.Name -ceq $candidateKey) ('The run-owner omitted candidate {0}.' -f $candidateKey)
        $candidateOwner = $candidateProperty.Value
        $expected = $script:runOwner['candidateOwners'][$candidateKey]
        Assert-UatEqual $expected $candidateOwner ('The run-owner record for candidate {0} differed from its in-memory pin.' -f $candidateKey)
        if ($null -eq $expected.setupSha256) {
            Assert-Uat (-not $RequireComplete -and -not $script:builds.Contains($candidateKey)) ('Candidate {0} was never fully pinned.' -f $candidateKey)
            continue
        }
        Assert-Uat ($script:builds.Contains($candidateKey)) ('The recorded candidate {0} was not in the selected builds.' -f $candidateKey)
        $build = $script:builds[$candidateKey]
        $expectedReceipt = [string]$expected.path + '.build.json'
        $expectedProvenance = Join-Path $providedEvidencePath ($script:selectedCases[0] + '-' + $candidateKey + '-candidate-provenance.json')
        Assert-Uat ([string]$build.name -ceq $expected.name -and [string]$build.rootName -ceq $expected.rootName) ('Candidate {0} changed name or root name.' -f $candidateKey)
        Assert-Uat ([string]::Equals($build.installRoot, $expected.root, [System.StringComparison]::OrdinalIgnoreCase)) ('Candidate {0} changed its immutable install root.' -f $candidateKey)
        Assert-Uat ([string]::Equals($build.path, $expected.path, [System.StringComparison]::OrdinalIgnoreCase)) ('Candidate {0} changed its exact EXE path.' -f $candidateKey)
        Assert-Uat ([string]::Equals([string]$candidateOwner.receiptPath, $expectedReceipt, [System.StringComparison]::OrdinalIgnoreCase) -and [string]::Equals($build.receiptPath, $expectedReceipt, [System.StringComparison]::OrdinalIgnoreCase)) ('Candidate {0} changed its exact build receipt path.' -f $candidateKey)
        Assert-Uat ([string]::Equals([string]$candidateOwner.provenancePath, $expectedProvenance, [System.StringComparison]::OrdinalIgnoreCase)) ('Candidate {0} changed its exact provenance path.' -f $candidateKey)
        Assert-Uat ($null -ne $candidateOwner.compilerExitCode -and $candidateOwner.compilerExitCode -eq 0 -and $build.compilerExitCode -eq 0) ('Candidate {0} has no actual zero compiler exit.' -f $candidateKey)
        Assert-Uat ([string]$build.sha256 -ceq [string]$expected.setupSha256 -and [string]$build.receiptSha256 -ceq [string]$expected.receiptSha256) ('Candidate {0} changed its build hash pins.' -f $candidateKey)
        foreach ($pin in @(
            @{ path = $expected.path; sha256 = $expected.setupSha256 },
            @{ path = $expectedReceipt; sha256 = $expected.receiptSha256 },
            @{ path = $expectedProvenance; sha256 = $expected.provenanceSha256 }
        )) {
            Assert-NoReparseComponents -Path $pin.path
            Assert-Uat ((Get-UatFileHash $pin.path) -ceq [string]$pin.sha256) ('Candidate {0} artifact SHA-256 changed: {1}.' -f $candidateKey, $pin.path)
        }
        Assert-Uat ((Get-Item -LiteralPath $expected.path).Length -eq [long]$expected.setupBytes) ('Candidate {0} EXE byte count changed.' -f $candidateKey)
    }
    Assert-Uat ([string]$Owner.candidateProvenancePath -ceq [string]$script:runOwner.candidateProvenancePath -and [string]$Owner.candidateProvenanceSha256 -ceq [string]$script:runOwner.candidateProvenanceSha256) 'The run-owner aggregate provenance pin changed.'
    if ($null -eq $script:runOwner.candidateProvenancePath) {
        Assert-Uat (-not $RequireComplete) 'The diagnostic aggregate provenance record was never pinned.'
        return
    }
    $aggregate = Join-Path $providedEvidencePath ($script:selectedCases[0] + '-candidate-provenance.json')
    Assert-Uat ([string]::Equals([string]$Owner.candidateProvenancePath, $aggregate, [System.StringComparison]::OrdinalIgnoreCase)) 'The diagnostic aggregate provenance path changed.'
    Assert-NoReparseComponents -Path $aggregate
    Assert-Uat ((Get-UatFileHash $aggregate) -ceq [string]$Owner.candidateProvenanceSha256) 'The diagnostic aggregate provenance file changed.'
}

function Assert-UatTrackedProcessResults {
    param([Parameter(Mandatory = $true)][string]$CaseId)
    foreach ($entry in $script:activeProcesses) {
        Assert-UatTrackedProcessResultDto -Entry $entry -CaseId $CaseId
        if ($entry.kind -cne 'owned-child') { continue }
        $capture = $entry.completedResult.capture
        $files = $entry.completedResult.evidence
        foreach ($path in @($files.stdoutRaw, $files.stderrRaw, $files.chunks)) {
            Assert-Uat (-not [string]::IsNullOrWhiteSpace([string]$path) -and (Test-Path -LiteralPath $path -PathType Leaf)) ('{0} is missing captured stream evidence for {1}.' -f $CaseId, $entry.name)
            Assert-NoReparseComponents -Path $path
        }
        $stdout = [System.IO.File]::ReadAllBytes([string]$files.stdoutRaw)
        $stderr = [System.IO.File]::ReadAllBytes([string]$files.stderrRaw)
        Assert-Uat ([Convert]::ToBase64String($stdout) -ceq [Convert]::ToBase64String($capture.StandardOutput)) ('{0} stdout raw bytes did not match the completed collector capture for {1}.' -f $CaseId, $entry.name)
        Assert-Uat ([Convert]::ToBase64String($stderr) -ceq [Convert]::ToBase64String($capture.StandardError)) ('{0} stderr raw bytes did not match the completed collector capture for {1}.' -f $CaseId, $entry.name)
        $chunkJson = [System.IO.File]::ReadAllText([string]$files.chunks, [System.Text.UTF8Encoding]::new($false, $true))
        $savedChunks = ConvertFrom-Json -InputObject ('{"rows":' + $chunkJson + '}')
        Assert-UatDiagnosticStreamChunks -Capture $capture -Rows $savedChunks.rows
    }
}

try {
    $currentIdentity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    try {
        Assert-Uat ($null -ne $currentIdentity.User -and -not [string]::IsNullOrWhiteSpace([string]$currentIdentity.User.Value)) 'The current Windows user SID was unavailable for the TEST lifecycle mutex.'
        $currentSid = [string]$currentIdentity.User.Value
    }
    finally {
        $currentIdentity.Dispose()
    }
    $script:mutexName = 'Global\SDIR-UAT-' + $currentSid + '-' + $script:testAppId
    $script:uatMutex = [System.Threading.Mutex]::new($false, $script:mutexName)
    try {
        $script:mutexAcquired = [bool]$script:uatMutex.WaitOne(0)
        if ($script:mutexAcquired) { $script:mutexWasAcquired = $true }
    }
    catch [System.Threading.AbandonedMutexException] {
        $script:mutexAcquired = $true
        $script:mutexWasAcquired = $true
        $script:mutexAbandoned = $true
        throw 'The TEST lifecycle mutex was abandoned; the diagnostic refused to continue.'
    }
    if (-not $script:mutexAcquired) {
        throw 'Another TEST lifecycle owns the current-user TEST AppID mutex; no run was started.'
    }
    Assert-UatNoExistingTestLifecycle
    Initialize-UatPaths
    if ($script:selectedCases[0] -ceq 'A16') {
        [void](Test-UatStaticProvenance -A16Only)
        Assert-Uat ($script:builds.Contains('silent') -and $null -ne $script:diagnosticCandidateRecord) 'A16 did not produce its single silent candidate provenance record.'
        [void](Test-UatA16BoundedSilentWorkflow)
        Assert-Uat ($script:caseResults['A16'].status -eq 'verified') 'A16 workflow returned without verified case evidence.'
        Assert-Uat (-not $script:caseResults['A1'].status.Equals('verified')) 'A16 diagnostic cannot verify A1.'
        Assert-Uat ($null -eq $script:sourceBuildIdentityEquivalence) 'A16 diagnostic must not claim stable/baseline build equivalence.'
        $silentCandidate = $script:builds['silent']
        $silentRoot = [System.IO.Path]::GetFullPath([string]$silentCandidate.installRoot)
        Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A16 final exit confirmation found a TEST registration.'
        Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A16 final exit confirmation found a TEST shortcut.'
        Assert-Uat (-not (Test-Path -LiteralPath $silentRoot)) 'A16 final exit confirmation found the silent candidate install root.'
        $a16CommandNames = @('A16-silent-fresh-install', 'A16-silent-reinstall', 'A16-silent-remove')
        foreach ($commandName in $a16CommandNames) {
            $commandEntries = @($script:activeProcesses | Where-Object { [string]::Equals([string]$_.name, $commandName, [System.StringComparison]::Ordinal) })
            Assert-Uat ($commandEntries.Count -eq 1) ('A16 final exit confirmation expected exactly one native command record for {0}.' -f $commandName)
        }
        foreach ($entry in $script:activeProcesses) {
            Assert-Uat ([bool]$entry.finished -and $null -ne $entry.completedResult) ('A16 final exit confirmation found an incomplete command: {0}.' -f $entry.name)
            $capture = $entry.completedResult.capture
            Assert-Uat ($null -ne $capture -and [bool]$capture.ProcessExited -and [bool]$capture.HasExitCode -and [bool]$capture.OutputComplete -and [bool]$capture.IsFinal) ('A16 final exit confirmation found incomplete launcher output or exit evidence for {0}.' -f $entry.name)
            Assert-Uat ($null -ne $entry.completedResult.exitCode) ('A16 final exit confirmation found an unknown launcher exit code for {0}.' -f $entry.name)
            foreach ($nativeExit in @($entry.completedResult.nativeEngineExitCodes)) {
                Assert-Uat ($null -ne $nativeExit.exitCode) ('A16 final exit confirmation found an unknown native engine exit code for {0} PID {1}.' -f $entry.name, $nativeExit.processId)
            }
        }
        $script:stableAppHashAfter = Get-UatFileHash (Join-Path $repoRoot 'dist\SDIR.exe')
        $script:stablePortableHashAfter = Get-UatFileHash $script:portablePath
        Assert-Uat ($script:stableAppHashBefore -ceq $script:stableAppExpected -and $script:stableAppHashAfter -ceq $script:stableAppExpected) 'A16 changed or misidentified the stable app artifact.'
        Assert-Uat ($script:stablePortableHashBefore -ceq $script:stablePortableExpected -and $script:stablePortableHashAfter -ceq $script:stablePortableExpected) 'A16 changed or misidentified the stable portable archive.'
        Assert-Uat (-not $script:cleanupVerified -and -not $script:fullsuiteComplete) 'A16 diagnostic cannot claim A18 cleanup or full-suite completion.'
        foreach ($caseId in $script:caseDefinitions.Keys) {
            if ($caseId -eq 'A16') { continue }
            $expectedStatus = if ($caseId -eq 'A12') { 'excluded' } else { 'pending' }
            Assert-Uat ($script:caseResults[$caseId].status -eq $expectedStatus) ('A16 diagnostic changed an unselected case status: {0}.' -f $caseId)
        }
        $script:completed = $true
        $script:diagnosticCompleted = $true
        $script:fullsuiteComplete = $false
        $script:exitCode = 0
    }
    else {
        [void](Test-UatStaticProvenance)
        [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
        if ($script:selectedCases[0] -ceq 'A6') {
            $script:a6Evidence = @(Test-UatA6PortableAndSeparateInstall)
        }
        else {
            $script:a15Evidence = @(Invoke-UatA15FreshRestoreOnlyEntry)
            $script:a15Evidence = @($script:a15Evidence) + @(Invoke-UatA15DiagnosticFinalRemove)
        }
        [void](Assert-UatTrackedProcessResults -CaseId ([string]$script:selectedCases[0]))
        [void](Read-UatDiagnosticCandidateOwner -RequireComplete)
        $script:stableAppHashAfter = Get-UatFileHash (Join-Path $repoRoot 'dist\SDIR.exe')
        $script:stablePortableHashAfter = Get-UatFileHash $script:portablePath
        Assert-Uat ($script:stableAppHashBefore -ceq $script:stableAppExpected -and $script:stableAppHashAfter -ceq $script:stableAppExpected) 'A6/A15 changed or misidentified the stable app artifact.'
        Assert-Uat ($script:stablePortableHashBefore -ceq $script:stablePortableExpected -and $script:stablePortableHashAfter -ceq $script:stablePortableExpected) 'A6/A15 changed or misidentified the stable portable archive.'
        $script:productionSnapshotAfter = Get-UatProductionSnapshot -ProductionRegistryKey ('Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $script:productionAppId + '_is1')
        Test-UatSnapshotEqual -Expected $script:productionSnapshotBefore -Actual $script:productionSnapshotAfter -Message 'A6/A15 changed production registration, shortcuts, install root, or profile.'
        Assert-Uat ((Get-UatFileHash $script:unrelatedSentinelPath) -ceq $script:unrelatedSentinelHash) 'A6/A15 changed the unrelated AppData sentinel.'
        Assert-Uat (-not (Get-UatRegistrySnapshot -Hive CurrentUser -SubKey $script:registryKey).exists) 'A6/A15 final exit found a TEST registration.'
        Assert-Uat (-not (Test-Path -LiteralPath $script:startMenuShortcut) -and -not (Test-Path -LiteralPath $script:desktopShortcut)) 'A6/A15 final exit found a TEST shortcut.'
        Assert-Uat (-not $script:cleanupVerified -and -not $script:fullsuiteComplete) 'A6/A15 diagnostics cannot claim cleanup or full-suite completion.'
        foreach ($caseId in $script:caseDefinitions.Keys) {
            if ($caseId -eq $script:selectedCases[0]) { continue }
            $expectedStatus = if ($caseId -eq 'A12') { 'excluded' } else { 'pending' }
            Assert-Uat (-not [bool]$script:caseResults[$caseId].selected -and $script:caseResults[$caseId].status -eq $expectedStatus) ('A6/A15 diagnostic changed an unselected case state: {0}.' -f $caseId)
        }
        if ($script:selectedCases[0] -ceq 'A6') {
            Set-UatCase -Id 'A6' -Status 'verified' -Evidence $script:a6Evidence -Details @('The A6 portable.flag preflight refusal was functionally verified by the actual launcher exit code 7, its command-bound closed setup log, and matching portable, profile, TEST registry, shortcut, separate root, ZIP, and production state. Refusal capture was ROOT_ONLY; native child observation was NOT_COLLECTED and child exits are UNKNOWN.', 'The separate install was payload/receipt verified, settings.json alone transferred with no-overwrite, and the registered GUI removal returned zero while preserving the isolated profile.') -DeferResultWrite
        }
        else {
            Set-UatCase -Id 'A15' -Status 'pending' -Evidence $script:a15Evidence -Details @('RestoreOnly measured the fresh install, normal UI save/close, first registered removal, reinstall, restored UI and persisted profile, and one terminal removal of the second registration. No lock refusal or ordinary running-app refusal was executed; A15 remains pending for those acceptance conditions.') -DeferResultWrite
        }
        $script:completed = $true
        $script:diagnosticCompleted = $true
        $script:fullsuiteComplete = $false
        $script:cleanupVerified = $false
        $script:exitCode = 0
    }
}
catch {
    $script:originalErrorRecord = $_
    $script:failureText = $_.ToString()
    $script:exitCode = 1
    $script:completed = $false
    $script:diagnosticCompleted = $false
    $script:fullsuiteComplete = $false
}
finally {
    $finalizationErrors = New-Object System.Collections.Generic.List[string]
    $ownerIsValid = $false
    if ($script:evidenceDirectoryCreated -and $script:runOwnerWritten) {
        try {
            Assert-Uat (Test-Path -LiteralPath $providedEvidencePath -PathType Container) 'The owned diagnostic evidence directory disappeared before finalization.'
            Assert-Uat (Test-Path -LiteralPath $script:runOwnerPath -PathType Leaf) 'The owned run-owner marker disappeared before finalization.'
            Assert-NoReparseComponents -Path $script:runOwnerPath
            $diskOwner = Get-Content -LiteralPath $script:runOwnerPath -Raw | ConvertFrom-Json
            Assert-Uat ([string]$diskOwner.marker -ceq 'SDIR-INSTALLER-UAT-RUN-V1') 'The run-owner marker signature changed before finalization.'
            Assert-Uat ([string]$diskOwner.runId -ceq $script:runId) 'The run-owner marker runId changed before finalization.'
            Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$diskOwner.evidenceDirectory), $providedEvidencePath, [System.StringComparison]::OrdinalIgnoreCase)) 'The run-owner marker evidence path changed before finalization.'
            Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$diskOwner.script), [System.IO.Path]::GetFullPath($PSCommandPath), [System.StringComparison]::OrdinalIgnoreCase)) 'The run-owner marker script path changed before finalization.'
            Assert-Uat ([string]$diskOwner.explicitDiagnostic -eq 'True' -and @($diskOwner.selectedCases).Count -eq 1 -and [string]$diskOwner.selectedCases[0] -ceq [string]$script:selectedCases[0]) 'The run-owner marker no longer identifies the explicit selected diagnostic.'
            Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$diskOwner.testRoot), [System.IO.Path]::GetFullPath([string]$script:initialTestRoot), [System.StringComparison]::OrdinalIgnoreCase)) 'The run-owner initial root changed before finalization.'
            Assert-Uat ([string]$diskOwner.rootName -ceq [string]$script:initialTestRootName) 'The run-owner initial root name changed before finalization.'
            Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$diskOwner.profileRoot), $script:profileRoot, [System.StringComparison]::OrdinalIgnoreCase)) 'The run-owner profile path changed before finalization.'
            $ownerIsValid = $true
        }
        catch {
            [void]$finalizationErrors.Add(('Run-owner validation failed: {0}' -f $_.Exception.ToString()))
        }
    }
    if ($ownerIsValid) {
        try {
            if ($script:selectedCases[0] -ceq 'A16') {
                $expectedSilentRoot = [System.IO.Path]::GetFullPath((Join-Path (Join-Path $env:LOCALAPPDATA 'Programs') ($script:initialTestRootName + '-SILENT')))
                $expectedSilentCandidate = [System.IO.Path]::GetFullPath((Join-Path $providedEvidencePath 'stable-silent-root.exe'))
                Assert-Uat ($null -ne $diskOwner.silentCandidate) 'The A16 run-owner silent candidate was missing.'
                Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$diskOwner.silentCandidate.root), $expectedSilentRoot, [System.StringComparison]::OrdinalIgnoreCase)) 'The run-owner marker silent candidate root changed before finalization.'
                Assert-Uat ([string]::Equals([System.IO.Path]::GetFullPath([string]$diskOwner.silentCandidate.path), $expectedSilentCandidate, [System.StringComparison]::OrdinalIgnoreCase)) 'The run-owner marker candidate EXE path changed before finalization.'
                if (-not [string]::IsNullOrWhiteSpace([string]$diskOwner.silentCandidate.setupSha256)) {
                    Assert-Uat (Test-Path -LiteralPath $expectedSilentCandidate -PathType Leaf) 'The run-owner marker candidate EXE disappeared before finalization.'
                    Assert-Uat ((Get-UatFileHash $expectedSilentCandidate) -ceq [string]$diskOwner.silentCandidate.setupSha256) 'The owned candidate EXE hash changed before finalization.'
                    Assert-Uat (Test-Path -LiteralPath ([string]$diskOwner.silentCandidate.receiptPath) -PathType Leaf) 'The owned candidate receipt disappeared before finalization.'
                    Assert-Uat ((Get-UatFileHash ([string]$diskOwner.silentCandidate.receiptPath)) -ceq [string]$diskOwner.silentCandidate.receiptSha256) 'The owned candidate receipt hash changed before finalization.'
                }
            }
            else { [void](Assert-UatDiagnosticCandidateOwner -Owner $diskOwner -RequireComplete:($script:exitCode -eq 0)) }
        }
        catch { [void]$finalizationErrors.Add(('Candidate owner validation failed: {0}' -f $_.Exception.ToString())) }
        foreach ($candidateKey in $script:diagnosticCandidateKeys) {
            if (-not $script:builds.Contains($candidateKey)) { continue }
            $candidate = $script:builds[$candidateKey]
            $startedCandidateCommands = @($script:activeProcesses | Where-Object {
                [string]::Equals([string]$_.executable, [string]$candidate.path, [System.StringComparison]::OrdinalIgnoreCase)
            })
            if ($startedCandidateCommands.Count -gt 0) { Set-UatCandidateExecuted -Name ([string]$candidate.name) }
        }
        if ($script:builds.Contains('silent')) {
            $silentCandidate = $script:builds['silent']
            $startedCandidateCommands = @($script:activeProcesses | Where-Object {
                [string]::Equals([string]$_.executable, [string]$silentCandidate.path, [System.StringComparison]::OrdinalIgnoreCase)
            })
            if ($startedCandidateCommands.Count -gt 0) {
                Set-UatCandidateExecuted -Name ([string]$silentCandidate.name)
            }
        }
        foreach ($entry in $script:activeProcesses) {
            if ($entry.finished) {
                if ($entry.kind -ceq 'owned-child' -and $null -eq $entry.completedResult) {
                    [void]$finalizationErrors.Add(('Finished owned child has no completed result DTO: {0} PID {1}.' -f $entry.name, $entry.processId))
                }
                continue
            }
            if ($entry.kind -ceq 'owned-child') {
                if ($entry.partialWriteFailed) {
                    [void]$finalizationErrors.Add(('Partial process evidence previously failed for {0} PID {1}; preservation was not retried.' -f $entry.name, $entry.processId))
                    continue
                }
                try { Save-UatProcessPartial -Entry $entry -Diagnostic $script:failureText }
                catch { [void]$finalizationErrors.Add(('Partial process evidence failed for {0} PID {1}: {2}' -f $entry.name, $entry.processId, $_.Exception.ToString())) }
            }
            elseif ($entry.kind -ceq 'external-app') {
                $observedAppExit = $entry.appExitCode
                if ($null -eq $observedAppExit -and $null -ne $entry.process) {
                    try {
                        if ($entry.process.Id -eq $entry.processId -and
                            $entry.process.Handle -eq $entry.pinnedHandle -and $entry.pinnedHandle -ne [IntPtr]::Zero -and
                            $entry.process.HasExited -and $entry.process.StartTime.ToUniversalTime().Ticks -eq $entry.startTicks) {
                            $observedAppExit = [int]$entry.process.ExitCode
                        }
                    }
                    catch { [void]$finalizationErrors.Add(('Pinned external-app exit observation failed for {0} PID {1}: {2}' -f $entry.name, $entry.processId, $_.Exception.ToString())) }
                }
                $incompleteRecord = [ordered]@{
                    commandId = $entry.commandId; sessionId = $entry.sessionId; name = $entry.name
                    executable = $entry.executable; workingDirectory = $entry.workingDirectory
                    processId = [int]$entry.processId; startedUtc = $entry.startedUtc; startTicks = $entry.startTicks
                    phase = 'external-app-incomplete'; appExitCode = $observedAppExit
                    appExitKnown = ($null -ne $observedAppExit)
                    wrapperProcessId = if ($null -ne $entry.wrapperIdentity) { [int]$entry.wrapperIdentity.processId } else { $null }
                    wrapperFinished = [bool]$entry.wrapperFinished; wrapperExitCode = $entry.wrapperExitCode
                    launchHandoff = if ($null -ne $entry.launchHandoff) { $entry.launchHandoff.commandEvidence } else { $null }
                    outputCapture = [string]$entry.outputCapture; diagnostic = $script:failureText
                }
                try {
                    [System.IO.File]::AppendAllText($script:commandsPath, (ConvertTo-Json -InputObject $incompleteRecord -Depth 20 -Compress) + [System.Environment]::NewLine, (New-Object System.Text.UTF8Encoding -ArgumentList $false))
                    Add-UatEvent -Kind 'external-app-incomplete' -Name ([string]$entry.name) -Data @{
                        commandId = $entry.commandId; processId = $entry.processId; appExitCode = $observedAppExit
                        wrapperProcessId = $incompleteRecord.wrapperProcessId; wrapperExitCode = $entry.wrapperExitCode
                    }
                }
                catch { [void]$finalizationErrors.Add(('External-app partial identity record failed for {0} PID {1}: {2}' -f $entry.name, $entry.processId, $_.Exception.ToString())) }
            }
            else { [void]$finalizationErrors.Add(('Unsupported unfinished process record kind {0} for {1} PID {2}.' -f $entry.kind, $entry.name, $entry.processId)) }
        }
    }
    if ($script:mutexAcquired -and $null -ne $script:uatMutex) {
        try {
            $script:uatMutex.ReleaseMutex()
            $script:mutexAcquired = $false
        }
        catch {
            [void]$finalizationErrors.Add(('Owned TEST lifecycle mutex release failed: {0}' -f $_.Exception.ToString()))
        }
    }
    if ($null -ne $script:uatMutex) {
        try { $script:uatMutex.Dispose() }
        catch { [void]$finalizationErrors.Add(('TEST lifecycle mutex disposal failed: {0}' -f $_.Exception.ToString())) }
    }
    if ($finalizationErrors.Count -gt 0) {
        $script:finalizationFailure = $finalizationErrors -join ' | '
        $script:exitCode = 1
        $script:completed = $false
        $script:diagnosticCompleted = $false
        $script:fullsuiteComplete = $false
        if ([string]::IsNullOrEmpty($script:failureText)) { $script:failureText = ([string]$script:selectedCases[0] + ' diagnostic finalization failed.') }
    }
    if ($script:exitCode -ne 0 -and $script:selectedCases[0] -ceq 'A6' -and $script:caseResults['A6'].status -eq 'verified') {
        $script:caseResults['A6'].status = 'pending'
        $script:caseResults['A6'].details = @($script:caseResults['A6'].details) + @('The diagnostic did not pass all common finalization checks; A6 remains pending.')
    }
    if ($ownerIsValid -and -not $script:resultWriteFailed) {
        try {
            Write-UatResult
        }
        catch {
            $script:finalizationFailure = if ([string]::IsNullOrEmpty($script:finalizationFailure)) { $_.Exception.ToString() } else { $script:finalizationFailure + ' | Final result write failed: ' + $_.Exception.ToString() }
            $script:exitCode = 1
            $script:completed = $false
            $script:diagnosticCompleted = $false
            $script:fullsuiteComplete = $false
            if ([string]::IsNullOrEmpty($script:failureText)) { $script:failureText = 'The final diagnostic result could not be written.' }
        }
    }
    elseif ($ownerIsValid -and $script:resultWriteFailed) {
        $script:finalizationFailure = if ([string]::IsNullOrEmpty($script:finalizationFailure)) { $script:resultWriteFailureText } else { $script:finalizationFailure + ' | Previous result write failed: ' + $script:resultWriteFailureText }
        $script:exitCode = 1
        $script:completed = $false
        $script:diagnosticCompleted = $false
        $script:fullsuiteComplete = $false
        if ([string]::IsNullOrEmpty($script:failureText)) { $script:failureText = 'An earlier diagnostic result write failed and was not retried.' }
    }
    if ($script:exitCode -ne 0) {
        if (-not [string]::IsNullOrEmpty($script:failureText)) { [Console]::Error.WriteLine(([string]$script:selectedCases[0] + ' diagnostic failure: ' + $script:failureText)) }
        if (-not [string]::IsNullOrEmpty($script:finalizationFailure)) { [Console]::Error.WriteLine(([string]$script:selectedCases[0] + ' finalization failure: ' + $script:finalizationFailure)) }
    }
}
exit $script:exitCode
