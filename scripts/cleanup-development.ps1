param([ValidateSet('Inspect','Apply')][string]$Action='Inspect',[string]$Inventory="$PSScriptRoot\..\docs\task-id\performance-optimize\cleanup-inventory.json")
$ErrorActionPreference='Stop'
$task=Split-Path ([IO.Path]::GetFullPath($Inventory)) -Parent
$data=Get-Content -Raw -LiteralPath $Inventory|ConvertFrom-Json
$temp=[IO.Path]::GetTempPath().TrimEnd('\')
if(-not ('SdirOwnedCleanup' -as [type])){
Add-Type -TypeDefinition @'
using System;using System.IO;using System.Collections.Generic;
public class SdirOwnedCleanup {
 public class Size {public long bytes,files,folders;public bool backup;public List<string> errors=new List<string>();}
 public static Size Inspect(string root){var s=new Size();var todo=new Stack<string>();todo.Push(root);while(todo.Count>0){string p=todo.Pop();try{foreach(var entry in new DirectoryInfo(p).EnumerateFileSystemInfos()){if(entry.Name==".clipboard-backup.dpapi")s.backup=true;if((entry.Attributes&FileAttributes.ReparsePoint)!=0)continue;if((entry.Attributes&FileAttributes.Directory)!=0){s.folders++;todo.Push(entry.FullName);}else{s.files++;s.bytes+=((FileInfo)entry).Length;}}}catch(Exception e){s.errors.Add(e.GetType().Name);}}return s;}
 public static void Delete(string root){foreach(var entry in new DirectoryInfo(root).EnumerateFileSystemInfos()){bool dir=(entry.Attributes&FileAttributes.Directory)!=0;if((entry.Attributes&FileAttributes.ReparsePoint)!=0){if(dir)Directory.Delete(entry.FullName,false);else File.Delete(entry.FullName);continue;}if(dir)Delete(entry.FullName);else{entry.Attributes=FileAttributes.Normal;entry.Delete();}}Directory.Delete(root,false);}
}
'@
}
function Guard($item){
 $p=[IO.Path]::GetFullPath($item.path).TrimEnd('\')
 if((Split-Path $p -Parent) -ne $temp -or (Split-Path $p -Leaf) -notlike 'sdir-*'){throw 'Not a direct SDIR fixture under TEMP.'}
 if(-not $item.eligible -or (-not $item.markers.Count -and -not $item.references.Count)){throw 'Missing ownership evidence.'}
 $d=Get-Item -LiteralPath $p -Force
 if($d.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Fixture root is a reparse point.'}
 return $p
}
$before=@(Get-CimInstance Win32_LogicalDisk|Where-Object{$_.DeviceID -in @('C:','E:')}|Select-Object DeviceID,FreeSpace)
if($Action -eq 'Inspect'){
 $rows=@();foreach($item in $data.directories){if(-not $item.eligible){continue};$p=Guard $item;$size=[SdirOwnedCleanup]::Inspect($p);$rows+=[pscustomobject]@{path=$p;bytes=$size.bytes;files=$size.files;folders=$size.folders;backup=$size.backup;errors=@($size.errors);eligible=(-not $size.backup -and $size.errors.Count -eq 0)}}
 @{before=$before;directories=$rows}|ConvertTo-Json -Depth 7|Set-Content -LiteralPath (Join-Path $task 'cleanup-sized.json') -Encoding UTF8
 @{folders=$rows.Count;logicalBytes=($rows|Measure-Object bytes -Sum).Sum;files=($rows|Measure-Object files -Sum).Sum;preservedForBackup=@($rows|Where-Object{$_.backup}).Count;inspectionErrors=@($rows|Where-Object{$_.errors.Count}).Count}|ConvertTo-Json -Compress
 exit 0
}
$sized=Get-Content -Raw -LiteralPath (Join-Path $task 'cleanup-sized.json')|ConvertFrom-Json
$processes=@(Get-CimInstance Win32_Process)
$results=@();foreach($row in $sized.directories){
 if(-not $row.eligible){continue};$item=@($data.directories|Where-Object{$_.path -eq $row.path})[0]
 if(-not(Test-Path -LiteralPath $row.path)){continue};$p=Guard $item
 $active=@($processes|Where-Object{($_.ExecutablePath -and $_.ExecutablePath.StartsWith($p+'\',[StringComparison]::OrdinalIgnoreCase)) -or ($_.CommandLine -and $_.CommandLine.IndexOf($p,[StringComparison]::OrdinalIgnoreCase) -ge 0)})
 if($active.Count){$results+=@{path=$p;deleted=$false;reason='active process'};continue}
 $check=[SdirOwnedCleanup]::Inspect($p)
 if($check.backup -or $check.errors.Count){$results+=@{path=$p;deleted=$false;reason='protected backup or unreadable contents'};continue}
 try{[SdirOwnedCleanup]::Delete($p);$results+=@{path=$p;deleted=$true;logicalBytes=$check.bytes;files=$check.files}}
 catch{$results+=@{path=$p;deleted=$false;reason=$_.Exception.GetBaseException().GetType().Name;detail=$_.Exception.GetBaseException().Message}}
 $results|ConvertTo-Json -Depth 5|Set-Content -LiteralPath (Join-Path $task 'cleanup-receipts.json') -Encoding UTF8
}
$after=@(Get-CimInstance Win32_LogicalDisk|Where-Object{$_.DeviceID -in @('C:','E:')}|Select-Object DeviceID,FreeSpace)
@{before=$before;after=$after;deleted=@($results|Where-Object{$_.deleted}).Count;skipped=@($results|Where-Object{-not $_.deleted}).Count;logicalBytes=($results|Where-Object{$_.deleted}|ForEach-Object{[double]$_.logicalBytes}|Measure-Object -Sum).Sum}|ConvertTo-Json -Depth 6|Set-Content -LiteralPath (Join-Path $task 'cleanup-summary.json') -Encoding UTF8
Write-Host 'Owned fixture cleanup completed; see receipts and drive free-space snapshots.'
