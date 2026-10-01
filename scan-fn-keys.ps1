<#
.SYNOPSIS
  Walks through the Fn combinations one at a time and records the code each one
  raises.

.DESCRIPTION
  probe-msi-events.ps1 maps one button per run, which is slow when there are
  twenty to try. This asks for each combination in turn, waits for the first
  event, and writes it to codes.csv with the label -- so the mapping stays
  unambiguous without a run per key.

  A combination that raises nothing within -WaitSeconds is recorded as silent,
  which is itself a result: the volume and brightness keys are handled by
  Windows over HID and never reach this channel.

  Needs an elevated session, and the event channel must be armed (MSI_Software
  instance 0_0 = 1). It checks and says so rather than waiting in silence.

.EXAMPLE
  .\scan-fn-keys.ps1
#>
[CmdletBinding()]
param(
    [int]    $WaitSeconds = 12,
    [string] $CsvPath     = (Join-Path $PSScriptRoot 'codes.csv'),
    [string[]] $Keys = @(
        'Fn+F1', 'Fn+F2', 'Fn+F3', 'Fn+F4', 'Fn+F5', 'Fn+F6',
        'Fn+F7', 'Fn+F8', 'Fn+F9', 'Fn+F10', 'Fn+F11', 'Fn+F12',
        'Fn+Up', 'Fn+Down', 'Fn+Left', 'Fn+Right',
        'Fn+Home', 'Fn+End', 'Fn+PgUp', 'Fn+PgDn',
        'Fn+Enter', 'Fn+Esc', 'Fn+Del', 'Fn+Ins'
    )
)

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
         ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Needs an elevated session."
    exit 1
}

$armed = (Get-CimInstance -Namespace root/WMI -ClassName MSI_Software |
          Where-Object InstanceName -eq 'ACPI\PNP0C14\0_0').Software
if ($armed -ne 1) {
    Write-Warning "The channel is not armed (MSI_Software 0_0 = $armed): no events will arrive."
    Write-Warning "Start the handler once, or set the byte to 1, and run this again."
    exit 1
}

$sid = "FnScan_$PID"
Get-EventSubscriber -SourceIdentifier $sid -ErrorAction SilentlyContinue |
    Unregister-Event -ErrorAction SilentlyContinue
$null = Register-CimIndicationEvent -Namespace 'root/WMI' -ClassName 'MSI_Event' -SourceIdentifier $sid

if (-not (Test-Path $CsvPath)) {
    'timestamp,label,code_hex,code_dec,instance' | Set-Content -Path $CsvPath -Encoding utf8
}

function Clear-Pending {
    while ($true) {
        $stale = Wait-Event -SourceIdentifier $sid -Timeout 0
        if (-not $stale) { break }
        Remove-Event -EventIdentifier $stale.EventIdentifier
    }
}

Write-Host ""
Write-Host "Fn scan: $($Keys.Count) combinations, up to $WaitSeconds s each." -ForegroundColor Cyan
Write-Host "Press the combination when asked. Do nothing to record it as silent." -ForegroundColor DarkGray
Write-Host "Ctrl+C stops; everything caught so far is already in codes.csv." -ForegroundColor DarkGray
Write-Host ""

$rows    = @()
$results = @()

try {
    foreach ($key in $Keys) {
        Clear-Pending
        Write-Host ("  {0,-10} " -f $key) -NoNewline -ForegroundColor Yellow

        $deadline = (Get-Date).AddSeconds($WaitSeconds)
        $caught   = @()

        while ((Get-Date) -lt $deadline) {
            $event = Wait-Event -SourceIdentifier $sid -Timeout 1
            if ($event) {
                $code = [uint32]$event.SourceEventArgs.NewEvent.MSIEvt
                $instance = $event.SourceEventArgs.NewEvent.InstanceName
                Remove-Event -EventIdentifier $event.EventIdentifier
                $caught += $code
                $rows += '{0},{1},0x{2:X6},{3},{4}' -f `
                    (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $key, $code, $code, $instance
                # Give a key that raises several codes a moment to finish.
                Start-Sleep -Milliseconds 400
                while ($true) {
                    $more = Wait-Event -SourceIdentifier $sid -Timeout 0
                    if (-not $more) { break }
                    $extra = [uint32]$more.SourceEventArgs.NewEvent.MSIEvt
                    Remove-Event -EventIdentifier $more.EventIdentifier
                    $caught += $extra
                    $rows += '{0},{1},0x{2:X6},{3},{4}' -f `
                        (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $key, $extra, $extra, $instance
                }
                break
            }
        }

        if ($caught.Count) {
            $hex = ($caught | ForEach-Object { '0x{0:X6}' -f $_ }) -join ' '
            Write-Host $hex -ForegroundColor Green
            $results += [pscustomobject]@{ Key = $key; Codes = $hex }
        } else {
            Write-Host "тихо" -ForegroundColor DarkGray
            $results += [pscustomobject]@{ Key = $key; Codes = '' }
        }
    }
}
finally {
    Unregister-Event -SourceIdentifier $sid -ErrorAction SilentlyContinue
    if ($rows.Count) {
        $rows | Add-Content -Path $CsvPath -Encoding utf8
        Write-Host ""
        Write-Host "$($rows.Count) event(s) appended to $CsvPath" -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "=== what raised something ===" -ForegroundColor Cyan
$loud = $results | Where-Object { $_.Codes }
if ($loud) { $loud | Format-Table -AutoSize } else { Write-Host "  nothing at all" }

Write-Host "=== silent (handled by firmware or by Windows over HID) ===" -ForegroundColor Cyan
$quiet = ($results | Where-Object { -not $_.Codes }).Key -join ', '
if ($quiet) { Write-Host "  $quiet" } else { Write-Host "  none" }
