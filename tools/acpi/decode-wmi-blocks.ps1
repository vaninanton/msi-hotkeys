<#
.SYNOPSIS
  Lists which EC fields each MSI WMI block hands out, in order.

.DESCRIPTION
  _WDG maps each block GUID to a two-character object id, and the firmware then
  implements WQ<id> to read the block and WS<id> to write it. Those methods name
  the EC fields they touch, so reading the names out of the method bodies gives
  the byte map of every window — from the firmware itself rather than from
  guesswork about observed values.

  This is not a disassembler. It finds the method, then walks its body collecting
  four-character names that match the EC field table decode-ec-fields.ps1
  produced, which is enough to see what a window is made of and in what order.

.EXAMPLE
  .\decode-wmi-blocks.ps1
#>
[CmdletBinding()]
param(
    [string] $AmlPath    = (Join-Path $PSScriptRoot 'dsdt.aml'),
    [string] $FieldsPath = (Join-Path $PSScriptRoot 'ec-fields.csv')
)

$ErrorActionPreference = 'Stop'

$aml = [IO.File]::ReadAllBytes($AmlPath)
$fields = @{}
foreach ($row in Import-Csv $FieldsPath) {
    $fields[$row.Name] = $row
}

# Object ids from _WDG, with the instance counts we read over WMI.
$blocks = [ordered]@{
    "AA" = @("MSI_Software", 34)
    'AB' = 'MSI_Device (4)'
    'AC' = 'MSI_Power (3)'
    'AD' = 'MSI_Master_Battery (16)'
    'AE' = 'MSI_Slave_Battery (14)'
    'AF' = 'MSI_CPU (19)'
    'AG' = 'MSI_VGA (18)'
    'AH' = 'MSI_System (21)'
    'AI' = 'MSI_AP (8)'
}

function Find-Name {
    param([byte[]] $Bytes, [string] $Name, [int] $From = 0)

    $needle = [byte[]][char[]]$Name
    for ($i = $From; $i -lt $Bytes.Length - $needle.Length; $i++) {
        $ok = $true
        for ($j = 0; $j -lt $needle.Length; $j++) {
            if ($Bytes[$i + $j] -ne $needle[$j]) { $ok = $false; break }
        }
        if ($ok) { return $i }
    }
    -1
}

function Read-NameSeg {
    param([byte[]] $Bytes, [int] $At)
    -join (0..3 | ForEach-Object { [char]$Bytes[$At + $_] })
}

Write-Host ""
Write-Host "=== what each WMI block is made of ===" -ForegroundColor Cyan

foreach ($id in $blocks.Keys) {
    foreach ($verb in 'WQ', 'WS') {
        $method = "$verb$id"
        $at = Find-Name $aml $method
        if ($at -lt 0) { continue }

        # The method body: walk forward and collect names that are EC fields.
        # 600 bytes is well past the longest of these methods.
        $seen = [System.Collections.ArrayList]::new()
        for ($i = $at + 4; $i -lt [Math]::Min($at + 600, $aml.Length - 4); $i++) {
            $name = Read-NameSeg $aml $i
            if ($name -notmatch '^[A-Z0-9_]{4}$') { continue }
            if (-not $fields.ContainsKey($name)) { continue }
            if ($seen -contains $name) { continue }
            [void]$seen.Add($name)
        }

        if ($seen.Count) {
            Write-Host ""
            Write-Host ("{0}  {1}  ({2})" -f $method, $blocks[$id], $seen.Count) -ForegroundColor Yellow
            $line = $seen | ForEach-Object {
                $f = $fields[$_]
                if ([int]$f.Width -ge 8) { "{0}@{1}" -f $_, $f.Byte }
                else { "{0}@{1}.{2}" -f $_, $f.Byte, $f.Bit }
            }
            ($line -join '  ') -split '(.{1,110}\s)' | Where-Object { $_ } | ForEach-Object { "  $_" }
        }
    }
}

Write-Host ""
Write-Host "Names are the firmware's own; @ gives the EC address." -ForegroundColor DarkGray
