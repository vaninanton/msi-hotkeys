<#
.SYNOPSIS
  Tests whether the event channel can be armed without MSI's service, by
  writing the one EC byte that tracks it.

.DESCRIPTION
  arming-experiment.ps1 found that MSI_AP instance 0_2 reads 78 (0x4E) while
  'Micro Star SCM' runs and 77 (0x4D) once it has been stopped -- a one-bit
  difference, and MSI_AP.AP carries write=True, so it can be set from here.

  This script stops the service, waits for the channel to close, confirms that
  no events arrive, writes 78 back by hand, and asks for the same button again.
  Events after the write would mean the service is replaceable.

  It restores the original byte and restarts the service in a finally block, so
  an error or Ctrl+C in the middle still puts things back.

  Needs an elevated session, and asks you to press a button twice.

.EXAMPLE
  .\try-self-arming.ps1
#>
[CmdletBinding()]
param(
    [int]    $InstanceIndex  = 2,
    [int]    $ArmedValue     = 78,
    [int]    $SettleSeconds  = 120,
    [int]    $ListenSeconds  = 30,
    [string] $ServiceName    = 'Micro Star SCM'
)

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
         ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Needs an elevated session."
    exit 1
}

$target = "ACPI\PNP0C14\0_$InstanceIndex"

# Counts MSI_Event indications over a window, while the operator presses a key.
function Measure-Events {
    param([string] $Prompt, [int] $Seconds)

    $sid = "SelfArm_$PID"
    Get-EventSubscriber -SourceIdentifier $sid -ErrorAction SilentlyContinue |
        Unregister-Event -ErrorAction SilentlyContinue
    $null = Register-CimIndicationEvent -Namespace 'root/WMI' `
                -ClassName 'MSI_Event' -SourceIdentifier $sid

    Write-Host ""
    Write-Host ">>> $Prompt" -ForegroundColor Yellow
    Write-Host ">>> Listening for $Seconds seconds." -ForegroundColor Yellow

    $seen     = @()
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        $e = Wait-Event -SourceIdentifier $sid -Timeout 2
        if ($e) {
            $code = [uint32]$e.SourceEventArgs.NewEvent.MSIEvt
            $seen += $code
            Write-Host ("    got 0x{0:X6}" -f $code) -ForegroundColor Green
            Remove-Event -EventIdentifier $e.EventIdentifier
        }
    }

    Unregister-Event -SourceIdentifier $sid
    Write-Host ("    {0} event(s) in that window." -f $seen.Count)
    $seen.Count
}

function Get-ApByte {
    param([string] $InstanceName)
    (Get-CimInstance -Namespace root/WMI -ClassName MSI_AP |
        Where-Object InstanceName -eq $InstanceName).AP
}

$original = Get-ApByte $target
Write-Host ""
Write-Host "MSI_AP $target currently reads $original (service is $((Get-Service $ServiceName).Status))." -ForegroundColor Cyan
Write-Host "Original value recorded; it will be written back at the end." -ForegroundColor DarkGray

$armedCount = -1
$deadCount  = -1

try {
    Write-Host ""
    Write-Host "[1/4] stopping '$ServiceName'" -ForegroundColor Cyan
    Stop-Service -Name $ServiceName -Force

    Write-Host "[2/4] waiting $SettleSeconds s for the channel to close" -ForegroundColor Cyan
    Start-Sleep -Seconds $SettleSeconds
    Write-Host "      MSI_AP $target now reads $(Get-ApByte $target)"

    $deadCount = Measure-Events "Press Cooler Boost a few times (expecting NOTHING)." $ListenSeconds

    Write-Host ""
    Write-Host "[3/4] writing $ArmedValue to MSI_AP $target by hand" -ForegroundColor Cyan
    $instance = Get-CimInstance -Namespace root/WMI -ClassName MSI_AP |
                Where-Object InstanceName -eq $target
    Set-CimInstance -InputObject $instance -Property @{ AP = [byte]$ArmedValue }
    Write-Host "      reads back as $(Get-ApByte $target)"

    $armedCount = Measure-Events "Press Cooler Boost again (this is the test)." $ListenSeconds
}
finally {
    Write-Host ""
    Write-Host "[4/4] restoring" -ForegroundColor Yellow
    try {
        $now = Get-ApByte $target
        if ($now -ne $original) {
            $instance = Get-CimInstance -Namespace root/WMI -ClassName MSI_AP |
                        Where-Object InstanceName -eq $target
            Set-CimInstance -InputObject $instance -Property @{ AP = [byte]$original }
            Write-Host "      MSI_AP $target written back to $original (reads $(Get-ApByte $target))"
        } else {
            Write-Host "      MSI_AP $target already $original, nothing to undo"
        }
    } catch {
        Write-Warning "could not restore MSI_AP ${target}: $($_.Exception.Message)"
    }

    try {
        Start-Service -Name $ServiceName
        Write-Host "      '$ServiceName' is $((Get-Service $ServiceName).Status)" -ForegroundColor Green
    } catch {
        Write-Warning "COULD NOT RESTART '$ServiceName': $($_.Exception.Message)"
        Write-Warning "Start it by hand: sc start `"$ServiceName`""
    }
}

Write-Host ""
Write-Host "=== result ===" -ForegroundColor Cyan
Write-Host "  service stopped, no write : $deadCount event(s)"
Write-Host "  service stopped, byte set : $armedCount event(s)"
Write-Host ""

if ($deadCount -eq 0 -and $armedCount -gt 0) {
    Write-Host "The write armed the channel. The service is replaceable." -ForegroundColor Green
} elseif ($deadCount -gt 0) {
    Write-Host "Events arrived BEFORE the write, so the channel had not closed yet." -ForegroundColor Yellow
    Write-Host "Nothing is proven; rerun with a longer -SettleSeconds." -ForegroundColor Yellow
} else {
    Write-Host "The write changed nothing: that byte is not what arms the channel." -ForegroundColor Yellow
    Write-Host "Next leads: MSI_Device bytes, or the vendor HID collection." -ForegroundColor Yellow
}
