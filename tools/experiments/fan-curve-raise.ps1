# Raises every point of the CPU fan curve (MSI_CPU[12..17], EC 0x73..0x78) to
# 100, watches the tachometer, and puts the original values back. The restore
# is in finally, so it happens on an error or a closed window too; a cold boot
# resets the controller regardless.
#
# Two questions, answered in order: does the controller accept writes to the
# MSI_CPU window at all (read-back), and does the fan follow the curve it
# was given (tachometer).
$ErrorActionPreference = 'Stop'
$log = Join-Path $env:TEMP 'msi-curve-test.log'
Set-Content -Path $log -Value ("=== curve test, {0} ===" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))
function Say { param([string] $Text) Add-Content -Path $log -Value ("{0}  {1}" -f (Get-Date -Format 'HH:mm:ss'), $Text) }

function Read-Window {
    param([string] $Class, [string] $Property)
    Get-CimInstance -Namespace root\WMI -ClassName $Class |
        Sort-Object { [int](($_.InstanceName -split '_')[-1]) } |
        ForEach-Object { [int]$_.$Property }
}
function Get-Point {
    param([int] $Index)
    Get-CimInstance -Namespace root\WMI -ClassName MSI_CPU -Filter ("InstanceName='ACPI\\PNP0C14\\0_{0}'" -f $Index)
}
function Sample {
    param([string] $Tag)
    $cpu = Read-Window 'MSI_CPU' 'CPU'
    $ap  = Read-Window 'MSI_AP'  'AP'
    Say ("{0,-9} tach={1,3}  0x71={2,3}  0x33=0x{3:X2}  cpu={4}C  curve={5}" -f `
        $Tag, $ap[2], $cpu[2], $cpu[0], $cpu[1], (($cpu[12..17]) -join ' '))
}

$indices = 12..17
$original = @{}
foreach ($i in $indices) { $original[$i] = [int](Get-Point $i).CPU }
Say ("original curve: " + (($indices | ForEach-Object { $original[$_] }) -join ' '))

try {
    Say '--- before ---'
    1..5 | ForEach-Object { Sample 'before'; Start-Sleep -Seconds 1 }

    Say 'writing 100 into every point'
    foreach ($i in $indices) {
        $p = Get-Point $i
        Set-CimInstance -InputObject $p -Property @{ CPU = [byte]100 }
    }
    $after = $indices | ForEach-Object { [int](Get-Point $_).CPU }
    Say ("read back: " + ($after -join ' '))
    if (($after | Where-Object { $_ -ne 100 }).Count -gt 0) { Say 'WRITE DID NOT STICK on at least one point' }

    Say '--- after write, 25 s ---'
    1..25 | ForEach-Object { Sample 'raised'; Start-Sleep -Seconds 1 }
}
catch {
    Say ("ERROR: {0}" -f $_.Exception.Message)
}
finally {
    try {
        foreach ($i in $indices) {
            $p = Get-Point $i
            Set-CimInstance -InputObject $p -Property @{ CPU = [byte]$original[$i] }
        }
        $back = $indices | ForEach-Object { [int](Get-Point $_).CPU }
        Say ("restored: " + ($back -join ' ') + "  (expected " + (($indices | ForEach-Object { $original[$_] }) -join ' ') + ")")
    } catch {
        Say ("RESTORE FAILED: {0} - a cold boot resets the controller" -f $_.Exception.Message)
    }
    Say '--- after restore, 15 s ---'
    1..15 | ForEach-Object { Sample 'restored'; Start-Sleep -Seconds 1 }
    Say 'done'
}
