<#
.SYNOPSIS
  Finds the one thing MSI's service does at startup that arms the event channel.

.DESCRIPTION
  Measured on 2026-10-02: after a boot in which 'Micro Star SCM' never ran, a
  subscription receives nothing. Once the service has run, events arrive and keep
  arriving after it is stopped. So arming happens once, at service startup.

  This script captures that moment. It snapshots every MSI_* instance while the
  channel is still disarmed, turns on WMI activity tracing, runs the service for
  a few seconds, stops it, and then shows both what changed in the classes and
  what WMI operations the service performed.

  Snapshots are keyed by InstanceName, never by position: enumeration order is
  not guaranteed, and an earlier diff by position produced a false candidate.

  Leaves the service Disabled and tracing off. Needs an elevated session.
#>
[CmdletBinding()]
param(
    [int]    $RunSeconds  = 8,
    [string] $OutDir      = (Join-Path $PSScriptRoot 'snapshots'),
    [string] $ServiceName = 'Micro Star SCM',
    [string] $TraceLog    = 'Microsoft-Windows-WMI-Activity/Trace'
)

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
         ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Needs an elevated session."
    exit 1
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

# Keyed by InstanceName, so a diff can never compare two different bytes.
function Get-MsiSnapshot {
    $snapshot = [ordered]@{}
    $classes = Get-CimClass -Namespace root/WMI -ClassName MSI_* |
               Select-Object -ExpandProperty CimClassName | Sort-Object

    foreach ($class in $classes) {
        try { $instances = @(Get-CimInstance -Namespace root/WMI -ClassName $class) }
        catch { $snapshot["$class"] = "ERROR: $($_.Exception.Message)"; continue }

        foreach ($instance in $instances) {
            foreach ($p in $instance.CimInstanceProperties) {
                if ($p.Name -eq 'InstanceName') { continue }
                $value = $p.Value
                if ($value -is [byte[]]) {
                    $value = ($value | ForEach-Object { '{0:X2}' -f $_ }) -join ' '
                }
                $snapshot["$class/$($instance.InstanceName)/$($p.Name)"] = [string]$value
            }
        }
    }
    $snapshot
}

function Compare-Snapshot {
    param($Before, $After)
    $keys = ($Before.Keys + $After.Keys) | Sort-Object -Unique
    foreach ($k in $keys) {
        $l = if ($Before.Contains($k)) { $Before[$k] } else { '<absent>' }
        $r = if ($After.Contains($k))  { $After[$k]  } else { '<absent>' }
        if ($l -ne $r) {
            [pscustomobject]@{ Property = $k; Disarmed = $l; Armed = $r }
        }
    }
}

Write-Host ""
Write-Host "[1/5] snapshot while the channel is still DISARMED" -ForegroundColor Cyan
Write-Host "      service: $((Get-Service $ServiceName).Status) / $((Get-Service $ServiceName).StartType)"
$disarmed = Get-MsiSnapshot
$disarmed | ConvertTo-Json -Depth 3 | Set-Content (Join-Path $OutDir 'msi-state-disarmed.json') -Encoding utf8
Write-Host "      $($disarmed.Count) properties recorded"

$tracingOn = $false
try {
    Write-Host "[2/5] turning on WMI activity tracing" -ForegroundColor Cyan
    try {
        wevtutil set-log $TraceLog /enabled:true /quiet 2>$null
        wevtutil clear-log $TraceLog 2>$null
        $tracingOn = $true
        Write-Host "      '$TraceLog' enabled and cleared"
    } catch {
        Write-Warning "could not enable '$TraceLog': $($_.Exception.Message)"
        Write-Warning "continuing without a trace; the snapshot diff still works"
    }

    Write-Host "[3/5] running '$ServiceName' for $RunSeconds s" -ForegroundColor Cyan
    Set-Service $ServiceName -StartupType Manual
    Start-Service $ServiceName
    Start-Sleep -Seconds $RunSeconds

    Write-Host "[4/5] snapshot while ARMED, then stopping the service" -ForegroundColor Cyan
    $armed = Get-MsiSnapshot
    $armed | ConvertTo-Json -Depth 3 | Set-Content (Join-Path $OutDir 'msi-state-armed.json') -Encoding utf8
}
finally {
    Write-Host "[5/5] restoring: service back to Disabled, tracing off" -ForegroundColor Yellow
    try { Stop-Service $ServiceName -Force -ErrorAction SilentlyContinue } catch {}
    try {
        Set-Service $ServiceName -StartupType Disabled
        Write-Host "      service: $((Get-Service $ServiceName).Status) / $((Get-Service $ServiceName).StartType)" -ForegroundColor Green
    } catch { Write-Warning "could not disable the service: $($_.Exception.Message)" }
}

Write-Host ""
Write-Host "=== what changed in MSI_* (keyed by InstanceName) ===" -ForegroundColor Cyan
$changes = Compare-Snapshot $disarmed $armed
if ($changes) { $changes | Format-Table -AutoSize } else { Write-Host "  nothing changed" }

if ($tracingOn) {
    Write-Host ""
    Write-Host "=== WMI operations mentioning MSI or ACPI ===" -ForegroundColor Cyan
    $dump = Join-Path $OutDir 'wmi-trace.txt'
    try {
        $events = Get-WinEvent -LogName $TraceLog -ErrorAction Stop |
                  Where-Object { $_.Message -match 'MSI_|PNP0C14|MSIService' }
        if ($events) {
            $events | ForEach-Object { "$($_.TimeCreated)  $($_.Id)  $($_.Message)" } |
                Set-Content $dump -Encoding utf8
            $events | Select-Object -First 25 |
                ForEach-Object { '  {0}' -f ($_.Message -replace '\s+', ' ') }
            Write-Host "  full dump: $dump"
        } else {
            Write-Host "  no matching operations recorded"
        }
    } catch {
        Write-Warning "could not read '$TraceLog': $($_.Exception.Message)"
    }
    try { wevtutil set-log $TraceLog /enabled:false /quiet 2>$null } catch {}
}
