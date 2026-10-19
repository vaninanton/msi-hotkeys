# Stage two of the curve experiment. Writes to MSI_CPU stick, but a curve of
# all-100 moved nothing while fan mode (EC 0xF4) read 0x0C, "auto" — in which,
# per msi-ec, the controller follows its built-in table and ignores the curve
# bytes. So: raise the curve, then switch to "advanced" (0x8C) and watch; if
# still nothing, lower the first temperature point below the current CPU
# temperature and watch again. Everything is put back in finally.
$ErrorActionPreference = 'Stop'
$log = Join-Path $env:TEMP 'msi-curve-test-2.log'
Set-Content -Path $log -Value ("=== curve test 2, {0} ===" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))
function Say { param([string] $Text) Add-Content -Path $log -Value ("{0}  {1}" -f (Get-Date -Format 'HH:mm:ss'), $Text) }

function Read-Window {
    param([string] $Class, [string] $Property)
    Get-CimInstance -Namespace root\WMI -ClassName $Class |
        Sort-Object { [int](($_.InstanceName -split '_')[-1]) } |
        ForEach-Object { [int]$_.$Property }
}
function Get-Inst {
    param([string] $Class, [int] $Index)
    Get-CimInstance -Namespace root\WMI -ClassName $Class -Filter ("InstanceName='ACPI\\PNP0C14\\0_{0}'" -f $Index)
}
function Write-Byte {
    param([string] $Class, [string] $Property, [int] $Index, [int] $Value)
    $o = Get-Inst $Class $Index
    Set-CimInstance -InputObject $o -Property @{ $Property = [byte]$Value }
    [int](Get-Inst $Class $Index).$Property
}
function Sample {
    param([string] $Tag)
    $cpu = Read-Window 'MSI_CPU' 'CPU'
    $ap  = Read-Window 'MSI_AP'  'AP'
    $sys = Read-Window 'MSI_System' 'System'
    Say ("{0,-10} tach={1,3}  0x71={2,3}  0x33=0x{3:X2}  0xF4=0x{4:X2}  cpu={5}C  temps={6}  values={7}" -f `
        $Tag, $ap[2], $cpu[2], $cpu[0], $sys[9], $cpu[1], (($cpu[5..10]) -join ' '), (($cpu[12..17]) -join ' '))
}

$values = 12..17
$temps  = 5..10
$origValues = @{}; foreach ($i in $values) { $origValues[$i] = [int](Get-Inst 'MSI_CPU' $i).CPU }
$origTemps  = @{}; foreach ($i in $temps)  { $origTemps[$i]  = [int](Get-Inst 'MSI_CPU' $i).CPU }
$origMode = [int](Get-Inst 'MSI_System' 9).System
Say ("original: mode=0x{0:X2}  temps={1}  values={2}" -f $origMode, (($temps | ForEach-Object { $origTemps[$_] }) -join ' '), (($values | ForEach-Object { $origValues[$_] }) -join ' '))

try {
    1..3 | ForEach-Object { Sample 'before'; Start-Sleep -Seconds 1 }

    Say 'A: curve values -> 100'
    foreach ($i in $values) { [void](Write-Byte 'MSI_CPU' 'CPU' $i 100) }
    Say ('A: read back ' + (($values | ForEach-Object { [int](Get-Inst 'MSI_CPU' $_).CPU }) -join ' '))

    Say 'B: fan mode 0xF4 -> 0x8C (advanced)'
    $m = Write-Byte 'MSI_System' 'System' 9 0x8C
    Say ("B: read back 0x{0:X2}" -f $m)
    1..20 | ForEach-Object { Sample 'advanced'; Start-Sleep -Seconds 1 }

    Say 'C: first temperature point -> 30'
    $t = Write-Byte 'MSI_CPU' 'CPU' 5 30
    Say ("C: read back {0}" -f $t)
    1..20 | ForEach-Object { Sample 'temp30'; Start-Sleep -Seconds 1 }
}
catch {
    Say ("ERROR: {0}" -f $_.Exception.Message)
}
finally {
    try {
        foreach ($i in $temps)  { [void](Write-Byte 'MSI_CPU' 'CPU' $i $origTemps[$i]) }
        foreach ($i in $values) { [void](Write-Byte 'MSI_CPU' 'CPU' $i $origValues[$i]) }
        $m = Write-Byte 'MSI_System' 'System' 9 $origMode
        Say ("restored: mode=0x{0:X2}  temps={1}  values={2}" -f $m, (($temps | ForEach-Object { [int](Get-Inst 'MSI_CPU' $_).CPU }) -join ' '), (($values | ForEach-Object { [int](Get-Inst 'MSI_CPU' $_).CPU }) -join ' '))
    } catch {
        Say ("RESTORE FAILED: {0} - a cold boot resets the controller" -f $_.Exception.Message)
    }
    1..12 | ForEach-Object { Sample 'restored'; Start-Sleep -Seconds 1 }
    Say 'done'
}
