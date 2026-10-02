<#
.SYNOPSIS
  Draws the program icon into assets/msi-hotkeys.ico.

.DESCRIPTION
  The same disc the tray draws in code — a rim with a dark hub — rendered at the
  four sizes Windows asks for and packed into one .ico with PNG payloads, which
  Windows has understood since Vista.

  Kept as a script rather than a committed-and-forgotten binary: when the tray
  colours change, the file icon can be regenerated to match.

.EXAMPLE
  .\make-icon.ps1
#>
[CmdletBinding()]
param(
    [string] $OutPath = (Join-Path (Split-Path $PSScriptRoot -Parent) 'assets\msi-hotkeys.ico'),
    [int[]]  $Sizes   = @(16, 32, 48, 256)
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$rim = [System.Drawing.Color]::FromArgb(255, 86, 184, 214)
$hub = [System.Drawing.Color]::FromArgb(255, 24, 28, 34)

function New-Disc {
    param([int] $Size)

    $bitmap = [System.Drawing.Bitmap]::new($Size, $Size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.Clear([System.Drawing.Color]::Transparent)

        $rimBrush = [System.Drawing.SolidBrush]::new($rim)
        $hubBrush = [System.Drawing.SolidBrush]::new($hub)
        try {
            $g.FillEllipse($rimBrush, 0, 0, $Size - 1, $Size - 1)

            # The hub is 30% of the radius, as in tray::icon.
            $inset = [Math]::Round($Size * 0.35)
            $g.FillEllipse($hubBrush, $inset, $inset, $Size - 1 - 2 * $inset, $Size - 1 - 2 * $inset)
        }
        finally {
            $rimBrush.Dispose()
            $hubBrush.Dispose()
        }
    }
    finally {
        $g.Dispose()
    }

    $stream = [System.IO.MemoryStream]::new()
    $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    , $stream.ToArray()
}

$images = foreach ($size in $Sizes) {
    [pscustomobject]@{ Size = $size; Png = (New-Disc -Size $size) }
}

# ICONDIR, then one 16-byte ICONDIRENTRY per image, then the PNG payloads.
$out = [System.IO.MemoryStream]::new()
$w = [System.IO.BinaryWriter]::new($out)

$w.Write([uint16]0)                 # reserved
$w.Write([uint16]1)                 # type: icon
$w.Write([uint16]$images.Count)

$offset = 6 + 16 * $images.Count
foreach ($image in $images) {
    # 256 is written as 0: the field is one byte wide.
    $dimension = if ($image.Size -ge 256) { 0 } else { $image.Size }
    $w.Write([byte]$dimension)      # width
    $w.Write([byte]$dimension)      # height
    $w.Write([byte]0)               # palette size
    $w.Write([byte]0)               # reserved
    $w.Write([uint16]1)             # colour planes
    $w.Write([uint16]32)            # bits per pixel
    $w.Write([uint32]$image.Png.Length)
    $w.Write([uint32]$offset)
    $offset += $image.Png.Length
}
foreach ($image in $images) {
    $w.Write($image.Png)
}

$w.Flush()
$bytes = $out.ToArray()
$w.Dispose()
$out.Dispose()

$dir = Split-Path $OutPath -Parent
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
[System.IO.File]::WriteAllBytes($OutPath, $bytes)

Write-Host ("{0}: {1} bytes, sizes {2}" -f $OutPath, $bytes.Length, ($Sizes -join ', ')) -ForegroundColor Green
