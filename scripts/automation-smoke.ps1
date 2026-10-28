param([string]$Base='http://127.0.0.1:18961')
$ErrorActionPreference='Stop'
$page=(Invoke-WebRequest $Base -UseBasicParsing).Content
$token=[regex]::Match($page,'token\s*=\s*"([a-f0-9]{64})"').Groups[1].Value
if(!$token){throw 'Missing session token'}
$headers=@{'X-CoreScope-Token'=$token}
function Send-Json($path,$body){Invoke-RestMethod "$Base$path" -Method Post -Headers $headers -ContentType application/json -Body ($body|ConvertTo-Json -Depth 12 -Compress)}
function Snapshot {Invoke-RestMethod "$Base/api/snapshot"}
$initial=Snapshot
if($initial.automation.config.rules.Count -or $initial.automation.config.balance.enabled -or $initial.automation.config.cpu_guard.enabled){throw 'Run this test against an isolated profile with automation disabled'}
$testDirectory=Join-Path (Join-Path (Split-Path -Parent $PSScriptRoot) 'target') ('corescope-smoke-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $testDirectory | Out-Null
$testBinary=Join-Path $testDirectory 'corescope-disposable-worker.exe'
Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\ping.exe') -Destination $testBinary
$child=$null
try {
    $child=Start-Process -FilePath $testBinary -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru
    $processSnapshot=$null
    for($i=0;$i -lt 10;$i++) {Start-Sleep -Milliseconds 400;$processSnapshot=(Snapshot).processes|Where-Object pid -eq $child.Id;if($processSnapshot.identity){break}}
    if(!$processSnapshot.identity){throw 'Disposable child unavailable'}
    $targetCore=[int]$processSnapshot.allowed[0]
    $r=Send-Json "/api/process/$($child.Id)/priority" @{identity=$processSnapshot.identity;priority='below_normal'}
    if($r.priority -ne 'below_normal'){throw 'Priority update failed'}
    try {Send-Json "/api/process/$($child.Id)/priority" @{identity='stale';priority='normal'}|Out-Null;throw 'Stale identity accepted'}catch{if(!$_.Exception.Response -or [int]$_.Exception.Response.StatusCode -ne 400){throw}}
    $config=$initial.automation.config
    $config.rules=@(@{target=$testBinary;priority='below_normal';cores=@($targetCore);exclude_balance=$false;exclude_guard=$false})
    $saved=Send-Json '/api/automation' @{config=$config;revision=$initial.automation.revision}
    try {Send-Json '/api/automation' @{config=$config;revision=$initial.automation.revision}|Out-Null;throw 'Stale revision accepted'}catch{if(!$_.Exception.Response -or [int]$_.Exception.Response.StatusCode -ne 400){throw}}
    Start-Sleep -Seconds 2
    $p=(Snapshot).processes|Where-Object pid -eq $child.Id
    if($p.priority -ne 'below_normal' -or $p.affinity.Count -ne 1 -or $p.affinity[0] -ne $targetCore){throw 'Persistent rule was not applied'}
    Stop-Process -Id $child.Id -Force;$child.WaitForExit()
    $child=Start-Process -FilePath $testBinary -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru
    Start-Sleep -Seconds 3
    $p=(Snapshot).processes|Where-Object pid -eq $child.Id
    if($p.priority -ne 'below_normal' -or $p.affinity.Count -ne 1){throw 'Rule did not survive process restart'}
    $logCore=if($initial.cpus.Count -gt 22){22}else{0}
    Send-Json '/api/activity/logging' @{core=$logCore;enabled=$true}|Out-Null
    Start-Sleep -Seconds 3
    $history=Invoke-RestMethod "$Base/api/activity/history?core=$logCore&minutes=1"
    if(!$history.samples.Count){throw 'No log samples'}
    $s=Snapshot
    if(!$s.latency.history.Count){throw 'No wake-up latency measurements'}
    if($s.activity.status -eq 'running' -and (!$s.activity.recent.Count -or !$s.activity.interrupts.Count)){throw 'ETW history or DPC/ISR statistics missing'}
    Write-Output "PASS: priority, identity guard, persistent rule across restart, configuration revision, CPU log ($($history.samples.Count) samples), wake-up latency. ETW status: $($s.activity.status)."
} finally {
    $current=Snapshot
    $initial.automation.config.rules=@()
    Send-Json '/api/automation' @{config=$initial.automation.config;revision=$current.automation.revision}|Out-Null
    Send-Json '/api/settings' $initial.settings|Out-Null
    if($child -and !$child.HasExited){Stop-Process -Id $child.Id -Force;$child.WaitForExit()}
    Remove-Item -LiteralPath $testBinary -Force
    Remove-Item -LiteralPath $testDirectory
}
