param(
  [int]$Seconds = 12,
  [switch]$ClickPanelClose
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$ProjectRoot = Resolve-Path (Join-Path $RepoRoot "..\..")
$WorkDir = Join-Path $ProjectRoot "work"
$OutputDir = Join-Path $ProjectRoot "outputs"
$DiagPath = Join-Path $WorkDir "window-diagnostics.jsonl"
$RunnerPath = Join-Path $WorkDir "run-window-verify-dev.ps1"
$StdoutPath = Join-Path $WorkDir "window-verify-stdout.log"
$StderrPath = Join-Path $WorkDir "window-verify-stderr.log"
$WindowsPath = Join-Path $WorkDir "window-verify-windows.json"
$BeforeCloseWindowsPath = Join-Path $WorkDir "window-verify-windows-before-close.json"
$ScreenshotPath = Join-Path $OutputDir "window-verification.png"

New-Item -ItemType Directory -Force -Path $WorkDir, $OutputDir | Out-Null
Remove-Item -LiteralPath $DiagPath -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $StdoutPath, $StderrPath, $WindowsPath, $BeforeCloseWindowsPath -ErrorAction SilentlyContinue
Get-ChildItem -LiteralPath $OutputDir -Filter "window-capture-*.png" -ErrorAction SilentlyContinue |
  Remove-Item -Force -ErrorAction SilentlyContinue

$runner = @"
`$env:VITE_USE_MOCK = '1'
`$env:VITE_WINDOW_DIAGNOSTICS = '1'
`$env:VITE_DIAG_TOGGLE_COMPACT = '1'
`$env:AI_STATUS_DIAG_PATH = '$DiagPath'
`$env:AI_STATUS_DIAG_OPEN_PANEL = '1'
`$env:RUSTUP_HOME = "`$env:USERPROFILE\.rustup"
`$env:CARGO_HOME = '$WorkDir\cargo-home'
`$env:HOME = '$WorkDir\cargo-home\home'
`$env:Path = "`$env:USERPROFILE\.cargo\bin;`$env:Path"
`$env:GIT_CONFIG_GLOBAL = 'NUL'
`$env:HTTP_PROXY = ''
`$env:HTTPS_PROXY = ''
`$env:ALL_PROXY = ''
`$env:http_proxy = ''
`$env:https_proxy = ''
`$env:all_proxy = ''
`$env:NO_PROXY = '*'
`$env:no_proxy = '*'
Set-Location -LiteralPath '$RepoRoot'
npm run tauri dev
"@
Set-Content -LiteralPath $RunnerPath -Value $runner -Encoding ascii

function Stop-ProcessTree {
  param([int]$RootPid)
  $children = Get-CimInstance Win32_Process -Filter "ParentProcessId=$RootPid" -ErrorAction SilentlyContinue
  foreach ($child in $children) {
    Stop-ProcessTree -RootPid ([int]$child.ProcessId)
  }
  Stop-Process -Id $RootPid -Force -ErrorAction SilentlyContinue
}

function Get-ProcessTreeIds {
  param([int]$RootPid)
  $ids = New-Object System.Collections.Generic.List[int]
  $ids.Add($RootPid)
  $children = Get-CimInstance Win32_Process -Filter "ParentProcessId=$RootPid" -ErrorAction SilentlyContinue
  foreach ($child in $children) {
    foreach ($id in Get-ProcessTreeIds -RootPid ([int]$child.ProcessId)) {
      if (-not $ids.Contains($id)) {
        $ids.Add($id)
      }
    }
  }
  return $ids
}

function Save-Screenshot {
  param([string]$Path)
  Add-Type -AssemblyName System.Windows.Forms
  Add-Type -AssemblyName System.Drawing
  $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  try {
    $graphics.CopyFromScreen($bounds.Left, $bounds.Top, 0, 0, $bounds.Size)
    $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
  } finally {
    $graphics.Dispose()
    $bitmap.Dispose()
  }
}

function Save-WindowCaptures {
  param(
    [int[]]$TargetPids,
    [string]$OutputDirectory,
    [string]$WindowListPath
  )
  Add-Type -AssemblyName System.Drawing
  if (-not ("Win32WindowTools" -as [type])) {
    Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class Win32WindowTools {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

  [StructLayout(LayoutKind.Sequential)]
  public struct RECT {
    public int Left;
    public int Top;
    public int Right;
    public int Bottom;
  }

  [DllImport("user32.dll")]
  public static extern bool SetProcessDpiAwarenessContext(IntPtr dpiContext);

  [DllImport("user32.dll")]
  public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

  [DllImport("user32.dll")]
  public static extern bool IsWindowVisible(IntPtr hWnd);

  [DllImport("user32.dll")]
  public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

  [DllImport("user32.dll")]
  public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);

  [DllImport("user32.dll")]
  public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
}
"@
  }

  # Per-monitor DPI aware v2. This makes GetWindowRect and PrintWindow use physical pixels.
  [void][Win32WindowTools]::SetProcessDpiAwarenessContext([IntPtr](-4))

  $target = New-Object 'System.Collections.Generic.HashSet[int]'
  foreach ($pidValue in $TargetPids) {
    [void]$target.Add($pidValue)
  }
  $windows = New-Object System.Collections.Generic.List[object]
  $callback = [Win32WindowTools+EnumWindowsProc]{
    param([IntPtr]$hWnd, [IntPtr]$lParam)
    [uint32]$windowPid = 0
    [void][Win32WindowTools]::GetWindowThreadProcessId($hWnd, [ref]$windowPid)
    if ($target.Contains([int]$windowPid) -and [Win32WindowTools]::IsWindowVisible($hWnd)) {
      $rect = New-Object Win32WindowTools+RECT
      if ([Win32WindowTools]::GetWindowRect($hWnd, [ref]$rect)) {
        $width = $rect.Right - $rect.Left
        $height = $rect.Bottom - $rect.Top
        $titleBuilder = New-Object System.Text.StringBuilder 512
        [void][Win32WindowTools]::GetWindowText($hWnd, $titleBuilder, $titleBuilder.Capacity)
        $handle = $hWnd.ToInt64()
        $capturePath = Join-Path $OutputDirectory ("window-capture-{0}-{1}.png" -f $windowPid, $handle)
        $captured = $false
        if ($width -gt 0 -and $height -gt 0) {
          $bitmap = New-Object System.Drawing.Bitmap $width, $height
          $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
          $hdc = $graphics.GetHdc()
          try {
            $captured = [Win32WindowTools]::PrintWindow($hWnd, $hdc, 2)
          } finally {
            $graphics.ReleaseHdc($hdc)
            $graphics.Dispose()
          }
          if ($captured) {
            $bitmap.Save($capturePath, [System.Drawing.Imaging.ImageFormat]::Png)
          }
          $bitmap.Dispose()
        }
        $windows.Add([pscustomobject]@{
          pid = [int]$windowPid
          hwnd = $handle
          title = $titleBuilder.ToString()
          x = $rect.Left
          y = $rect.Top
          width = $width
          height = $height
          captured = $captured
          capture = if ($captured) { $capturePath } else { $null }
        })
      }
    }
    return $true
  }
  [void][Win32WindowTools]::EnumWindows($callback, [IntPtr]::Zero)
  $windows | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $WindowListPath -Encoding utf8
  return $windows
}

