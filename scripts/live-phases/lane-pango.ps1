# H89/PANGO: a configured `hotkey-overlay-title` is Pango markup, as niri paints it.
# niri hands the title to Pango's set_markup (src/ui/hotkey_overlay.rs; wiki
# Configuration:-Key-Bindings, section "Custom Hotkey Overlay Titles"), so `<b>` is
# formatting and `&amp;` is one ampersand - and an invalid title is not partially styled:
# the whole original string is painted literally, with one warning at config load.
#
# Only lane-owned daemons are started (their own pipe and config); nothing here changes a
# system setting.  The probe reads the painted sheet through a screenshot: Windows OCR for
# the words and their boxes, then those boxes are sampled in the capture for the colour
# span and for the bold run's heavier strokes.  A missing OCR engine or unreadable capture
# fails closed (never a skipped or passing gate).  Never run outside the live cycle.
& {
    $mpRoot = Join-Path $D 'lane-pango'
    $mpDir = Join-Path $mpRoot ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path $mpDir
    $mpDetails = [ordered]@{ fixture_directory = $mpDir }
    $mpGates = @(
        'h89_markup_title_text_and_entity_render',
        'h89_markup_colour_span_is_painted',
        'h89_markup_bold_run_has_more_ink',
        'h89_invalid_markup_falls_back_to_the_whole_string',
        'h89_invalid_markup_logs_one_warning_at_load',
        'h89_markup_cleanup'
    )
    foreach ($mpGate in $mpGates) {
        $summary[$mpGate] = $false
        $mpDetails[$mpGate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    function MPGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $mpDetails[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
        $mpDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $mpRoot 'evidence.json')
    }
    $mpSavedPipe = $env:WIRI_PIPE
    $mpDaemon = $null
    $mpLog = Join-Path $mpDir 'pango.stderr'
    $mpCapture = Join-Path $mpDir 'pango.png'
    # The fixture: the same words bold and plain, an entity plus a colour span, and one
    # unknown tag (a markup error, so the whole title must paint literally).
    $mpConfig = @'
hotkey-overlay { skip-at-startup; hide-not-bound; }
binds {
    extend-defaults false
    Ctrl+Alt+F9 hotkey-overlay-title="<b>PangoBold</b>" { spawn "notepad.exe"; }
    Ctrl+Alt+F10 hotkey-overlay-title="PangoBold" { spawn "calc.exe"; }
    Ctrl+Alt+F11 hotkey-overlay-title=r##"<span foreground="#ff8800">PangoOrange</span> &amp; more"## { spawn "mspaint.exe"; }
    Ctrl+Alt+F12 hotkey-overlay-title="<blinkish>PangoFallback" { spawn "write.exe"; }
}
'@
    function MPNative() {
        if ('LanePangoNative' -as [type]) { return }
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class LanePangoNative {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    struct RECT { public int Left, Top, Right, Bottom; }
    public static IntPtr Find(uint pid, string className) {
        IntPtr found = IntPtr.Zero;
        EnumWindows(delegate(IntPtr h, IntPtr l) {
            uint owner; GetWindowThreadProcessId(h, out owner);
            if (owner != pid || !IsWindowVisible(h)) return true;
            var name = new StringBuilder(128);
            GetClassNameW(h, name, name.Capacity);
            if (!String.Equals(name.ToString(), className, StringComparison.Ordinal)) return true;
            found = h;
            return false;
        }, IntPtr.Zero);
        return found;
    }
    public static int[] Bounds(IntPtr h) {
        RECT r;
        if (!GetWindowRect(h, out r)) throw new InvalidOperationException("overlay bounds unavailable");
        return new int[] { r.Left, r.Top, r.Right, r.Bottom };
    }
    public static void Close(IntPtr h) { PostMessageW(h, 0x0010, IntPtr.Zero, IntPtr.Zero); }
}
'@ -ErrorAction Stop
    }
    # Windows OCR over a retained screenshot; unavailable OCR throws (fails closed).
    function MPAwaitWinRT($operation, [type]$resultType, [DateTime]$deadline) {
        $method = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
            $_.Name -eq 'AsTask' -and $_.IsGenericMethod -and
            $_.GetGenericArguments().Count -eq 1 -and $_.GetParameters().Count -eq 1 -and
            $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
        } | Select-Object -First 1
        $task = $method.MakeGenericMethod($resultType).Invoke($null, @($operation))
        $remaining = [int][Math]::Floor(($deadline - [DateTime]::UtcNow).TotalMilliseconds)
        if ($remaining -le 0 -or -not $task.Wait($remaining)) { throw 'Lane PANGO OCR operation timed out' }
        return $task.Result
    }
    function MPRead([string]$path, [DateTime]$deadline) {
        $file = MPAwaitWinRT ([Windows.Storage.StorageFile]::GetFileFromPathAsync($path)) ([Windows.Storage.StorageFile]) $deadline
        $stream = MPAwaitWinRT ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream]) $deadline
        try {
            $decoder = MPAwaitWinRT ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder]) $deadline
            $software = MPAwaitWinRT ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap]) $deadline
            try {
                $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromLanguage((New-Object Windows.Globalization.Language 'en-US'))
                if ($null -eq $engine) { throw 'English Windows OCR is unavailable' }
                $result = MPAwaitWinRT ($engine.RecognizeAsync($software)) ([Windows.Media.Ocr.OcrResult]) $deadline
                $words = New-Object System.Collections.Generic.List[object]
                foreach ($line in $result.Lines) {
                    foreach ($word in $line.Words) {
                        $box = $word.BoundingRect
                        $words.Add([pscustomobject]@{
                                text = [string]$word.Text
                                x = [double]$box.X; y = [double]$box.Y
                                w = [double]$box.Width; h = [double]$box.Height
                            })
                    }
                }
                return [pscustomobject]@{ text = [string]$result.Text; words = $words.ToArray() }
            } finally { $software.Dispose() }
        } finally { $stream.Dispose() }
    }
    # Words on the painted sheet whose text contains $needle, top to bottom.
    function MPWords($ocr, [string]$needle) {
        return @($ocr.words | Where-Object { $_.text -match [regex]::Escape($needle) } | Sort-Object y)
    }
    # Ink = pixels clearly brighter than the sheet background (0x201c1c); the value is the
    # count inside the box, so a bold run (thicker strokes) has more of it than the same
    # words in regular weight.
    function MPInk($bitmap, $word) {
        $ink = 0
        $left = [int][Math]::Floor($word.x); $top = [int][Math]::Floor($word.y)
        $right = [int][Math]::Ceiling($word.x + $word.w); $bottom = [int][Math]::Ceiling($word.y + $word.h)
        $left = [Math]::Max($left, 0); $top = [Math]::Max($top, 0)
        $right = [Math]::Min($right, $bitmap.Width - 1); $bottom = [Math]::Min($bottom, $bitmap.Height - 1)
        for ($y = $top; $y -le $bottom; $y++) {
            for ($x = $left; $x -le $right; $x++) {
                $c = $bitmap.GetPixel($x, $y)
                if (($c.R + $c.G + $c.B) -gt 288) { $ink++ }
            }
        }
        return $ink
    }
    # Orange-ish pixels inside the box: the requested #ff8800, allowing for anti-aliasing.
    function MPOrange($bitmap, $word) {
        $count = 0
        $left = [int][Math]::Floor($word.x); $top = [int][Math]::Floor($word.y)
        $right = [int][Math]::Ceiling($word.x + $word.w); $bottom = [int][Math]::Ceiling($word.y + $word.h)
        $left = [Math]::Max($left, 0); $top = [Math]::Max($top, 0)
        $right = [Math]::Min($right, $bitmap.Width - 1); $bottom = [Math]::Min($bottom, $bitmap.Height - 1)
        for ($y = $top; $y -le $bottom; $y++) {
            for ($x = $left; $x -le $right; $x++) {
                $c = $bitmap.GetPixel($x, $y)
                if ($c.R -ge 120 -and $c.G -ge 60 -and $c.G -le 200 -and $c.B -le 120) { $count++ }
            }
        }
        return $count
    }
    $mpOcr = $null
    $mpBold = @()
    $mpPlain = @()
    $mpOrangeWord = $null
    $mpBoldInk = -1
    $mpPlainInk = -1
    $mpOrangeInk = -1
    $mpControlOrange = -1
    try {
        MPNative
        Add-Type -AssemblyName System.Drawing
        Add-Type -AssemblyName System.Runtime.WindowsRuntime
        $null = [Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime]
        $null = [Windows.Storage.Streams.IRandomAccessStream, Windows.Storage.Streams, ContentType = WindowsRuntime]
        $null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType = WindowsRuntime]
        $null = [Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType = WindowsRuntime]
        $null = [Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType = WindowsRuntime]
        $null = [Windows.Media.Ocr.OcrResult, Windows.Foundation, ContentType = WindowsRuntime]
        $null = [Windows.Globalization.Language, Windows.Globalization, ContentType = WindowsRuntime]
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'H89 refuses an existing wiri instance'
        }
        $env:WIRI_PIPE = '\\.\pipe\wiri_live_h89_' + [Guid]::NewGuid().ToString('N')
        $mpDaemon = Start-WiriWithConfig $mpConfig $mpLog -NoDefaultBinds
        $mpDetails['daemon'] = [ordered]@{ pid = $mpDaemon.Id; pipe = $env:WIRI_PIPE; config = $cfg; log = $mpLog }
        $null = Ctl 'h89-show-overlay' @('--json', 'action', 'show-hotkey-overlay')
        $mpClock = [Diagnostics.Stopwatch]::StartNew()
        $mpWindow = [IntPtr]::Zero
        while ($mpClock.ElapsedMilliseconds -lt 10000) {
            if ($mpDaemon.HasExited) { throw 'H89 owned daemon exited before the sheet appeared' }
            $mpWindow = [LanePangoNative]::Find([uint32]$mpDaemon.Id, 'WiriBindingsCheatsheet')
            if ($mpWindow -ne [IntPtr]::Zero) { break }
            Start-Sleep -Milliseconds 150
        }
        $mpDetails['window'] = $mpWindow.ToInt64()
        if ($mpWindow -eq [IntPtr]::Zero) { throw 'H89 the hotkey sheet window never appeared' }
        Start-Sleep -Milliseconds 700
        $mpBounds = [LanePangoNative]::Bounds($mpWindow)
        $mpBitmap = New-Object Drawing.Bitmap ($mpBounds[2] - $mpBounds[0]), ($mpBounds[3] - $mpBounds[1])
        try {
            $mpGraphics = [Drawing.Graphics]::FromImage($mpBitmap)
            try { $mpGraphics.CopyFromScreen($mpBounds[0], $mpBounds[1], 0, 0, $mpBitmap.Size); $mpBitmap.Save($mpCapture, [Drawing.Imaging.ImageFormat]::Png) }
            finally { $mpGraphics.Dispose() }
            $mpOcr = MPRead $mpCapture ([DateTime]::UtcNow.AddSeconds(20))
            $mpBold = @(MPWords $mpOcr 'PangoBold')
            $mpOrangeWord = @(MPWords $mpOcr 'PangoOrange') | Select-Object -First 1
            if ($mpBold.Count -ge 2) {
                $mpBoldInk = MPInk $mpBitmap $mpBold[0]
                $mpPlainInk = MPInk $mpBitmap $mpBold[$mpBold.Count - 1]
                $mpControlOrange = MPOrange $mpBitmap $mpBold[$mpBold.Count - 1]
            }
            if ($null -ne $mpOrangeWord) { $mpOrangeInk = MPOrange $mpBitmap $mpOrangeWord }
        } finally { $mpBitmap.Dispose() }
        $mpDetails['capture'] = $mpCapture
        $mpDetails['rendered_text'] = $(if ($null -ne $mpOcr) { $mpOcr.text } else { '' })
        $mpDetails['words'] = $(if ($null -ne $mpOcr) { @($mpOcr.words | ForEach-Object { "$($_.text)@$([int]$_.x),$([int]$_.y)" }) } else { @() })
    } catch {
        $mpDetails['capture-error'] = ($_ | Out-String)
    }
    # Group 1: the painted words and the entity.
    try {
        $mpText = $(if ($null -ne $mpOcr) { $mpOcr.text } else { '' })
        $mpHasOrange = $mpText -match 'PangoOrange'
        $mpHasBold = ($mpBold.Count -ge 2)
        $mpHasMore = $mpText -match 'more'
        # The entity was decoded and the tags were not painted as text.
        $mpNoEntity = ($mpText -notmatch 'amp;') -and ($mpText -notmatch '&amp')
        $mpNoTagText = ($mpText -notmatch '<b>') -and ($mpText -notmatch '<span')
        MPGate 'h89_markup_title_text_and_entity_render' ($mpHasOrange -and $mpHasBold -and $mpHasMore -and $mpNoEntity -and $mpNoTagText) ([ordered]@{
                text = $mpText; orange = $mpHasOrange; bold_rows = $mpBold.Count; more = $mpHasMore
                entity_decoded = $mpNoEntity; tags_not_literal = $mpNoTagText
            })
    } catch {
        $mpDetails['text-gate-error'] = ($_ | Out-String)
    }
    # Group 2: the colour span reached the pixels.
    try {
        $mpColoured = ($null -ne $mpOrangeWord) -and ($mpOrangeInk -ge 12) -and ($mpControlOrange -lt 12)
        MPGate 'h89_markup_colour_span_is_painted' $mpColoured ([ordered]@{
                orange_word = $(if ($null -ne $mpOrangeWord) { "$($mpOrangeWord.text)@$([int]$mpOrangeWord.x),$([int]$mpOrangeWord.y)" } else { '<missing>' })
                orange_pixels = $mpOrangeInk; control_pixels = $mpControlOrange
            })
    } catch {
        $mpDetails['colour-gate-error'] = ($_ | Out-String)
    }
    # Group 3: the bold run has heavier strokes than the same words in regular weight.
    try {
        $mpHeavier = ($mpBoldInk -gt 0) -and ($mpPlainInk -gt 0) -and ($mpBoldInk -gt [int][Math]::Floor($mpPlainInk * 1.05))
        MPGate 'h89_markup_bold_run_has_more_ink' $mpHeavier ([ordered]@{
                bold_ink = $mpBoldInk; plain_ink = $mpPlainInk; needed = [int][Math]::Floor($mpPlainInk * 1.05)
            })
    } catch {
        $mpDetails['bold-gate-error'] = ($_ | Out-String)
    }
    # Group 4: invalid markup paints the whole original string, tags and all.
    try {
        $mpText = $(if ($null -ne $mpOcr) { $mpOcr.text } else { '' })
        $mpFallback = ($mpText -match 'PangoFallback') -and ($mpText -match 'blinkish')
        MPGate 'h89_invalid_markup_falls_back_to_the_whole_string' $mpFallback ([ordered]@{
                text = $mpText; tag_text_visible = ($mpText -match 'blinkish'); title_visible = ($mpText -match 'PangoFallback')
            })
    } catch {
        $mpDetails['fallback-gate-error'] = ($_ | Out-String)
    }
    # Group 5: exactly one warning for the invalid title, logged at config load.
    try {
        $mpWarnings = DiagnosticCount $mpLog 'is not valid Pango markup'
        $mpWarned = ($mpWarnings -eq 1) -and (@(LogLines $mpLog 'blinkish').Count -ge 1)
        MPGate 'h89_invalid_markup_logs_one_warning_at_load' $mpWarned ([ordered]@{
                warnings = $mpWarnings; lines = @(LogLines $mpLog 'is not valid Pango markup' | ForEach-Object { $_.Line })
            })
    } catch {
        $mpDetails['warning-gate-error'] = ($_ | Out-String)
    }
    # Group 6: the lane's own daemon is gone.
    try {
        if ($null -ne $mpDaemon) {
            if (-not $mpDaemon.HasExited) {
                try { $null = Ctl 'h89-quit' @('quit') } catch { }
                $null = $mpDaemon.WaitForExit(8000)
            }
            if (-not $mpDaemon.HasExited) { Stop-Process -Id $mpDaemon.Id -Force -ErrorAction SilentlyContinue }
            $null = $mpDaemon.WaitForExit(4000)
        }
        $mpGone = ($null -ne $mpDaemon) -and $mpDaemon.HasExited
        MPGate 'h89_markup_cleanup' $mpGone ([ordered]@{
                pid = $(if ($null -ne $mpDaemon) { $mpDaemon.Id } else { -1 }); exited = $mpGone
                left_running = @(Get-Process -Name wiri -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
            })
    } catch {
        $mpDetails['cleanup-gate-error'] = ($_ | Out-String)
    }
    $env:WIRI_PIPE = $mpSavedPipe
    $mpDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $mpRoot 'evidence.json')
}
