param([string]$Base='http://127.0.0.1:18961')
$ErrorActionPreference='Stop'
$page=(Invoke-WebRequest $Base -UseBasicParsing).Content
$token=[regex]::Match($page,'token\s*=\s*"([a-f0-9]{64})"').Groups[1].Value
if(!$token){throw 'Missing session token'}
$headers=@{'X-CoreScope-Token'=$token}
function Snapshot {Invoke-RestMethod "$Base/api/snapshot"}
function Send-Json($path,$body){Invoke-RestMethod "$Base$path" -Method Post -Headers $headers -ContentType application/json -Body ($body|ConvertTo-Json -Depth 16 -Compress)}
$initial=Snapshot
if($initial.automation.config.balance.enabled -or $initial.automation.config.cpu_guard.enabled){throw 'Use an isolated preview profile with automatic modes disabled'}
$originalConfig=$initial.automation.config|ConvertTo-Json -Depth 16|ConvertFrom-Json
$fixtureDirectory=Join-Path (Join-Path (Split-Path -Parent $PSScriptRoot) 'target') ('application-smoke-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $fixtureDirectory|Out-Null
$rootBinary=Join-Path $fixtureDirectory 'corescope-fixture-app.exe'
$helperBinary=Join-Path $fixtureDirectory 'shared-helper.exe'
$batch=Join-Path $fixtureDirectory 'children.cmd'
Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\cmd.exe') -Destination $rootBinary
Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\ping.exe') -Destination $helperBinary
@('@echo off',('start "" /b "'+$helperBinary+'" -n 90 127.0.0.1 >nul'),'ping -n 90 127.0.0.1 >nul')|Set-Content -LiteralPath $batch -Encoding ascii
$fixtures=[Collections.Generic.List[Diagnostics.Process]]::new()
function Start-Fixture {
    $child=Start-Process -FilePath $rootBinary -ArgumentList '/d','/c',$batch -WindowStyle Hidden -PassThru
    $fixtures.Add($child)
    return $child
}
function Remember-Members($snapshot) {
    $ids=[Collections.Generic.HashSet[int]]::new()
    foreach($p in $snapshot.processes){if($p.path -eq $rootBinary -or $p.application_rule -eq $rootBinary){[void]$ids.Add([int]$p.pid)}}
    do {$before=$ids.Count;foreach($p in $snapshot.processes){if($p.parent -and $ids.Contains([int]$p.parent)){[void]$ids.Add([int]$p.pid)}}}while($ids.Count -ne $before)
    $members=@($snapshot.processes|Where-Object {$ids.Contains([int]$_.pid)})
    foreach($p in $members){
        if(!@($fixtures|Where-Object Id -eq $p.pid).Count){
            $process=[Diagnostics.Process]::GetProcessById([int]$p.pid)
            if($process.StartTime.ToFileTimeUtc().ToString() -ne $p.identity){throw 'Fixture PID identity changed'}
            $fixtures.Add($process)
        }
    }
    return $members
}
function Current-Affinity($process){$process.Refresh();return $process.ProcessorAffinity.ToInt64()}
function Has-Core($process,$core){return ((Current-Affinity $process) -band (1L -shl $core)) -ne 0}
try {
    $first=Start-Fixture
    $second=Start-Fixture
    $outsider=Start-Process -FilePath $helperBinary -ArgumentList '-n','90','127.0.0.1' -WindowStyle Hidden -PassThru
    $fixtures.Add($outsider)
    $outsiderAffinity=$outsider.ProcessorAffinity.ToInt64()
    Start-Sleep -Seconds 3
    $snapshot=Snapshot
    $members=@(Remember-Members $snapshot)
    if($members.Count -lt 6){throw 'Expected two app roots and their children'}
    $fixtureMask=0L
    foreach($cpu in $initial.cpus){$fixtureMask=$fixtureMask -bor (1L -shl [int]$cpu.id)}
    foreach($process in $fixtures){$process.ProcessorAffinity=[IntPtr]$fixtureMask}
    $outsiderAffinity=$outsider.ProcessorAffinity.ToInt64()
    $anchor=$snapshot.processes|Where-Object pid -eq $first.Id
    $core=if($anchor.allowed -contains 22){22}else{[int]$anchor.allowed[0]}
    try {Send-Json "/api/process/$($first.Id)/application/core" @{identity='stale';core=$core;enabled=$false}|Out-Null;throw 'Stale app identity accepted'}catch{if(!$_.Exception.Response -or [int]$_.Exception.Response.StatusCode -ne 400){throw}}
    $denied=Send-Json "/api/process/$($first.Id)/application/core" @{identity=$anchor.identity;core=$core;enabled=$false}
    if($denied.failed.Count -or $denied.matched -lt 6 -or $denied.changed -lt 6){throw 'Application deny did not cover roots and descendants'}
    foreach($p in $members){$process=$fixtures|Where-Object Id -eq $p.pid|Select-Object -First 1;if(Has-Core $process $core){throw 'A matching process still permits the excluded CPU'}}
    if((Current-Affinity $outsider) -ne $outsiderAffinity){throw 'Unrelated shared helper was modified'}
    $third=Start-Fixture
    $third.ProcessorAffinity=[IntPtr]$fixtureMask
    Start-Sleep -Seconds 3
    $snapshot=Snapshot
    $members=@(Remember-Members $snapshot)
    foreach($p in $members){$process=$fixtures|Where-Object Id -eq $p.pid|Select-Object -First 1;if(Has-Core $process $core){throw 'Persistent app rule did not cover a new launch/child'}}
    $orphan=$snapshot.processes|Where-Object {$_.path -eq $helperBinary -and $_.parent -eq $first.Id}|Select-Object -First 1
    if(!$orphan){throw 'Expected a disposable helper for detached-parent test'}
    $first.Kill();$first.WaitForExit()
    Start-Sleep -Seconds 2
    $allowed=Send-Json "/api/process/$($orphan.pid)/application/core" @{identity=$orphan.identity;core=$core;enabled=$true}
    if($allowed.rule_target -ne $rootBinary -or $allowed.failed.Count){throw 'Detached helper did not resolve its original application'}
    foreach($p in $fixtures){if(!$p.HasExited -and $p.Id -ne $outsider.Id -and !(Has-Core $p $core)){throw 'Application allow did not restore the selected CPU'}}
    if((Current-Affinity $outsider) -ne $outsiderAffinity){throw 'Unrelated shared helper was modified on allow'}
    Write-Output "PASS: app deny/allow across $($denied.matched) processes; new launch/children, detached helper, unrelated helper isolation, stale identity."
} finally {
    $current=Snapshot
    Send-Json '/api/automation' @{config=$originalConfig;revision=$current.automation.revision}|Out-Null
    foreach($p in $fixtures){try{if(!$p.HasExited){$p.Kill();$p.WaitForExit()}}catch{Write-Warning $_.Exception.Message}}
    foreach($file in @($rootBinary,$helperBinary,$batch)){if(Test-Path -LiteralPath $file){Remove-Item -LiteralPath $file -Force}}
    Remove-Item -LiteralPath $fixtureDirectory
}