function Invoke-LeftClick {
  param(
    [int64]$WindowHandle,
    [int]$X,
    [int]$Y
  )
  if (-not ("Win32InputTools" -as [type])) {
    Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class Win32InputTools {
  [DllImport("user32.dll")]
  public static extern bool SetForegroundWindow(IntPtr hWnd);

  [DllImport("user32.dll")]
  public static extern bool SetCursorPos(int x, int y);

  [DllImport("user32.dll")]
  public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, UIntPtr dwExtraInfo);
}
"@
  }
  [void][Win32InputTools]::SetForegroundWindow([IntPtr]$WindowHandle)
  Start-Sleep -Milliseconds 150
  [void][Win32InputTools]::SetCursorPos($X, $Y)
  Start-Sleep -Milliseconds 80
  [Win32InputTools]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 60
  [Win32InputTools]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
}

$process = Start-Process `
  -FilePath "powershell.exe" `
  -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $RunnerPath) `
  -PassThru `
  -WindowStyle Hidden `
  -RedirectStandardOutput $StdoutPath `
  -RedirectStandardError $StderrPath

try {
  Start-Sleep -Seconds $Seconds
  $treeIds = @(Get-ProcessTreeIds -RootPid $process.Id)
  if ($ClickPanelClose) {
    $beforeCloseWindows = Save-WindowCaptures -TargetPids $treeIds -OutputDirectory $OutputDir -WindowListPath $BeforeCloseWindowsPath
    $panelBeforeClose = $beforeCloseWindows |
      Where-Object { $_.title -like "AI Status*" -or $_.title -like "*Status*Panel*" } |
      Sort-Object width, height -Descending |
      Select-Object -First 1
    if (-not $panelBeforeClose) {
      throw "Panel window was not visible before close-click verification."
    }
    $clickX = [int]($panelBeforeClose.x + $panelBeforeClose.width - 68)
    $clickY = [int]($panelBeforeClose.y + 66)
    Write-Host "clickPanelClose=$($panelBeforeClose.hwnd),$clickX,$clickY"
    Invoke-LeftClick -WindowHandle ([int64]$panelBeforeClose.hwnd) -X $clickX -Y $clickY
    Start-Sleep -Milliseconds 900
  }
  Save-Screenshot -Path $ScreenshotPath
  $windowsAfter = Save-WindowCaptures -TargetPids $treeIds -OutputDirectory $OutputDir -WindowListPath $WindowsPath
  if ($ClickPanelClose) {
    $panelAfterClose = $windowsAfter |
      Where-Object { $_.title -like "AI Status*" -or $_.title -like "*Status*Panel*" } |
      Select-Object -First 1
    if ($panelAfterClose) {
      throw "Panel window remained visible after close click."
    }
  }
} finally {
  Stop-ProcessTree -RootPid $process.Id
}

Write-Host "diagnostics=$DiagPath"
Write-Host "screenshot=$ScreenshotPath"
Write-Host "stdout=$StdoutPath"
Write-Host "stderr=$StderrPath"
Write-Host "windows=$WindowsPath"
if ($ClickPanelClose) {
  Write-Host "beforeCloseWindows=$BeforeCloseWindowsPath"
}
if (Test-Path -LiteralPath $DiagPath) {
  Get-Content -LiteralPath $DiagPath -Tail 40
}
if (Test-Path -LiteralPath $WindowsPath) {
  Write-Host "--- windows ---"
  Get-Content -LiteralPath $WindowsPath
}
if ($ClickPanelClose -and (Test-Path -LiteralPath $BeforeCloseWindowsPath)) {
  Write-Host "--- windows before close ---"
  Get-Content -LiteralPath $BeforeCloseWindowsPath
}
if (Test-Path -LiteralPath $StdoutPath) {
  Write-Host "--- stdout tail ---"
  Get-Content -LiteralPath $StdoutPath -Tail 80
}
if (Test-Path -LiteralPath $StderrPath) {
  Write-Host "--- stderr tail ---"
  Get-Content -LiteralPath $StderrPath -Tail 80
}
