$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$result = [Collections.Generic.List[object]]::new()
$errors = [Collections.Generic.List[string]]::new()
$provider = 'Windows ACPI / Storage'
$diagnostics = [ordered]@{}
if ($env:CORESCOPE_SENSOR_MODE -ne 'extended') {
    try {
        foreach ($zone in @(Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature -OperationTimeoutSec 3)) {
            $value = [math]::Round($zone.CurrentTemperature / 10.0 - 273.15, 1)
            if ($value -gt -50 -and $value -lt 150) {
                $result.Add(@{name=$zone.InstanceName; identifier="windows/acpi/$($zone.InstanceName)"; parent='ACPI thermal zone'; kind='Temperature'; value=$value})
            }
        }
    } catch { $errors.Add('ACPI: ' + $_.Exception.Message) }
    try {
        foreach ($disk in @(Get-PhysicalDisk)) {
            try {
                $counter = $disk | Get-StorageReliabilityCounter
                if ($null -ne $counter.Temperature -and $counter.Temperature -gt 0 -and $counter.Temperature -lt 150) {
                    $result.Add(@{name=$disk.FriendlyName;identifier="windows/storage/$($disk.DeviceId)";parent="Storage $($disk.DeviceId)";kind='Temperature';value=[double]$counter.Temperature})
                }
            } catch { $errors.Add('Storage: ' + $_.Exception.Message) }
        }
    } catch { $errors.Add('Storage: ' + $_.Exception.Message) }
} else {
    $provider = 'LibreHardwareMonitorLib'
    $computer = $null
    try {
        $assembly = [Reflection.Assembly]::LoadFrom((Join-Path $env:CORESCOPE_HARDWARE_DIR 'LibreHardwareMonitorLib.dll'))
        $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
        $diagnostics.is_administrator = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        $diagnostics.library_version = $assembly.GetName().Version.ToString()
        $pawnIoType = $assembly.GetType('LibreHardwareMonitor.PawnIo.PawnIo', $false)
        $diagnostics.pawnio_installed = if ($null -ne $pawnIoType) { [bool]$pawnIoType.GetProperty('IsInstalled').GetValue($null, $null) } else { $null }
        $pawnIoVersion = if ($null -ne $pawnIoType) { $pawnIoType.GetProperty('Version').GetValue($null, $null) } else { $null }
        $diagnostics.pawnio_version = if ($null -ne $pawnIoVersion) { $pawnIoVersion.ToString() } else { $null }
        $diagnostics.cpu_temperature_status = 'not_detected'
        $script:cpuHardwareCount = 0
        $script:cpuTemperatureSensorCount = 0
        $script:cpuTemperatureReadingCount = 0
        $computer = New-Object LibreHardwareMonitor.Hardware.Computer
        $computer.IsCpuEnabled = $true
        $computer.IsGpuEnabled = $true
        $computer.IsMemoryEnabled = $true
        $computer.IsMotherboardEnabled = $true
        $computer.IsControllerEnabled = $true
        $computer.Open()
        function Read-Hardware($hardware) {
            try {
                $hardware.Update()
                $isCpu = $hardware.HardwareType.ToString() -eq 'Cpu'
                if ($isCpu) { $script:cpuHardwareCount++ }
                foreach ($sensor in $hardware.Sensors) {
                    $kind = $sensor.SensorType.ToString()
                    # Null values are normal when low-level CPU access is unavailable.
                    # Count actual thermal sensors separately so this is reported to the UI.
                    $isCpuTemperature = $isCpu -and $kind -eq 'Temperature' -and $sensor.Name -notmatch 'Distance|Tj\s*Max|Margin'
                    if ($isCpuTemperature) {
                        $script:cpuTemperatureSensorCount++
                        if ($null -ne $sensor.Value -and $sensor.Value -gt 0) { $script:cpuTemperatureReadingCount++ }
                    }
                    if ($kind -in @('Temperature','Fan','Power','Voltage','Clock','Load','Factor') -and $null -ne $sensor.Value) {
                        if ($isCpuTemperature -and $sensor.Value -eq 0) { $errors.Add('CPU temperature unavailable: zero reading'); continue }
                        # Only an explicit flag can establish throttling, never a temperature or a low clock.
                        if ($kind -eq 'Factor') {
                            if ($sensor.Name -match 'throttl' -and $sensor.Value -in @(0,1)) { $kind='Throttling' } else { continue }
                        }
                        $result.Add(@{name=$sensor.Name;identifier=$sensor.Identifier.ToString();parent=$hardware.Identifier.ToString();hardware_type=$hardware.HardwareType.ToString();kind=$kind;value=[double]$sensor.Value})
                    }
                }
                foreach ($sub in $hardware.SubHardware) { Read-Hardware $sub }
            } catch { $errors.Add($hardware.Name + ': ' + $_.Exception.Message) }
        }
        foreach ($hardware in $computer.Hardware) { Read-Hardware $hardware }
        $diagnostics.cpu_temperature_sensor_count = $script:cpuTemperatureSensorCount
        $diagnostics.cpu_temperature_reading_count = $script:cpuTemperatureReadingCount
        if ($script:cpuTemperatureReadingCount -gt 0) {
            $diagnostics.cpu_temperature_status = 'available'
        } elseif ($script:cpuHardwareCount -gt 0) {
            if ($diagnostics.pawnio_installed -eq $false) {
                $diagnostics.cpu_temperature_status = 'driver_missing'
                $errors.Add('CPU temperature unavailable: PawnIO driver is not installed. Install PawnIO from https://pawnio.eu and restart CoreScope.')
            } elseif (-not $diagnostics.is_administrator) {
                $diagnostics.cpu_temperature_status = 'access_required'
                $errors.Add('CPU temperature unavailable: low-level sensor access failed. Restart CoreScope as administrator and check PawnIO.')
            } else {
                $diagnostics.cpu_temperature_status = 'unavailable'
                $errors.Add('CPU temperature unavailable: LibreHardwareMonitor returned no valid CPU temperature readings. Check the PawnIO driver and CPU support.')
            }
        }
    } catch { $errors.Add($_.Exception.Message) }
    finally { if ($null -ne $computer) { try { $computer.Close() } catch {} } }
}
@{provider=$provider;sensors=@($result.ToArray());errors=@($errors.ToArray());diagnostics=$diagnostics} | ConvertTo-Json -Depth 5 -Compress
