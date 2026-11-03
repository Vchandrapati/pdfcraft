# Print a print-ready PDF on Windows (CUPS-independent backend). The job arrives in CRAFT_* environment
# variables, never as arguments: printer names contain spaces and quotes, and the document's
# path must not be visible in the process list while it prints.
#
# Sheets print as vectors: the in-box Windows.Data.Pdf draws each one through Direct2D into an
# XPS print job (windows_vector.cs), with copies, collation, duplex and colour in the job's print
# ticket, the way `lp -o` sends them on CUPS. Where that can't start (no Direct2D, no print
# ticket), and with CRAFT_PRINT_RASTER=1, Windows.Data.Pdf renders each sheet at the printer's
# own resolution and System.Drawing.Printing spools the image with the driver's settings. No PDF
# reader is needed, and the calling code touches no Win32 API.
#
# CRAFT_PRINT_DRYRUN=1 renders every sheet in memory, both ways, and prints nothing: the tests
# run the whole rendering path on machines without a printer, CI included.
# CRAFT_PRINT_OUTPUT_FILE (tests) sends a vector job's output to that file instead of the port.
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

Add-Type -AssemblyName System.Drawing
# Windows duplex values are binding edges: Vertical = long edge, Horizontal = short edge.
# https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-devmodea
function Get-Duplex($edge) {
    switch ($edge) {
        'long-edge' { [System.Drawing.Printing.Duplex]::Vertical }
        'short-edge' { [System.Drawing.Printing.Duplex]::Horizontal }
        default { [System.Drawing.Printing.Duplex]::Simplex }
    }
}

function Set-JobDuplex($settings, $edge) {
    if ($edge) { $settings.Duplex = Get-Duplex $edge }
}

# WinRT dimensions are DIPs (96/inch), printing dimensions are hundredths of an inch.
# The PDF is already composed: preserve physical size, even if the printer clips its edges.
function Get-SheetGeometry($width, $height) {
    if ([double]::IsNaN($width) -or [double]::IsNaN($height) -or
        [double]::IsInfinity($width) -or [double]::IsInfinity($height) -or
        $width -le 0 -or $height -le 0 -or $width -gt 12288 -or $height -gt 12288) {
        throw 'the PDF sheet dimensions are invalid or exceed 128 inches'
    }
    # DEVMODE paper lengths use signed 16-bit tenths of a millimetre (128 inches fits).
    $drawW = $width * 100.0 / 96.0
    $drawH = $height * 100.0 / 96.0
    [pscustomobject]@{
        Width = $drawW; Height = $drawH; Landscape = ($width -gt $height)
        PaperWidth = [int][Math]::Max(1, [Math]::Round([Math]::Min($drawW, $drawH)))
        PaperHeight = [int][Math]::Max(1, [Math]::Round([Math]::Max($drawW, $drawH)))
    }
}

function Set-SheetSettings($settings, $papers, $sheet, $scaling = 'exact', $fallbackPaper = $null) {
    # Prefer the driver's paper identifier over a custom form when the dimensions match.
    $paper = $papers | Where-Object {
        [Math]::Abs($_.Width - $sheet.PaperWidth) -le 1 -and
        [Math]::Abs($_.Height - $sheet.PaperHeight) -le 1
    } | Select-Object -First 1
    if (-not $paper -and $scaling -eq 'shrink-to-fit') {
        if (-not $fallbackPaper) { throw 'the printer has no default paper for shrink-to-fit' }
        $paper = $fallbackPaper
    }
    if (-not $paper) {
        $paper = [System.Drawing.Printing.PaperSize]::new('PDF sheet', $sheet.PaperWidth, $sheet.PaperHeight)
    }
    $settings.PaperSize = $paper
    $settings.Landscape = $sheet.Landscape
    $settings.Margins = [System.Drawing.Printing.Margins]::new(0, 0, 0, 0)
}

function Get-DrawBox($sheet, $hardMarginX, $hardMarginY) {
    # GDI's origin is the printable area's corner. Offset to the physical paper's corner.
    [System.Drawing.RectangleF]::new(-$hardMarginX, -$hardMarginY, $sheet.Width, $sheet.Height)
}

