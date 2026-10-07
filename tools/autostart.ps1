<#
.SYNOPSIS
  Registers the handler to start at logon, or removes that registration.

.DESCRIPTION
  The handler needs administrator rights, which rules out the usual autostart
  spots: a Run key or a Startup shortcut would make Windows ask for elevation at
  every logon. A scheduled task with the highest privileges is the one way to
  start an elevated program without a prompt.

  Three of the settings below are not decoration, and on a laptop they decide
  whether this works at all:

    RunLevel Highest            no UAC prompt at logon, which is the whole point
    AllowStartIfOnBatteries     without it the task will not start on battery
    DontStopIfGoingOnBatteries  without it the task is killed when the mains go
    ExecutionTimeLimit 0        without it the scheduler stops the task after
                                three days, and a tray program simply vanishes

  The program guards against a second instance, so a manual launch alongside the
  task is harmless.

.PARAMETER ExePath
  Which binary to start. Defaults to the release build in this working copy.

.PARAMETER Remove
  Unregisters the task instead of creating it.

.EXAMPLE
  .\autostart.ps1
  Registers the task for the current user, from an elevated prompt.

.EXAMPLE
  .\autostart.ps1 -ExePath 'C:\Tools\msi-hotkeys.exe'

.EXAMPLE
  .\autostart.ps1 -Remove
#>
[CmdletBinding()]
param(
    [string] $ExePath,
    [string] $TaskName = 'msi-hotkeys',
    [switch] $Remove
)

$ErrorActionPreference = 'Stop'

# Resolved here rather than as a parameter default: Windows PowerShell leaves
# $PSScriptRoot empty in default-value expressions when the script is started
# with -File, and the failure is an unhelpful one about an empty Path.
if (-not $ExePath) {
    $root = if ($PSScriptRoot) { Split-Path $PSScriptRoot -Parent } else { (Get-Location).Path }
    $ExePath = Join-Path $root 'target\release\msi-hotkeys.exe'
}

$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $elevated) {
    Write-Error "Run this from an elevated prompt: registering a task that runs with the highest privileges needs administrator rights."
    return
}

if ($Remove) {
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Host "Removed the task '$TaskName'. The handler will not start at logon any more." -ForegroundColor Green
        Write-Host "It keeps running until it is closed from the notification area." -ForegroundColor DarkGray
    }
    else {
        Write-Host "No task named '$TaskName'; nothing to remove." -ForegroundColor DarkGray
    }
    return
}

if (-not (Test-Path $ExePath)) {
    Write-Error "No binary at $ExePath. Build it first with ``cargo build --release``, or pass -ExePath."
    return
}
$ExePath = (Resolve-Path $ExePath).Path

# A task pointing into target\ breaks the moment `cargo clean` runs, and it
# breaks silently: the program just stops appearing at logon.
if ($ExePath -match '\\target\\(debug|release)\\') {
    Write-Warning "This points into the build directory, so ``cargo clean`` would break the task."
    Write-Warning "For something lasting, copy the exe somewhere stable and pass -ExePath."
}

$action = New-ScheduledTaskAction -Execute $ExePath
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit 0 `
    -MultipleInstances IgnoreNew `
    -StartWhenAvailable

Register-ScheduledTask -TaskName $TaskName `
    -Description 'Handler for the MSI hardware buttons. Needs elevation, so it runs as a task rather than from a Run key.' `
    -Action $action -Trigger $trigger -Principal $principal -Settings $settings `
    -Force | Out-Null

# Read it back rather than trusting the call: a task that was not created the way
# it was asked for fails at the next logon, which is a bad place to find out.
$task = Get-ScheduledTask -TaskName $TaskName
$info = @(
    "task:      $($task.TaskName)  ($($task.State))"
    "runs:      $($task.Actions[0].Execute)"
    "as:        $($task.Principal.UserId), RunLevel $($task.Principal.RunLevel)"
    "trigger:   $($task.Triggers[0].CimClass.CimClassName)"
    "battery:   starts on battery = $(-not $task.Settings.DisallowStartIfOnBatteries), stops on battery = $($task.Settings.StopIfGoingOnBatteries)"
    "time cap:  $(if ($task.Settings.ExecutionTimeLimit -in @('PT0S', '')) { 'none' } else { $task.Settings.ExecutionTimeLimit })"
)
Write-Host ""
Write-Host "Registered:" -ForegroundColor Green
$info | ForEach-Object { Write-Host "  $_" }

$wrong = @()
if ($task.Principal.RunLevel -ne 'Highest') { $wrong += 'not running with the highest privileges — expect a UAC prompt at logon' }
if ($task.Settings.DisallowStartIfOnBatteries) { $wrong += 'will not start on battery' }
if ($task.Settings.StopIfGoingOnBatteries) { $wrong += 'will be stopped when the mains go' }
if ($wrong) {
    Write-Host ""
    Write-Warning "Not as intended:"
    $wrong | ForEach-Object { Write-Warning "  $_" }
}

Write-Host ""
Write-Host "Takes effect at the next logon. To start it now:" -ForegroundColor DarkGray
Write-Host "  Start-ScheduledTask -TaskName $TaskName" -ForegroundColor DarkGray
Write-Host "To undo:  .\autostart.ps1 -Remove" -ForegroundColor DarkGray
