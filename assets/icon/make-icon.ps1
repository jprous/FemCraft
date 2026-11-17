# Generates the FemCraft application icon: femcraft.ico (all Windows sizes) and
# femcraft-256.png (window icon). Run from anywhere:
#   powershell -ExecutionPolicy Bypass -File assets\icon\make-icon.ps1
#
# Design (original to FemCraft): a deep navy rounded tile carrying a small finite-element
# mesh - a plate split into 2 x 2 elements, seen in perspective - in mint, with a coral load
# arrow pressing on its top node. Each size is drawn natively (not downscaled); the smallest
# sizes drop the inner mesh lines so they stay crisp.

Add-Type -AssemblyName System.Drawing
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

$navyTop = [System.Drawing.Color]::FromArgb(255, 0x15, 0x40, 0x66)
$navyBottom = [System.Drawing.Color]::FromArgb(255, 0x0A, 0x1A, 0x2E)
$rim = [System.Drawing.Color]::FromArgb(110, 0x5C, 0x9E, 0xD6)
$mint = [System.Drawing.Color]::FromArgb(255, 0x4F, 0xE3, 0xC1)
$mintFill = [System.Drawing.Color]::FromArgb(48, 0x4F, 0xE3, 0xC1)
$coral = [System.Drawing.Color]::FromArgb(255, 0xFF, 0x7A, 0x59)

function Draw-Icon([int]$size) {
    $s = $size / 256.0
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)

    # Rounded tile with a vertical gradient and a faint rim.
    $m = [Math]::Max(0.5, 8 * $s); $w = $size - 2 * $m; $r = 46 * $s; $d = 2 * $r
    $tile = New-Object System.Drawing.Drawing2D.GraphicsPath
    $tile.AddArc($m, $m, $d, $d, 180, 90)
    $tile.AddArc($m + $w - $d, $m, $d, $d, 270, 90)
    $tile.AddArc($m + $w - $d, $m + $w - $d, $d, $d, 0, 90)
    $tile.AddArc($m, $m + $w - $d, $d, $d, 90, 90)
    $tile.CloseFigure()
    $grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush (New-Object System.Drawing.PointF 0, $m), (New-Object System.Drawing.PointF 0, ($m + $w)), $navyTop, $navyBottom
    $g.FillPath($grad, $tile)
    $rimPen = New-Object System.Drawing.Pen $rim, ([Math]::Max(1.0, 3 * $s))
    $g.DrawPath($rimPen, $tile)

    # Plate P(u, v) = O + u U + v V (256-unit design space), u, v in [0, 1].
    # (PowerShell names are case-insensitive: keep these distinct from the u, v parameters.)
    $Org = @(44, 163); $DirU = @(84, 44); $DirV = @(84, -44)
    function P([double]$a, [double]$b) {
        New-Object System.Drawing.PointF (($Org[0] + $a * $DirU[0] + $b * $DirV[0]) * $s), (($Org[1] + $a * $DirU[1] + $b * $DirV[1]) * $s)
    }
    $corners = [System.Drawing.PointF[]]@((P 0 0), (P 1 0), (P 1 1), (P 0 1))
    $g.FillPolygon((New-Object System.Drawing.SolidBrush $mintFill), $corners)

    $small = $size -le 24
    $line = New-Object System.Drawing.Pen $mint, ([Math]::Max(1.25, 9 * $s))
    $line.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $g.DrawPolygon($line, $corners)
    if (-not $small) {
        $g.DrawLine($line, (P 0.5 0), (P 0.5 1))
        $g.DrawLine($line, (P 0 0.5), (P 1 0.5))
    }

    # Nodes; the loaded (top) node is coral.
    $steps = if ($small) { @(0, 1) } else { @(0, 0.5, 1) }
    $nr = [Math]::Max(1.4, 13 * $s)
    foreach ($su in $steps) {
        foreach ($sv in $steps) {
            $p = P $su $sv
            $brush = if ($su -eq 0 -and $sv -eq 1) { New-Object System.Drawing.SolidBrush $coral } else { New-Object System.Drawing.SolidBrush $mint }
            $g.FillEllipse($brush, $p.X - $nr, $p.Y - $nr, 2 * $nr, 2 * $nr)
        }
    }

    # Coral load arrow onto the top node (128, 119).
    $arrowPen = New-Object System.Drawing.Pen $coral, ([Math]::Max(1.5, 12 * $s))
    $arrowPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $g.DrawLine($arrowPen, (128 * $s), (40 * $s), (128 * $s), (80 * $s))
    $head = [System.Drawing.PointF[]]@(
        (New-Object System.Drawing.PointF (104 * $s), (76 * $s)),
        (New-Object System.Drawing.PointF (152 * $s), (76 * $s)),
        (New-Object System.Drawing.PointF (128 * $s), (104 * $s)))
    $g.FillPolygon((New-Object System.Drawing.SolidBrush $coral), $head)

    $g.Dispose()
    return $bmp
}

function DibBytes($bmp) {
    # BITMAPINFOHEADER + 32-bit BGRA rows bottom-up + 1-bit AND mask (all zero: alpha is used).
    $w = $bmp.Width; $h = $bmp.Height
    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter $ms
    $bw.Write([UInt32]40); $bw.Write([Int32]$w); $bw.Write([Int32]($h * 2))
    $bw.Write([UInt16]1); $bw.Write([UInt16]32); $bw.Write([UInt32]0)
    $bw.Write([UInt32]($w * $h * 4)); $bw.Write([Int32]0); $bw.Write([Int32]0)
    $bw.Write([UInt32]0); $bw.Write([UInt32]0)
    for ($y = $h - 1; $y -ge 0; $y--) {
        for ($x = 0; $x -lt $w; $x++) {
            $c = $bmp.GetPixel($x, $y)
            $bw.Write([byte]$c.B); $bw.Write([byte]$c.G); $bw.Write([byte]$c.R); $bw.Write([byte]$c.A)
        }
    }
    $maskRow = [Math]::Ceiling($w / 32) * 4
    $bw.Write((New-Object byte[] ($maskRow * $h)))
    $bw.Flush()
    return , $ms.ToArray()
}

function PngBytes($bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $ms.ToArray()
}

# Multi-size .ico: classic bitmap entries below 256 px, PNG at 256 px.
$sizes = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
$images = @()
foreach ($sz in $sizes) {
    $bmp = Draw-Icon $sz
    if ($sz -ge 256) { $images += , (PngBytes $bmp) } else { $images += , (DibBytes $bmp) }
    if ($sz -eq 256) { $bmp.Save((Join-Path $here 'femcraft-256.png'), [System.Drawing.Imaging.ImageFormat]::Png) }
    $bmp.Dispose()
}
$out = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter $out
$bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
    $bw.Write([byte]$dim); $bw.Write([byte]$dim); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([UInt16]1); $bw.Write([UInt16]32)
    $bw.Write([UInt32]$images[$i].Length); $bw.Write([UInt32]$offset)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $bw.Write($img) }
$bw.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $here 'femcraft.ico'), $out.ToArray())
"wrote femcraft.ico ($($sizes -join ', ') px) and femcraft-256.png"