function Get-ShrinkDrawBox($sheet, $area, $hardMarginX, $hardMarginY) {
    foreach ($value in @($area.X, $area.Y, $area.Width, $area.Height, $hardMarginX, $hardMarginY)) {
        if ([double]::IsNaN($value) -or [double]::IsInfinity($value)) {
            throw 'the printer reported an invalid printable area'
        }
    }
    if ($area.Width -le 0 -or $area.Height -le 0) { throw 'the printer has no printable area' }
    $scale = [Math]::Min(1.0, [Math]::Min($area.Width / $sheet.Width, $area.Height / $sheet.Height))
    $width = $sheet.Width * $scale
    $height = $sheet.Height * $scale
    [System.Drawing.RectangleF]::new(
        $area.X + ($area.Width - $width) / 2.0 - $hardMarginX,
        $area.Y + ($area.Height - $height) / 2.0 - $hardMarginY,
        $width, $height)
}

function Get-RenderSize($sheet, $dpi) {
    $dpi = [Math]::Min(600, [Math]::Max(72, $dpi))
    $w = [Math]::Ceiling($sheet.Width / 100.0 * $dpi)
    $h = [Math]::Ceiling($sheet.Height / 100.0 * $dpi)
    # Bound allocation without changing the physical draw rectangle.
    $scale = [Math]::Min(1.0, [Math]::Min(32768.0 / [Math]::Max($w, $h), [Math]::Sqrt(64000000.0 / ($w * $h))))
    [pscustomobject]@{ Width = [int][Math]::Max(1, [Math]::Floor($w * $scale)); Height = [int][Math]::Max(1, [Math]::Floor($h * $scale)) }
}

# Exercise the production settings/placement helpers without an installed printer. The
# caller clears this internal test flag for ordinary submissions.
if ($env:CRAFT_PRINT_TEST_SETTINGS -eq '1') {
    $papers = @([System.Drawing.Printing.PaperSize]::new('Driver Letter', 850, 1100))
    $results = foreach ($size in @(@(816, 1056), @(1056, 816), @(793.92, 1122.24), @(816, 1344))) {
        $sheet = Get-SheetGeometry $size[0] $size[1]
        $settings = [System.Drawing.Printing.PageSettings]::new()
        Set-SheetSettings $settings $papers $sheet
        $box = Get-DrawBox $sheet 25 30
        $pixels = Get-RenderSize $sheet 150
        [pscustomobject]@{ Width = $settings.PaperSize.Width; Height = $settings.PaperSize.Height;
            Name = $settings.PaperSize.PaperName; Landscape = $settings.Landscape;
            Margins = @($settings.Margins.Left, $settings.Margins.Right, $settings.Margins.Top, $settings.Margins.Bottom);
            X = $box.X; Y = $box.Y; DrawWidth = $box.Width; DrawHeight = $box.Height;
            Pixels = @($pixels.Width, $pixels.Height) }
    }
    $large = Get-RenderSize (Get-SheetGeometry 12288 12288) 600
    $invalidRejected = $false
    try { $null = Get-SheetGeometry ([double]::NaN) 100 } catch { $invalidRejected = $true }
    $fallback = [System.Drawing.Printing.PageSettings]::new()
    Set-SheetSettings $fallback $papers (Get-SheetGeometry 1536 1920) 'shrink-to-fit' $papers[0]
    $area = [System.Drawing.RectangleF]::new(25, 30, 800, 1040)
    $shrinkBoxes = foreach ($size in @(@(1536, 1920), @(384, 480))) {
        $box = Get-ShrinkDrawBox (Get-SheetGeometry $size[0] $size[1]) $area 25 30
        ,@($box.X, $box.Y, $box.Width, $box.Height)
    }
    $defaultDuplex = [pscustomobject]@{ Duplex = [System.Drawing.Printing.Duplex]::Vertical }
    Set-JobDuplex $defaultDuplex ''
    $preservedDuplex = $defaultDuplex.Duplex.ToString()
    Set-JobDuplex $defaultDuplex 'off'
    [pscustomobject]@{ Sheets = @($results); LongEdge = (Get-Duplex 'long-edge').ToString();
        ShortEdge = (Get-Duplex 'short-edge').ToString(); Off = (Get-Duplex 'off').ToString();
        DefaultDuplex = $preservedDuplex; ForcedSimplex = $defaultDuplex.Duplex.ToString();
        FallbackPaper = $fallback.PaperSize.PaperName; ShrinkBoxes = @($shrinkBoxes);
        LargePixels = @($large.Width, $large.Height); InvalidRejected = $invalidRejected } | ConvertTo-Json -Depth 5 -Compress
    return
}

