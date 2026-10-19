# Clears bit 0 of EC 0x33 (MSI_CPU[0]), which msi-laptop documents as the
# auto-fan flag: "if zero, fan rotates at maximal speed". Watches the
# tachometer for 25 s, then writes the original byte back. The restore is in
# finally; a cold boot resets the controller regardless.
$ErrorActionPreference = 'Stop'
$log = Join-Path $env:TEMP 'msi-autofan-test.log'
Set-Content -Path $log -Value ("=== auto-fan bit test, {0} ===" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))
function Say { param([string] $Text) Add-Content -Path $log -Value ("{0}  {1}" -f (Get-Date -Format 'HH:mm:ss'), $Text) }

function Read-Window {
    param([string] $Class, [string] $Property)
    Get-CimInstance -Namespace root\WMI -ClassName $Class |
        Sort-Object { [int](($_.InstanceName -split '_')[-1]) } |
        ForEach-Object { [int]$_.$Property }
}
function Get-Inst {
    Get-CimInstance -Namespace root\WMI -ClassName MSI_CPU -Filter "InstanceName='ACPI\\PNP0C14\\0_0'"
}
function Sample {
    param([string] $Tag)
    $cpu = Read-Window 'MSI_CPU' 'CPU'
    $ap  = Read-Window 'MSI_AP'  'AP'
    Say ("{0,-9} tach={1,3}  0x33=0x{2:X2} (bit0={3})  0x71={4,3}  cpu={5}C" -f `
        $Tag, $ap[2], $cpu[0], ($cpu[0] -band 1), $cpu[2], $cpu[1])
}

$original = [int](Get-Inst).CPU
Say ("original 0x33 = 0x{0:X2}" -f $original)
if (($original -band 1) -eq 0) {
    Say 'bit 0 already clear - nothing to test'
    Say 'done'
    exit 0
}

try {
    1..5 | ForEach-Object { Sample 'before'; Start-Sleep -Seconds 1 }

    $target = $original -band 0xFE
    Say ("writing 0x{0:X2} -> 0x{1:X2}" -f $original, $target)
    Set-CimInstance -InputObject (Get-Inst) -Property @{ CPU = [byte]$target }
    $after = [int](Get-Inst).CPU
    Say ("read back 0x{0:X2}" -f $after)
    if ($after -ne $target) { Say 'WRITE DID NOT STICK' }

    1..25 | ForEach-Object { Sample 'cleared'; Start-Sleep -Seconds 1 }
}
catch {
    Say ("ERROR: {0}" -f $_.Exception.Message)
}
finally {
    try {
        Set-CimInstance -InputObject (Get-Inst) -Property @{ CPU = [byte]$original }
        Say ("restored 0x{0:X2} (expected 0x{1:X2})" -f ([int](Get-Inst).CPU), $original)
    } catch {
        Say ("RESTORE FAILED: {0} - a cold boot resets the controller" -f $_.Exception.Message)
    }
    1..12 | ForEach-Object { Sample 'restored'; Start-Sleep -Seconds 1 }
    Say 'done'
}
