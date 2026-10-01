<#
.SYNOPSIS
  Polls every readable EC byte once a second and reports the ones that move.

.DESCRIPTION
  Written to answer one question: is there a byte that says the fan is running
  flat out right now? Static before-and-after snapshots could not settle it,
  because the sensors drift between them and the controller updates its state
  at no fixed delay after the keypress -- once within 700 ms, once after 5.4 s,
  and once not at all within thirty seconds.

  So this takes the opposite approach: sample continuously while the operator
  toggles Cooler Boost, then print, for every byte that changed at all, the
  whole series of its values with timestamps. A byte that tracks the fan shows
  a step up when the noise starts and a step back when it stops; a thermometer
  wanders instead.

  Needs an elevated session.

.EXAMPLE
  .\poll-ec.ps1 -Seconds 90
#>
[CmdletBinding()]
param(
    [int]    $Seconds  = 90,
    [int]    $EveryMs  = 1000,
    [string] $CsvPath  = (Join-Path $PSScriptRoot 'ec-timeline.csv')
)

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
         ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Needs an elevated session."
    exit 1
}

$classes = @(
    @{ Class = 'MSI_AP';     Property = 'AP' }
    @{ Class = 'MSI_Device'; Property = 'Device' }
    @{ Class = 'MSI_System'; Property = 'System' }
    @{ Class = 'MSI_Power';  Property = 'Power' }
    @{ Class = 'MSI_CPU';    Property = 'CPU' }
    @{ Class = 'MSI_VGA';    Property = 'VGA' }
)

function Get-Sample {
    $sample = [ordered]@{}
    foreach ($entry in $classes) {
        foreach ($instance in @(Get-CimInstance -Namespace root/WMI -ClassName $entry.Class -ErrorAction SilentlyContinue)) {
            # Sorting by the index in the name, never by enumeration order.
            $index = [int]($instance.InstanceName -split '_')[-1]
            $sample["$($entry.Class)[$index]"] = [int]$instance.($entry.Property)
        }
    }
    $sample
}

Write-Host ""
Write-Host "Polling for $Seconds s, every $EveryMs ms." -ForegroundColor Cyan
Write-Host "Toggle Cooler Boost while this runs, and remember roughly when." -ForegroundColor Yellow
Write-Host ""

$samples  = @()
$deadline = (Get-Date).AddSeconds($Seconds)

while ((Get-Date) -lt $deadline) {
    $at = Get-Date
    $row = Get-Sample
    $row.Insert(0, 'time', $at.ToString('HH:mm:ss'))
    $samples += ,$row

    $elapsed = [int]((Get-Date) - $at).TotalMilliseconds
    Write-Host ("  {0}  {1} bytes read in {2} ms" -f $row['time'], ($row.Count - 1), $elapsed)

    $wait = $EveryMs - $elapsed
    if ($wait -gt 0) { Start-Sleep -Milliseconds $wait }
}

$samples | ForEach-Object { [pscustomobject]$_ } | Export-Csv -Path $CsvPath -NoTypeInformation -Encoding utf8
Write-Host ""
Write-Host "timeline saved: $CsvPath" -ForegroundColor Green

# Only the bytes that moved are worth printing; the rest are constants.
$keys = $samples[0].Keys | Where-Object { $_ -ne 'time' }
Write-Host ""
Write-Host "=== bytes that changed during the run ===" -ForegroundColor Cyan

$moved = 0
foreach ($key in $keys) {
    $series = $samples | ForEach-Object { $_[$key] }
    $distinct = $series | Sort-Object -Unique
    if ($distinct.Count -le 1) { continue }

    $moved++
    $span = '{0}..{1}' -f ($distinct | Measure-Object -Minimum).Minimum,
                          ($distinct | Measure-Object -Maximum).Maximum
    Write-Host ("  {0,-18} {1,-10} {2}" -f $key, $span, ($series -join ' '))
}

if (-not $moved) { Write-Host "  nothing moved at all" }
Write-Host ""
Write-Host "A byte tracking the fan steps up with the noise and back down after it." -ForegroundColor DarkGray