Add-Type -AssemblyName System.Runtime.WindowsRuntime
try {
    $null = [Windows.Data.Pdf.PdfDocument, Windows.Data.Pdf, ContentType = WindowsRuntime]
} catch {
    throw 'this Windows installation cannot render PDFs: the Windows.Data.Pdf component is missing'
}
$null = [Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime]

# Await the WinRT asynchronous APIs from .NET Framework's PowerShell. AsTask is an extension
# method: found by reflecting over the static class — with `.`, an instance call on the type
# object, since `GetMethods` is not static and `::` would not find it.
$type = [System.WindowsRuntimeSystemExtensions]
$opTask = $type.GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
} | Select-Object -First 1
$actTask = $type.GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncAction'
} | Select-Object -First 1
function AwaitOp($operation, $resultType) {
    $task = $opTask.MakeGenericMethod($resultType).Invoke($null, @($operation))
    $task.Wait()
    $task.Result
}
function AwaitAct($action) {
    $task = $actTask.Invoke($null, @($action))
    $task.Wait()
}

# Vector printing: Windows.Data.Pdf draws each sheet into a Direct2D command list, and Direct2D's
# print control turns the command lists into an XPS print job — paths, text and images stay
# vectors and fonts are embedded; only transparency is rasterized, region by region, at
# CRAFT_RASTER_DPI. This is the Windows print path for PDFs:
# https://learn.microsoft.com/en-us/windows/win32/direct2d/printing-and-command-lists
# The COM calls go through each interface's vtable slot (the order in d2d1.h, d2d1_1.h,
# windows.data.pdf.interop.h and documenttarget.h), so no interop assembly is needed.
# The C# is windows_vector.cs, passed in CRAFT_PRINT_VECTOR_SOURCE (the script itself is passed
# on the command line, which holds 32,767 characters).
$vectorSource = $env:CRAFT_PRINT_VECTOR_SOURCE

# Raster resolution for the parts of a sheet XPS can't express (transparency, blend modes).
$CRAFT_RASTER_DPI = 600

# The print queue for the job's printer (or the default), through System.Printing.
function Get-JobQueue($name) {
    $server = [System.Printing.LocalPrintServer]::new()
    if (-not $name) { return $server.DefaultPrintQueue }
    $kinds = [System.Printing.EnumeratedPrintQueueTypes[]]@('Local', 'Connections')
    $queue = $server.GetPrintQueues($kinds) | Where-Object { $_.FullName -eq $name -or $_.Name -eq $name } | Select-Object -First 1
    if (-not $queue) { throw ('the printer is not available: ' + $name) }
    $queue
}

# The job's print ticket: the user's defaults for the printer plus this job's copies, collation,
# duplex and colour, validated by the driver.
function Get-JobTicket($queue) {
    $base = $queue.UserPrintTicket
    if (-not $base) { $base = $queue.DefaultPrintTicket }
    $delta = [System.Printing.PrintTicket]::new()
    $delta.CopyCount = [Math]::Min(32767, [Math]::Max(1, [int]$env:CRAFT_PRINT_COPIES))
    $delta.Collation = if ($env:CRAFT_PRINT_COLLATE -eq '1') { 'Collated' } else { 'Uncollated' }
    switch ($env:CRAFT_PRINT_DUPLEX) {
        'long-edge' { $delta.Duplexing = 'TwoSidedLongEdge' }
        'short-edge' { $delta.Duplexing = 'TwoSidedShortEdge' }
        'off' { $delta.Duplexing = 'OneSided' }
    }
    if ($env:CRAFT_PRINT_GRAYSCALE -eq '1') { $delta.OutputColor = 'Grayscale' }
    $queue.MergeAndValidatePrintTicket($base, $delta).ValidatedPrintTicket
}

