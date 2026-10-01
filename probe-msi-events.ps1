<#
.SYNOPSIS
  Listens for MSI_Event WMI indications and logs the MSIEvt codes.

.DESCRIPTION
  Hardware buttons and Fn combos on MSI laptops raise an ACPI-WMI event
  (root\WMI : MSI_Event, property MSIEvt : UInt32). This script subscribes to
  them and prints every code it sees, appending to codes.csv next to itself.

  It waits for presses rather than running down a clock: it keeps listening
  until -Count events arrive, or -TimeoutSeconds elapses. So there is no rush
  to reach the keyboard.

  Workflow for mapping a button: run with -Label naming the button, then press
  ONLY that button. One button per run keeps the mapping unambiguous.

.EXAMPLE
  .\probe-msi-events.ps1 -Label CoolerBoost

.EXAMPLE
  .\probe-msi-events.ps1 -Label FnBrightnessUp -Count 2
#>
[CmdletBinding()]
param(
    [string] $Label          = 'unlabeled',
    [int]    $Count          = 3,
    [int]    $TimeoutSeconds = 120,
    [string] $CsvPath        = (Join-Path $PSScriptRoot 'codes.csv')
)

$sid = "MSIEvtProbe_$PID"

Get-EventSubscriber -SourceIdentifier $sid -ErrorAction SilentlyContinue |
    Unregister-Event -ErrorAction SilentlyContinue

try {
    $null = Register-CimIndicationEvent -Namespace 'root/WMI' `
                -ClassName 'MSI_Event' -SourceIdentifier $sid -ErrorAction Stop
} catch {
    Write-Error "Cannot subscribe to MSI_Event: $($_.Exception.Message)"
    Write-Host  "Hint: this usually needs an elevated session."
    exit 1
}

if (-not (Test-Path $CsvPath)) {
    'timestamp,label,code_hex,code_dec,instance' | Set-Content -Path $CsvPath -Encoding utf8
}

Write-Host ""
Write-Host "Label: '$Label'" -ForegroundColor Cyan
Write-Host "Waiting for $Count event(s). Press the button now - take your time."
Write-Host "Gives up after $TimeoutSeconds s. Ctrl+C to stop early." -ForegroundColor DarkGray
Write-Host ""

$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
$rows     = @()

while ($rows.Count -lt $Count -and (Get-Date) -lt $deadline) {
    $e = Wait-Event -SourceIdentifier $sid -Timeout 2
    if ($e) {
        $n    = $e.SourceEventArgs.NewEvent
        $code = [uint32]$n.MSIEvt
        $hex  = '0x{0:X6}' -f $code
        $ts   = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'

        Write-Host ("  [{0}] {1}  (dec {2})   {3}/{4}" -f `
                    $ts.Substring(11), $hex, $code, ($rows.Count + 1), $Count)
        $rows += '{0},{1},{2},{3},{4}' -f $ts, $Label, $hex, $code, $n.InstanceName

        Remove-Event -EventIdentifier $e.EventIdentifier
    }
}

Unregister-Event -SourceIdentifier $sid

if ($rows.Count) {
    $rows | Add-Content -Path $CsvPath -Encoding utf8
    Write-Host ""
    Write-Host "$($rows.Count) event(s) appended to $CsvPath" -ForegroundColor Green
    Write-Host ""
    Write-Host "Distinct codes this run:"
    $rows | ForEach-Object { ($_ -split ',')[2] } | Group-Object |
        Sort-Object Count -Descending |
        ForEach-Object { "  {0}  x{1}" -f $_.Name, $_.Count }
} else {
    Write-Host ""
    Write-Host "Timed out with no events." -ForegroundColor Yellow
    Write-Host "Checklist:" -ForegroundColor Yellow
    Write-Host "  - elevated session? (subscription succeeded, so probably yes)"
    Write-Host "  - pressing a dedicated MSI button, not a plain Fn combo?"
    Write-Host "    brightness/volume go through HID, not necessarily MSI_Event"
}
