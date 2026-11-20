param([string]$Executable,[ValidateSet('navigation-smoke.mjs','explorer-commands-smoke.mjs')][string]$Journey,[switch]$ValidateOnly)
$ErrorActionPreference='Stop'
Set-Location -LiteralPath (Split-Path $PSScriptRoot -Parent)
if([Threading.Thread]::CurrentThread.ApartmentState -ne 'STA'){throw 'Run clipboard verification in an STA PowerShell process.'}
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Security
Add-Type -TypeDefinition @'
using System;using System.Drawing;using System.Drawing.Imaging;using System.Runtime.InteropServices;
public class SdirClipboardPixels{
 public static byte[] Read(Bitmap image){using(var b=new Bitmap(image.Width,image.Height,PixelFormat.Format32bppArgb)){using(var g=Graphics.FromImage(b)){g.CompositingMode=System.Drawing.Drawing2D.CompositingMode.SourceCopy;g.DrawImageUnscaled(image,0,0);}var data=b.LockBits(new Rectangle(0,0,b.Width,b.Height),ImageLockMode.ReadOnly,PixelFormat.Format32bppArgb);try{var bytes=new byte[b.Width*b.Height*4];for(int y=0;y<b.Height;y++)Marshal.Copy(IntPtr.Add(data.Scan0,y*data.Stride),bytes,y*b.Width*4,b.Width*4);return bytes;}finally{b.UnlockBits(data);}}}
}
'@ -ReferencedAssemblies System.Drawing
function Hash-Bytes([byte[]]$Bytes){$h=[Security.Cryptography.SHA256]::Create();try{return [Convert]::ToBase64String($h.ComputeHash($Bytes))}finally{$h.Dispose()}}
function Encode-Value($Value){
 if($Value -is [string]){$kind='string';$bytes=[Text.Encoding]::UTF8.GetBytes($Value);$compare=Hash-Bytes $bytes}
 elseif($Value -is [IO.MemoryStream]){$kind='stream';$bytes=$Value.ToArray();$compare=Hash-Bytes $bytes}
 elseif($Value -is [byte[]]){$kind='bytes';$bytes=$Value;$compare=Hash-Bytes $bytes}
 elseif($Value -is [string[]]){$kind='strings';$bytes=[Text.Encoding]::UTF8.GetBytes((ConvertTo-Json -InputObject @($Value) -Compress));$compare=Hash-Bytes $bytes}
 elseif($Value -is [Drawing.Bitmap]){$kind='bitmap';$stream=[IO.MemoryStream]::new();try{$Value.Save($stream,[Drawing.Imaging.ImageFormat]::Png);$bytes=$stream.ToArray()}finally{$stream.Dispose()};$compare=Hash-Bytes ([SdirClipboardPixels]::Read($Value))}
 else{throw 'A clipboard format cannot be preserved; stopped before any test write.'}
 return @{kind=$kind;data=[Convert]::ToBase64String($bytes);compare=$compare}
}
function Decode-Value($Record){$bytes=[Convert]::FromBase64String($Record.data);switch($Record.kind){
 'string'{return [Text.Encoding]::UTF8.GetString($bytes)}
 'stream'{return ,[IO.MemoryStream]::new([byte[]]$bytes)}
 'bytes'{return ,$bytes}
 'strings'{$decoded=ConvertFrom-Json -InputObject ([Text.Encoding]::UTF8.GetString($bytes));if($null -eq $decoded){return ,([string[]]@())};return ,([string[]]$decoded)}
 'bitmap'{$s=[IO.MemoryStream]::new([byte[]]$bytes);try{$image=[Drawing.Bitmap]::new($s);try{return $image.Clone()}finally{$image.Dispose()}}finally{$s.Dispose()}}
 default{throw 'Unsupported protected clipboard format.'}
}}
function Matches-Original($Original){$d=[Windows.Forms.Clipboard]::GetDataObject();if($null -eq $d){return $Original.Count -eq 0};$formats=@($d.GetFormats($false));if($Original.Count -eq 0){return $formats.Count -eq 0};foreach($entry in $Original){if($formats -notcontains $entry.format){return $false};$now=Encode-Value ($d.GetData($entry.format,$false));if($now.kind -ne $entry.kind -or $now.compare -ne $entry.compare){return $false}};return $true}
function Is-TestClipboard([string]$Root){$d=[Windows.Forms.Clipboard]::GetDataObject();$formats=@(if($null -ne $d){$d.GetFormats($false)});$files=[Windows.Forms.Clipboard]::GetFileDropList();if($files.Count -gt 0){foreach($file in $files){if(-not $file.StartsWith($Root+'\',[StringComparison]::OrdinalIgnoreCase)){return $false}};return $true};$text=[Windows.Forms.Clipboard]::GetText();if($text -and $text.Contains($Root+'\')){return $true};return $formats.Count -eq 1 -and $formats[0] -eq 'Preferred DropEffect'}
if($ValidateOnly){
 $bitmap=[Drawing.Bitmap]::new(2,2);$bitmap.SetPixel(0,0,[Drawing.Color]::FromArgb(120,20,40,60))
 $values=@('검증 문자열',[IO.MemoryStream]::new([byte[]]@(1,2,3)),[byte[]]@(0,4,255),[string[]]@('한글','second'),[string[]]@(),$bitmap)
 $count=0;foreach($value in $values){$record=Encode-Value $value;$json=ConvertTo-Json -InputObject $record -Compress;$plain=[Text.Encoding]::UTF8.GetBytes($json);$cipher=[Security.Cryptography.ProtectedData]::Protect($plain,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser);$decoded=[Security.Cryptography.ProtectedData]::Unprotect($cipher,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser);$entry=[Text.Encoding]::UTF8.GetString($decoded)|ConvertFrom-Json;$again=Encode-Value (Decode-Value $entry);if($again.kind -ne $record.kind -or $again.compare -ne $record.compare){throw ("Protected clipboard codec round trip failed at $count : "+$record.kind+' -> '+$again.kind)};$count++}
 @{ok=$true;protectedFormatRoundTrips=$count;clipboardWrites=0}|ConvertTo-Json -Compress
 return
}
$exe=(Resolve-Path -LiteralPath $Executable).Path
$node=(Get-Command node -ErrorAction Stop).Source
$prefix=if($Journey -eq 'navigation-smoke.mjs'){'sdir-clipboard-'}else{'sdir-audit-'}
$fixtureParent=[IO.Path]::GetTempPath()
if($env:SDIR_TEST_FIXTURE_ROOT){
 $fixtureParent=[IO.Path]::GetFullPath($env:SDIR_TEST_FIXTURE_ROOT)
 if(-not $fixtureParent.StartsWith([IO.Path]::GetTempPath(),[StringComparison]::OrdinalIgnoreCase) -or -not (Test-Path -LiteralPath (Join-Path $fixtureParent '.sdir-test-run.json'))){throw 'Unmarked test fixture parent.'}
}
$fixture=Join-Path $fixtureParent ($prefix+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture|Out-Null
[IO.File]::WriteAllText((Join-Path $fixture '.sdir-audit-fixture'),'owned')
$original=[Windows.Forms.Clipboard]::GetDataObject();$entries=@();$formats=@(if($null -ne $original){$original.GetFormats($false)})
foreach($format in $formats){$record=Encode-Value ($original.GetData($format,$false));$record.format=$format;$entries+=$record}
# A private, user-bound backup survives a restore API error. Its contents are never logged.
$backupPath=Join-Path $fixture '.clipboard-backup.dpapi'
$plain=[Text.Encoding]::UTF8.GetBytes((ConvertTo-Json -InputObject @($entries) -Depth 5 -Compress))
$protected=[Security.Cryptography.ProtectedData]::Protect($plain,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser)
[IO.File]::WriteAllBytes($backupPath,$protected)
$original=$null
if(-not(Matches-Original $entries)){throw 'Clipboard changed during backup; stopped before test writes.'}
$exitCode=1;$restored=$false;$preservedNewer=$false;$verified=$false;$restoreErrors=0
$report=if($Journey -eq 'navigation-smoke.mjs'){'docs\task-id\clipboard-navigation\clipboard-restore.json'}else{'docs\task-id\explorer-basics\commands-clipboard-restore.json'}
try{& $node (Join-Path $PSScriptRoot $Journey) $exe $fixture;$exitCode=$LASTEXITCODE}
finally{
 $deadline=[DateTime]::UtcNow.AddSeconds(20)
 while([DateTime]::UtcNow -lt $deadline){
  try{
   if(Matches-Original $entries){$verified=$true;$restored=$true;break}
   if(-not(Is-TestClipboard $fixture)){$preservedNewer=$true;break}
   if($entries.Count -eq 0){[Windows.Forms.Clipboard]::Clear()}
   else{$data=New-Object Windows.Forms.DataObject;foreach($entry in $entries){$data.SetData($entry.format,$false,(Decode-Value $entry))};[Windows.Forms.Clipboard]::SetDataObject($data,$true,5,100)}
  }catch{$restoreErrors++}
  [Windows.Forms.Application]::DoEvents();Start-Sleep -Milliseconds 100
 }
 if($verified -or $preservedNewer){Remove-Item -LiteralPath $backupPath -Force}
 $kept=if(Test-Path -LiteralPath $backupPath){$backupPath}else{$null}
 @{testExitCode=$exitCode;originalFormatNames=$formats;restored=$restored;verified=$verified;preservedNewerClipboard=$preservedNewer;restoreApiErrors=$restoreErrors;protectedBackup=$kept}|ConvertTo-Json|Set-Content -LiteralPath $report -Encoding UTF8
 if(-not $verified -and -not $preservedNewer){throw "Clipboard restore could not be verified. The user-bound encrypted backup remains at $backupPath"}
}
exit $exitCode