# One sheet's placement: the paper (a print ticket), the page size handed to Direct2D, and the
# scale and offset of the sheet on it. DIPs throughout (96/inch), as System.Printing uses.
function Get-VectorSheet($queue, $jobTicket, $media, $width, $height) {
    $null = Get-SheetGeometry $width $height
    $short = [Math]::Min($width, $height)
    $long = [Math]::Max($width, $height)
    $landscape = $width -gt $height
    $paper = $media | Where-Object {
        $_.Width -and $_.Height -and
        [Math]::Abs($_.Width - $short) -le 1.5 -and [Math]::Abs($_.Height - $long) -le 1.5
    } | Select-Object -First 1
    $shrink = $env:CRAFT_PRINT_SCALING -eq 'shrink-to-fit'
    if (-not $paper) {
        $paper = if ($shrink) { $jobTicket.PageMediaSize } else { [System.Printing.PageMediaSize]::new($short, $long) }
    }
    $delta = [System.Printing.PrintTicket]::new()
    $delta.PageMediaSize = $paper
    $delta.PageOrientation = if ($landscape) { 'Landscape' } else { 'Portrait' }
    $ticket = $queue.MergeAndValidatePrintTicket($jobTicket, $delta).ValidatedPrintTicket
    $got = $ticket.PageMediaSize
    if (-not $got.Width -or -not $got.Height) { throw 'the printer reported no paper size' }
    $gotShort = [Math]::Min($got.Width, $got.Height)
    $gotLong = [Math]::Max($got.Width, $got.Height)
    if (-not $shrink -and ([Math]::Abs($gotShort - $short) -gt 1.5 -or [Math]::Abs($gotLong - $long) -gt 1.5)) {
        throw 'the printer cannot use the PDF sheet size; select matching paper and try again'
    }
    $pageW = if ($landscape) { $gotLong } else { $gotShort }
    $pageH = if ($landscape) { $gotShort } else { $gotLong }
    $scale = 1.0; $dx = 0.0; $dy = 0.0
    if ($shrink) {
        # Keep clear of the widest hardware margin on every side, whichever way the driver
        # reports the imageable area for this orientation; never enlarge.
        $margin = 0.0
        $area = $queue.GetPrintCapabilities($ticket).PageImageableArea
        if ($area) {
            $margin = [Math]::Max(0.0, [Math]::Max([Math]::Max($area.OriginWidth, $area.OriginHeight),
                [Math]::Max($gotShort - $area.OriginWidth - $area.ExtentWidth, $gotLong - $area.OriginHeight - $area.ExtentHeight)))
        }
        $availW = $pageW - 2 * $margin
        $availH = $pageH - 2 * $margin
        if ($availW -le 0 -or $availH -le 0) { throw 'the printer has no printable area' }
        $scale = [Math]::Min(1.0, [Math]::Min($availW / $width, $availH / $height))
        $dx = ($pageW - $width * $scale) / 2.0
        $dy = ($pageH - $height * $scale) / 2.0
    }
    [pscustomobject]@{ Ticket = $ticket.GetXmlStream().ToArray(); Width = $pageW; Height = $pageH; Scale = $scale; Dx = $dx; Dy = $dy }
}

