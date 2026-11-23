param(
    [Parameter(Mandatory=$true)][string]$ServiceExecutable,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [string]$PreviousServiceExecutable
)
$ErrorActionPreference='Stop'
$repo = Split-Path $PSScriptRoot -Parent
$payload = (Resolve-Path -LiteralPath $ServiceExecutable).Path
if ((Split-Path $payload -Leaf) -ine 'SDIRIndexService.exe') {
    throw 'Use the compiled SDIRIndexService.exe payload.'
}
$iss = Join-Path $repo 'packaging\windows-index-service\SDIRIndex.iss'
$pin = Get-Content -Raw -LiteralPath (Join-Path $repo 'docs\task-id\windows-installer\compiler.json') | ConvertFrom-Json
if ($pin.version -ne '6.7.4') { throw 'Unexpected configured compiler version.' }
$compiler = Join-Path $pin.installedPath 'ISCC.exe'
if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) { throw 'Pinned compiler unavailable.' }
$destination = [IO.Path]::GetFullPath($OutputDirectory)
$setup = Join-Path $destination 'SDIR-windows-x64-index-service-setup.exe'
$receipt = $setup+'.build.json'
if ((Test-Path -LiteralPath $setup) -or (Test-Path -LiteralPath $receipt)) {
    throw 'The installer output directory already contains a build. Choose a new directory.'
}
New-Item -Path $destination -ItemType Directory -Force | Out-Null
$expected = (Get-FileHash -LiteralPath $payload -Algorithm SHA256).Hash.ToLowerInvariant()
$prior=''
$priorPath=''
$defines=@(('/DServiceExecutable='+$payload),('/DServiceSha256='+$expected))
if ($PreviousServiceExecutable) {
    $priorPath=(Resolve-Path -LiteralPath $PreviousServiceExecutable).Path
    if ((Split-Path $priorPath -Leaf) -ine 'SDIRIndexService.exe') { throw 'Use a verified previous SDIRIndexService.exe release.' }
    $prior=(Get-FileHash -LiteralPath $priorPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($prior -ceq $expected) { throw 'Previous service upgrade payload must differ from the current service binary.' }
    $defines+=('/DPriorServiceSha256='+$prior)
}
$ErrorActionPreference='Continue'
& $compiler @defines ('/O'+$destination) $iss
$code = $LASTEXITCODE
$ErrorActionPreference='Stop'
if ($code -ne 0) { exit $code }
if (-not (Test-Path -LiteralPath $setup -PathType Leaf)) { throw 'The compiler returned no setup executable.' }
$after = (Get-FileHash -LiteralPath $payload -Algorithm SHA256).Hash.ToLowerInvariant()
if ($after -cne $expected) { throw 'The service executable changed during packaging.' }
if ($priorPath -and ((Get-FileHash -LiteralPath $priorPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne $prior)) { throw 'The prior release changed during packaging.' }
@{
    priorServiceSha256=$prior
    setupSha256=(Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash.ToLowerInvariant()
    serviceSha256=$expected
    sourceSha256=(Get-FileHash -LiteralPath $iss -Algorithm SHA256).Hash.ToLowerInvariant()
    setupSignature=(Get-AuthenticodeSignature -LiteralPath $setup).Status.ToString()
    serviceSignature=(Get-AuthenticodeSignature -LiteralPath $payload).Status.ToString()
} | ConvertTo-Json | Set-Content -LiteralPath $receipt -Encoding UTF8
Write-Output $setup
exit 0
