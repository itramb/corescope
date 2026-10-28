param()
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
$env:CORESCOPE_SENSOR_MODE='extended'
$env:CORESCOPE_HARDWARE_DIR=Join-Path $root 'assets\hardware'
$output=& powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $root 'assets\sensors.ps1')
if($LASTEXITCODE -ne 0){throw 'Sensor collector failed'}
$result=$output|ConvertFrom-Json
if($result.provider -ne 'LibreHardwareMonitorLib'){throw 'Wrong provider'}
if(!$result.diagnostics.library_version){throw 'Missing collector diagnostics'}
$cpuTemperatures=@($result.sensors|Where-Object {$_.hardware_type -eq 'Cpu' -and $_.kind -eq 'Temperature' -and $_.name -notmatch 'Distance|Tj\s*Max|Margin'})
if(@($cpuTemperatures|Where-Object {$_.value -le 0}).Count){throw 'Invalid CPU temperature was exposed'}
if($cpuTemperatures.Count -ne $result.diagnostics.cpu_temperature_reading_count){throw 'Temperature count does not match diagnostics'}
if($cpuTemperatures.Count){
    if($result.diagnostics.cpu_temperature_status -ne 'available'){throw 'Valid temperatures incorrectly reported unavailable'}
} elseif($result.diagnostics.cpu_temperature_sensor_count -gt 0){
    if($result.diagnostics.cpu_temperature_status -notin @('driver_missing','access_required','unavailable')){throw 'Missing temperature reason was not exposed'}
    if(!$result.errors.Count){throw 'Missing CPU temperatures must have an actionable explanation'}
    if($result.diagnostics.pawnio_installed -eq $false -and $result.diagnostics.cpu_temperature_status -ne 'driver_missing'){throw 'Missing PawnIO was not identified'}
}
Write-Output "PASS: CPU temperature status=$($result.diagnostics.cpu_temperature_status); valid readings=$($cpuTemperatures.Count); PawnIO=$($result.diagnostics.pawnio_installed); library=$($result.diagnostics.library_version)."