# Print every sheet as vectors. Returns $null when it printed, or why vector printing couldn't
# start — nothing reached the printer then, and the raster path prints instead. A failure after
# the job started cancels it and is the job's error.
function Invoke-VectorPrint($document) {
    try {
        Add-Type -AssemblyName System.Printing, ReachFramework
        if (-not ('CraftVectorJob' -as [type])) { Add-Type -TypeDefinition $vectorSource -Language CSharp }
        $queue = Get-JobQueue $env:CRAFT_PRINT_PRINTER
        $jobTicket = Get-JobTicket $queue
        $media = @($queue.GetPrintCapabilities($jobTicket).PageMediaSizeCapability)
        $sheets = for ($i = 0; $i -lt $document.PageCount; $i++) {
            $page = $document.GetPage($i)
            try { Get-VectorSheet $queue $jobTicket $media $page.Size.Width $page.Size.Height } finally { $page.Dispose() }
        }
        $output = if ($env:CRAFT_PRINT_OUTPUT_FILE) { $env:CRAFT_PRINT_OUTPUT_FILE } else { $null }
        $job = [CraftVectorJob]::Start($queue.FullName, $env:CRAFT_PRINT_TITLE, $jobTicket.GetXmlStream().ToArray(), $output, $CRAFT_RASTER_DPI)
    } catch {
        # A missing printer, or a sheet size it can't take, is the raster path's answer too.
        if ($_.Exception.Message -like 'the printer cannot use the PDF sheet size*' -or
            $_.Exception.Message -like 'the printer is not available*') { throw }
        $reason = $_.Exception
        while ($reason.InnerException) { $reason = $reason.InnerException }
        return $reason.Message
    }
    try {
        $i = 0
        foreach ($sheet in @($sheets)) {
            $page = $document.GetPage($i)
            try {
                $job.AddSheet($page, $sheet.Width, $sheet.Height, $sheet.Ticket, $sheet.Scale, $sheet.Dx, $sheet.Dy)
            } finally { $page.Dispose() }
            $i++
        }
        $job.Close()
    } finally { $job.Dispose() }
    $null
}

# Render one sheet at its physical size; only raster resolution can be reduced for memory.
function Render-Sheet($page, $sheet, $dpi) {
    $pixels = Get-RenderSize $sheet $dpi
    $options = [Windows.Data.Pdf.PdfPageRenderOptions]::new()
    $options.DestinationWidth = $pixels.Width
    $options.DestinationHeight = $pixels.Height
    $stream = [System.IO.MemoryStream]::new()
    try {
        $random = [System.IO.WindowsRuntimeStreamExtensions]::AsRandomAccessStream($stream)
        AwaitAct ($page.RenderToStreamAsync($random, $options))
        $stream.Position = 0
        $png = [System.Drawing.Image]::FromStream($stream)
        # Windows.Data.Pdf multiplies the destination size by the display's scale factor (2.5x on a
        # 250% display), so the sheet is resampled to exactly the size requested.
        try { return [System.Drawing.Bitmap]::new($png, $pixels.Width, $pixels.Height) } finally { $png.Dispose() }
    } finally { $stream.Dispose() }
}

$source = AwaitOp ([Windows.Storage.StorageFile]::GetFileFromPathAsync($env:CRAFT_PRINT_FILE)) ([Windows.Storage.StorageFile])
$document = AwaitOp ([Windows.Data.Pdf.PdfDocument]::LoadFromFileAsync($source)) ([Windows.Data.Pdf.PdfDocument])
if ($document.PageCount -lt 1) { throw 'the print-ready PDF has no pages' }

if ($env:CRAFT_PRINT_DRYRUN -eq '1') {
    # Render at actual sheet dimensions without touching a printer or leaving PNG files.
    for ($i = 0; $i -lt $document.PageCount; $i++) {
        $page = $document.GetPage($i)
        try {
            $sheet = Get-SheetGeometry $page.Size.Width $page.Size.Height
            $pixels = Get-RenderSize $sheet 150
            $bitmap = Render-Sheet $page $sheet 150
            try {
                if ($bitmap.Width -ne $pixels.Width -or $bitmap.Height -ne $pixels.Height) {
                    throw 'the PDF renderer changed the requested sheet dimensions'
                }
            } finally { $bitmap.Dispose() }
        } finally { $page.Dispose() }
    }
    # The vector path's renderer too: every sheet drawn into a Direct2D command list.
    Add-Type -TypeDefinition $vectorSource -Language CSharp
    $job = [CraftVectorJob]::Start($null, $null, $null, $null, $CRAFT_RASTER_DPI)
    try {
        for ($i = 0; $i -lt $document.PageCount; $i++) {
            $page = $document.GetPage($i)
            try { $job.AddSheet($page, $page.Size.Width, $page.Size.Height, $null, 1.0, 0.0, 0.0) } finally { $page.Dispose() }
        }
    } finally { $job.Dispose() }
    Write-Output ('rendered ' + $document.PageCount + ' sheet(s)')
    return
}

