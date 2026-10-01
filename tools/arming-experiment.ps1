<#
.SYNOPSIS
  Finds what the MSI service changes in the MSI_* WMI classes, to see whether
  the event channel can be armed without it.

.DESCRIPTION
  Dumps every instance of every MSI_* class in root\WMI three times: with the
  'Micro Star SCM' service running, shortly after stopping it, and once the
  channel has had time to close (FINDINGS.md: delivery survived the stop for
  some minutes, so an immediate snapshot can be misleading). It then prints the
  properties that differ -- a candidate for the arming flag.

  Read-only as far as WMI goes; the only change it makes is stopping the service
  and starting it again, which it does even if something fails in between.

  Needs an elevated session: root\WMI is not readable otherwise.

.EXAMPLE
  .\arming-experiment.ps1
#>
[CmdletBinding()]
param(
    [int]    $SettleSeconds = 90,
    [string] $OutDir        = (Join-Path $PSScriptRoot 'snapshots'),
    [string] $ServiceName   = 'Micro Star SCM'
)

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
         ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Needs an elevated session: root\WMI is not readable without it."
    exit 1
}

function Get-MsiSnapshot {
    $snapshot = [ordered]@{}

    $classes = Get-CimClass -Namespace root/WMI -ClassName MSI_* |
               Select-Object -ExpandProperty CimClassName | Sort-Object

    foreach ($class in $classes) {
        try {
            $instances = @(Get-CimInstance -Namespace root/WMI -ClassName $class)
        } catch {
            $snapshot[$class] = "ERROR: $($_.Exception.Message)"
            continue
        }

        $dumped = @()
        foreach ($instance in $instances) {
            $props = [ordered]@{}
            foreach ($p in ($instance.CimInstanceProperties | Sort-Object Name)) {
                $value = $p.Value
                if ($value -is [byte[]]) {
                    $value = ($value | ForEach-Object { '{0:X2}' -f $_ }) -join ' '
                }
                $props[$p.Name] = $value
            }
            $dumped += ,$props
        }
        $snapshot[$class] = $dumped
    }

    $snapshot
}

# Flattens a snapshot to "Class[i].Property = value" so two of them can be
# compared line by line.
function Convert-ToFlat {
    param($Snapshot)

    $flat = [ordered]@{}
    foreach ($class in $Snapshot.Keys) {
        $value = $Snapshot[$class]
        if ($value -is [string]) { $flat[$class] = $value; continue }

        for ($i = 0; $i -lt $value.Count; $i++) {
            foreach ($prop in $value[$i].Keys) {
                $flat["$class[$i].$prop"] = [string]$value[$i][$prop]
            }
        }
    }
    $flat
}

function Compare-Snapshot {
    param($Before, $After, [string] $BeforeName, [string] $AfterName)

    $a = Convert-ToFlat $Before
    $b = Convert-ToFlat $After

    $keys = ($a.Keys + $b.Keys) | Sort-Object -Unique
    $differences = @()

    foreach ($k in $keys) {
        $left  = if ($a.Contains($k)) { $a[$k] } else { '<absent>' }
        $right = if ($b.Contains($k)) { $b[$k] } else { '<absent>' }
        if ($left -ne $right) {
            $differences += [pscustomobject]@{
                Property   = $k
                $BeforeName = $left
                $AfterName  = $right
            }
        }
    }
    $differences
}

function Save-Snapshot {
    param($Snapshot, [string] $Name)

    $path = Join-Path $OutDir "msi-state-$Name.json"
    $Snapshot | ConvertTo-Json -Depth 6 | Set-Content -Path $path -Encoding utf8
    Write-Host "  saved $path" -ForegroundColor DarkGray
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

$service = Get-Service -Name $ServiceName
Write-Host ""
Write-Host "Service '$ServiceName' is currently $($service.Status)." -ForegroundColor Cyan

try {
    if ($service.Status -ne 'Running') {
        Write-Host "Starting it for the baseline snapshot..."
        Start-Service -Name $ServiceName
        Start-Sleep -Seconds 5
    }

    Write-Host "[1/3] snapshot with the service RUNNING" -ForegroundColor Cyan
    $running = Get-MsiSnapshot
    Save-Snapshot $running 'running'

    Write-Host "[2/3] stopping the service, snapshot after 5 s" -ForegroundColor Cyan
    Stop-Service -Name $ServiceName -Force
    Start-Sleep -Seconds 5
    $justStopped = Get-MsiSnapshot
    Save-Snapshot $justStopped 'stopped-5s'

    $wait = [Math]::Max(0, $SettleSeconds - 5)
    Write-Host "[3/3] waiting $wait s for the channel to close, then snapshot" -ForegroundColor Cyan
    Start-Sleep -Seconds $wait
    $settled = Get-MsiSnapshot
    Save-Snapshot $settled 'stopped-settled'
}
finally {
    Write-Host ""
    Write-Host "Restoring the service..." -ForegroundColor Yellow
    try {
        Start-Service -Name $ServiceName
        Write-Host "  '$ServiceName' is $((Get-Service -Name $ServiceName).Status)." -ForegroundColor Green
    } catch {
        Write-Warning "COULD NOT RESTART '$ServiceName': $($_.Exception.Message)"
        Write-Warning "Start it by hand: sc start `"$ServiceName`""
    }
}

Write-Host ""
Write-Host "=== running vs stopped+5s ===" -ForegroundColor Cyan
$early = Compare-Snapshot $running $justStopped 'Running' 'Stopped5s'
if ($early) { $early | Format-Table -AutoSize -Wrap } else { Write-Host "  no differences" }

Write-Host ""
Write-Host "=== running vs stopped+$SettleSeconds s  (the interesting one) ===" -ForegroundColor Cyan
$late = Compare-Snapshot $running $settled 'Running' 'Stopped'
if ($late) { $late | Format-Table -AutoSize -Wrap } else { Write-Host "  no differences" }

Write-Host ""
if (-not $late) {
    Write-Host "Nothing visible changed. Either the arming state is not exposed as a" -ForegroundColor Yellow
    Write-Host "property, or it lives somewhere these classes do not show." -ForegroundColor Yellow
} else {
    Write-Host "Candidates above. A Boolean that flips with the service is the one to try" -ForegroundColor Green
    Write-Host "writing while the service is stopped." -ForegroundColor Green
}
