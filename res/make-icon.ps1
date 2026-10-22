# Draws res/app.ico, the icon embedded in the exe: a blue scan frame, four corners and a line
# across. The shapes are those the window draws at run time (app_icon in src/ui/paint.rs), drawn
# here with the same GDI+. Each size is stored as a PNG. Run from the repository root:
#
#   powershell -ExecutionPolicy Bypass -File res\make-icon.ps1

Add-Type -AssemblyName System.Drawing

$sizes = 16, 20, 24, 32, 40, 48, 64, 96, 128, 256
$blue = [System.Drawing.Color]::FromArgb(255, 0x0B, 0x6B, 0xDE)

function Draw-Icon([int] $n) {
    $bitmap = New-Object System.Drawing.Bitmap $n, $n, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bitmap)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
    $pen = New-Object System.Drawing.Pen $blue, ([Math]::Max($n * 0.11, 1.5))
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    function P([double] $x, [double] $y) { New-Object System.Drawing.PointF ([single]($x * $n)), ([single]($y * $n)) }
    $a = 0.12; $b = 0.88; $l = 0.30
    $corners = @(
        @((P $a ($a + $l)), (P $a $a), (P ($a + $l) $a)),
        @((P ($b - $l) $a), (P $b $a), (P $b ($a + $l))),
        @((P $b ($b - $l)), (P $b $b), (P ($b - $l) $b)),
        @((P ($a + $l) $b), (P $a $b), (P $a ($b - $l)))
    )
    foreach ($corner in $corners) {
        $g.DrawLines($pen, [System.Drawing.PointF[]] $corner)
    }
    $g.DrawLine($pen, (P 0.30 0.5), (P 0.70 0.5))
    $pen.Dispose()
    $g.Dispose()
    $stream = New-Object System.IO.MemoryStream
    $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    , $stream.ToArray()
}

$images = foreach ($n in $sizes) { , (Draw-Icon $n) }

# An .ico file: a 6-byte header, one 16-byte entry per image, then the images.
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16] 0)               # reserved
$w.Write([uint16] 1)               # type: icon
$w.Write([uint16] $sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $n = $sizes[$i]
    $side = if ($n -ge 256) { 0 } else { $n }   # 0 means 256
    $w.Write([byte] $side)          # width
    $w.Write([byte] $side)          # height
    $w.Write([byte] 0)              # palette size
    $w.Write([byte] 0)              # reserved
    $w.Write([uint16] 1)            # colour planes
    $w.Write([uint16] 32)           # bits per pixel
    $w.Write([uint32] $images[$i].Length)
    $w.Write([uint32] $offset)
    $offset += $images[$i].Length
}
foreach ($image in $images) { $w.Write([byte[]] $image) }
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $PSScriptRoot 'app.ico'), $out.ToArray())
"Wrote res\app.ico: $($sizes.Count) sizes, $($out.Length) bytes"