# Vectors unless CRAFT_PRINT_RASTER=1 asks for images, or vector printing can't start here.
$vectorFailure = 'images were requested'
if ($env:CRAFT_PRINT_RASTER -ne '1') {
    $vectorFailure = Invoke-VectorPrint $document
    if (-not $vectorFailure) {
        Write-Output ('spooled ' + $document.PageCount + ' sheet(s) as vector graphics')
        return
    }
    # The tests' output file is for vector jobs: the raster path would prompt for a file name.
    if ($env:CRAFT_PRINT_OUTPUT_FILE) { throw ('vector printing failed: ' + $vectorFailure) }
}

$printer = [System.Drawing.Printing.PrintDocument]::new()
$printer.DocumentName = $env:CRAFT_PRINT_TITLE
$settings = $printer.PrinterSettings
if ($env:CRAFT_PRINT_PRINTER) {
    try {
        $settings.PrinterName = $env:CRAFT_PRINT_PRINTER
    } catch {
        throw ('the printer is not available: ' + $env:CRAFT_PRINT_PRINTER)
    }
}
$settings.Copies = [Math]::Min(32767, [Math]::Max(1, [int]$env:CRAFT_PRINT_COPIES))
$settings.Collate = ($env:CRAFT_PRINT_COLLATE -eq '1')
Set-JobDuplex $settings $env:CRAFT_PRINT_DUPLEX
if ($env:CRAFT_PRINT_GRAYSCALE -eq '1') { $printer.DefaultPageSettings.Color = $false }
$fallbackPaper = $printer.DefaultPageSettings.PaperSize

# QueryPageSettings runs before each PrintPage, allowing mixed sizes/orientations in one job.
# https://learn.microsoft.com/en-us/dotnet/api/system.drawing.printing.printdocument.querypagesettings
$printer.OriginAtMargins = $false
$printer.PrintController = [System.Drawing.Printing.StandardPrintController]::new()
$script:sheets = 0
$printer.add_QueryPageSettings({
    param($sender, $e)
    $page = $document.GetPage($script:sheets)
    try {
        $sheet = Get-SheetGeometry $page.Size.Width $page.Size.Height
        Set-SheetSettings $e.PageSettings $sender.PrinterSettings.PaperSizes $sheet $env:CRAFT_PRINT_SCALING $fallbackPaper
    } finally { $page.Dispose() }
})
$printer.add_PrintPage({
    param($sender, $e)
    $page = $document.GetPage($script:sheets)
    $bitmap = $null
    try {
        $sheet = Get-SheetGeometry $page.Size.Width $page.Size.Height
        if ($env:CRAFT_PRINT_SCALING -ne 'shrink-to-fit' -and
            ([Math]::Abs($e.PageBounds.Width - $sheet.Width) -gt 2 -or
             [Math]::Abs($e.PageBounds.Height - $sheet.Height) -gt 2)) {
            throw 'the printer cannot use the PDF sheet size; select matching paper and try again'
        }
        $bitmap = Render-Sheet $page $sheet $e.Graphics.DpiX
        if ($env:CRAFT_PRINT_SCALING -eq 'shrink-to-fit') {
            $box = Get-ShrinkDrawBox $sheet $e.PageSettings.PrintableArea $e.PageSettings.HardMarginX $e.PageSettings.HardMarginY
        } else {
            $box = Get-DrawBox $sheet $e.PageSettings.HardMarginX $e.PageSettings.HardMarginY
        }
        $e.Graphics.DrawImage($bitmap, $box)
    } finally {
        if ($bitmap) { $bitmap.Dispose() }
        $page.Dispose()
    }
    $script:sheets++
    $e.HasMorePages = ($script:sheets -lt $document.PageCount)
})
try {
    $printer.Print()
    Write-Output ('spooled ' + $document.PageCount + ' sheet(s) as images (' + $vectorFailure + ')')
} finally { $printer.Dispose() }
