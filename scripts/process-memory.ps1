function Get-SdirProcessMemory([int]$ProcessId){
 $all=@(Get-CimInstance Win32_Process|Select-Object ProcessId,ParentProcessId,CreationDate)
 $byId=@{};foreach($p in $all){$byId[[int]$p.ProcessId]=$p}
 if(-not $byId.ContainsKey($ProcessId) -or -not $byId[$ProcessId].CreationDate){throw 'Owned memory root disappeared.'}
 $epochs=@{};$epochs[$ProcessId]=$byId[$ProcessId].CreationDate
 for($i=0;$i -lt 32;$i++){$n=$epochs.Count;foreach($p in $all){$parent=[int]$p.ParentProcessId;$child=[int]$p.ProcessId;if($epochs.ContainsKey($parent) -and $p.CreationDate -and $p.CreationDate -ge $epochs[$parent]){$epochs[$child]=$p.CreationDate}};if($n -eq $epochs.Count){break}}
 $rows=@(foreach($id in @($epochs.Keys)){
  $p=Get-Process -Id $id -ErrorAction SilentlyContinue;if(-not $p){continue}
  if([Math]::Abs(($p.StartTime.ToUniversalTime()-$epochs[$id].ToUniversalTime()).TotalMilliseconds) -ge 1){continue}
  [pscustomobject]@{pid=$p.Id;name=$p.ProcessName;createdUtc=$epochs[$id].ToUniversalTime().ToString('o');parentPid=[int]$byId[$id].ParentProcessId;privateBytes=$p.PrivateMemorySize64;workingSetBytes=$p.WorkingSet64;cpuMs=$p.CPU*1000}
 })
 if($ProcessId -notin @($rows.pid)){throw 'Owned root identity changed during memory sampling.'}
 return @{processes=$rows;privateBytes=($rows|Measure-Object privateBytes -Sum).Sum;workingSetBytes=($rows|Measure-Object workingSetBytes -Sum).Sum;cpuMs=($rows|Measure-Object cpuMs -Sum).Sum;identityGuard='parent/child creation time and live process start time'}
}

