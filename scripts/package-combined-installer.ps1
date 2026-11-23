[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$AppSetup,
    [Parameter(Mandatory=$true)][string]$ServiceSetup,
    [Parameter(Mandatory=$true)][string]$ServiceExecutable,
    [Parameter(Mandatory=$true)][string]$OutputPath,
    [string]$PreviousServiceExecutable
)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$repo=Split-Path $PSScriptRoot -Parent
function Hash([string]$File){ (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash.ToLowerInvariant() }
function Full([string]$File){ (Resolve-Path -LiteralPath $File).Path }
$app=Full $AppSetup
$service=Full $ServiceSetup
$image=Full $ServiceExecutable
$appReceipt=Get-Content -LiteralPath ($app+'.build.json') -Raw | ConvertFrom-Json
$serviceReceipt=Get-Content -LiteralPath ($service+'.build.json') -Raw | ConvertFrom-Json
$appHash=Hash $app
$serviceHash=Hash $service
$imageHash=Hash $image
if ($appHash -cne $appReceipt.setupSha256 -or $serviceHash -cne $serviceReceipt.setupSha256 -or $imageHash -cne $serviceReceipt.serviceSha256) {
    throw 'The build receipt and an embedded setup or service executable differ.'
}
$priorHash=''
if ($PreviousServiceExecutable) { $priorHash=Hash (Full $PreviousServiceExecutable) }
if ($priorHash -cne $serviceReceipt.priorServiceSha256) { throw 'The embedded service installer and combined installer must pin the same previous release.' }
$output=$ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputPath)
if ((Test-Path -LiteralPath $output) -or (Test-Path -LiteralPath ($output+'.build.json'))) {throw 'The combined installer output already exists.'}
$stage=Join-Path (Split-Path -Parent $output) ('combined-compiler-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
$utf8=[Text.UTF8Encoding]::new($false)
Copy-Item -LiteralPath (Join-Path $repo 'packaging\windows-bootstrapper\bootstrap.cpp') -Destination (Join-Path $stage 'bootstrap.cpp')
Copy-Item -LiteralPath $app -Destination (Join-Path $stage 'AppPayload.exe')
Copy-Item -LiteralPath $service -Destination (Join-Path $stage 'ServicePayload.exe')
if ((Hash (Join-Path $stage 'AppPayload.exe')) -cne $appHash -or (Hash (Join-Path $stage 'ServicePayload.exe')) -cne $serviceHash) {throw 'Staged executable hashes differ.'}
$appBytes=(Get-Item -LiteralPath $app).Length
$serviceBytes=(Get-Item -LiteralPath $service).Length
$appId=if ($appReceipt.testBuild) { [string]$appReceipt.testAppId } else { 'SDIR-PerUser-4E7E0A18-79E0-41C0-97C5-65A10A79B3F4' }
if ($appId -cnotmatch '^[A-Za-z0-9-]{1,80}$') { throw 'Invalid AppId in application install receipt.' }
$header=@(
 '#pragma once',
 ('constexpr wchar_t APP_LINK_KEY[]=L"Software\\SDIR\\Installations\\{0}";' -f $appId),
 ('constexpr wchar_t APP_INSTALL_MUTEX[]=L"Local\\SDIR-InstallBundle-{0}";' -f $appId),
 'constexpr int APP_PAYLOAD_ID=101, SERVICE_PAYLOAD_ID=102;',
 ('constexpr DWORD APP_SETUP_BYTES={0}, SERVICE_SETUP_BYTES={1};' -f $appBytes,$serviceBytes),
 ('constexpr wchar_t APP_SETUP_SHA[]=L"{0}", SERVICE_SETUP_SHA[]=L"{1}";' -f $appHash,$serviceHash),
 ('constexpr wchar_t SERVICE_IMAGE_SHA[]=L"{0}", PRIOR_SERVICE_SHA[]=L"{1}";' -f $imageHash,$priorHash)
) -join "`r`n"
[IO.File]::WriteAllText((Join-Path $stage 'payload.h'),$header,$utf8)
[IO.File]::WriteAllLines((Join-Path $stage 'payload.rc'),@('101 RCDATA "AppPayload.exe"','102 RCDATA "ServicePayload.exe"'),$utf8)
$vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $vs -or @($vs).Count -ne 1) {throw 'MSVC x64 build tools were not found.'}
$vcvars=Full (Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat')
if ($vcvars -match '["%\r\n]') {throw 'Unexpected compiler environment path.'}
$runnerPath=Join-Path $stage 'capture-build-env.cmd'
[IO.File]::WriteAllLines($runnerPath,@('@echo off',('call "'+$vcvars+'" >nul'),'if errorlevel 1 exit /b %errorlevel%','set'),$utf8)
$cmd=Join-Path $env:SystemRoot 'System32\cmd.exe'
$saved=@{}
Push-Location -LiteralPath $stage
try {
    $environment=& $cmd /d /c capture-build-env.cmd
    if ($LASTEXITCODE -ne 0) {throw 'MSVC compiler environment initialization failed.'}
    foreach ($name in @('Path','Include','Lib','LibPath')) {
        $value=@($environment | Where-Object { $_ -imatch ('^'+$name+'=') })
        if ($value.Count -ne 1) {throw "A compiler environment item is missing: $name"}
        $saved[$name]=[Environment]::GetEnvironmentVariable($name,'Process')
        [Environment]::SetEnvironmentVariable($name,$value[0].Substring($name.Length+1),'Process')
    }
    $rc=(Get-Command rc.exe -ErrorAction Stop).Source
    $cl=(Get-Command cl.exe -ErrorAction Stop).Source
    $ErrorActionPreference='Continue'
    & $rc /nologo /fo payload.res payload.rc
    $code=$LASTEXITCODE
    $ErrorActionPreference='Stop'
    if ($code -ne 0) {exit $code}
    $ErrorActionPreference='Continue'
    & $cl /nologo /std:c++17 /utf-8 /O2 /W4 /EHsc /MT /DUNICODE /D_UNICODE bootstrap.cpp payload.res /Fe:bundle.exe /link /SUBSYSTEM:WINDOWS '/MANIFESTUAC:level="asInvoker" uiAccess="false"' bcrypt.lib advapi32.lib shell32.lib ole32.lib user32.lib uuid.lib
    $code=$LASTEXITCODE
    $ErrorActionPreference='Stop'
    if ($code -ne 0) {exit $code}
    $binary=Join-Path $stage 'bundle.exe'
    $process=Start-Process -FilePath $binary -ArgumentList '--verify-payloads' -PassThru -Wait
    if ($process.ExitCode -ne 0) {throw ('Compiled resources failed validation: '+$process.ExitCode)}
    Copy-Item -LiteralPath $binary -Destination $output
    $digest=Hash $output
    [ordered]@{
        artifactKind='combined-windows-installer'
        setupSha256=$digest
        setupBytes=(Get-Item -LiteralPath $output).Length
        embeddedAppSetupSha256=$appHash
        embeddedAppSha256=$appReceipt.embeddedAppSha256
        embeddedServiceSetupSha256=$serviceHash
        serviceSha256=$imageHash
        priorServiceSha256=$priorHash
        signature=(Get-AuthenticodeSignature -LiteralPath $output).Status.ToString()
        testBuild=[bool]$appReceipt.testBuild
        testAppId=$appReceipt.testAppId
        testAppName=$appReceipt.testAppName
        testRootName=$appReceipt.testRootName
        testProfileDirectory=$appReceipt.testProfileDirectory
        resourceVerificationExitCode=$process.ExitCode
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath ($output+'.build.json') -Encoding UTF8
    if ((Hash $app) -cne $appHash -or (Hash $service) -cne $serviceHash) {throw 'An embedded setup input changed during packaging.'}
    Write-Output $output
} finally {
    Pop-Location
    foreach ($name in $saved.Keys) {[Environment]::SetEnvironmentVariable($name,$saved[$name],'Process')}
}
exit 0
