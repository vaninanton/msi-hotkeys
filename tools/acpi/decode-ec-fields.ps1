<#
.SYNOPSIS
  Pulls the embedded-controller field map out of this machine's DSDT.

.DESCRIPTION
  The vendor's own ACPI code is the authority on what the EC bytes mean: it
  declares an OperationRegion over EC RAM and then a Field listing every byte
  with a name. Those names are what the firmware author called them, so they
  beat any inference from observed values.

  This reads the DSDT out of the registry (no elevation needed), finds every
  OperationRegion, and decodes the Field declarations over the EmbeddedControl
  ones into a table of name, byte offset and width.

  It decodes only as much AML as that needs: OperationRegion (5B 80), Field
  (5B 81), package lengths and field elements. It is not a disassembler.

.EXAMPLE
  .\decode-ec-fields.ps1
#>
[CmdletBinding()]
param(
    [string] $AmlPath = (Join-Path $PSScriptRoot 'dsdt.aml')
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $AmlPath)) {
    $key = 'HKLM:\HARDWARE\ACPI\DSDT\MSI_NB\MEGABOOK\01072009'
    Write-Host "no $AmlPath, reading the registry instead" -ForegroundColor DarkGray
    [IO.File]::WriteAllBytes($AmlPath, (Get-ItemProperty $key).'00000000')
}

$aml = [IO.File]::ReadAllBytes($AmlPath)
Write-Host "DSDT: $($aml.Length) bytes" -ForegroundColor Cyan

# Package length: the top two bits of the first byte say how many bytes follow,
# and the value is assembled little-endian from the nibbles that remain.
function Read-PkgLength {
    param([byte[]] $Bytes, [int] $At)

    $lead = $Bytes[$At]
    $extra = $lead -shr 6
    if ($extra -eq 0) {
        return @{ Value = ($lead -band 0x3F); Size = 1 }
    }
    $value = $lead -band 0x0F
    for ($i = 1; $i -le $extra; $i++) {
        $value = $value -bor ([int]$Bytes[$At + $i] -shl (4 + 8 * ($i - 1)))
    }
    @{ Value = $value; Size = $extra + 1 }
}

function Test-NameChar {
    param([byte] $B)
    ($B -ge 0x41 -and $B -le 0x5A) -or $B -eq 0x5F -or ($B -ge 0x30 -and $B -le 0x39)
}

function Read-NameSeg {
    param([byte[]] $Bytes, [int] $At)
    -join (0..3 | ForEach-Object { [char]$Bytes[$At + $_] })
}

$spaces = @{
    0 = 'SystemMemory'; 1 = 'SystemIO'; 2 = 'PCI_Config'; 3 = 'EmbeddedControl'
    4 = 'SMBus'; 5 = 'CMOS'; 6 = 'PciBarTarget'; 7 = 'IPMI'
}

# --- OperationRegions ---
$regions = @{}
Write-Host ""
Write-Host "=== OperationRegions ===" -ForegroundColor Cyan
for ($i = 0; $i -lt $aml.Length - 12; $i++) {
    if ($aml[$i] -ne 0x5B -or $aml[$i + 1] -ne 0x80) { continue }

    # NameString here is a plain 4-character segment in every vendor table seen.
    $at = $i + 2
    if (-not (Test-NameChar $aml[$at])) { continue }
    $name = Read-NameSeg $aml $at
    $space = $aml[$at + 4]
    if (-not $spaces.ContainsKey([int]$space)) { continue }

    $regions[$name] = $spaces[[int]$space]
    '  {0}  {1,-16} (0x{2:X5})' -f $name, $spaces[[int]$space], $i | Write-Host
}

# --- Fields over the EC regions ---
$ecRegions = $regions.Keys | Where-Object { $regions[$_] -eq 'EmbeddedControl' }
if (-not $ecRegions) {
    Write-Warning "no EmbeddedControl region found; the EC map is not in this table"
    return
}

Write-Host ""
Write-Host "=== Fields over $($ecRegions -join ', ') ===" -ForegroundColor Cyan

$rows = @()
for ($i = 0; $i -lt $aml.Length - 12; $i++) {
    if ($aml[$i] -ne 0x5B -or $aml[$i + 1] -ne 0x81) { continue }

    $at = $i + 2
    $pkg = Read-PkgLength $aml $at
    $end = $at + $pkg.Value
    $at += $pkg.Size

    if (-not (Test-NameChar $aml[$at])) { continue }
    $region = Read-NameSeg $aml $at
    if ($region -notin $ecRegions) { continue }
    $at += 4
    $at += 1   # field flags

    $bit = 0
    while ($at -lt $end -and $at -lt $aml.Length) {
        $tag = $aml[$at]
        if ($tag -eq 0x00) {
            $at++
            $skip = Read-PkgLength $aml $at
            $at += $skip.Size
            $bit += $skip.Value
        } elseif ($tag -eq 0x01) {
            $at += 3
        } elseif ($tag -eq 0x02) {
            $at += 2
        } elseif (Test-NameChar $tag) {
            $name = Read-NameSeg $aml $at
            $at += 4
            $width = Read-PkgLength $aml $at
            $at += $width.Size
            $rows += [pscustomobject]@{
                Region = $region
                Name   = $name
                Byte   = '0x{0:X2}' -f [int][Math]::Floor($bit / 8)
                Bit    = $bit % 8
                Width  = $width.Value
            }
            $bit += $width.Value
        } else {
            break
        }
    }
}

if (-not $rows) {
    Write-Warning "no field elements decoded"
    return
}

$rows | Format-Table -AutoSize
Write-Host "$($rows.Count) named EC fields" -ForegroundColor Green

$csv = Join-Path $PSScriptRoot 'ec-fields.csv'
$rows | Export-Csv -Path $csv -NoTypeInformation -Encoding utf8
Write-Host "saved: $csv" -ForegroundColor DarkGray
