param(
    [string]$BinDir = 'dist',
    [string]$Plugin = 'target/release/behaviors_demo.dll',
    [string]$OutputRoot = 'outputs',
    [ValidateSet('window', 'headless')][string]$Mode = 'window',
    [ValidateSet('none', 'input', 'plugin')][string]$Fault = 'none',
    [int]$TimeoutSeconds = 30
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
function Resolve-RepoPath([string]$path) {
    $candidate = if ([IO.Path]::IsPathRooted($path)) { $path } else { Join-Path $root $path }
    return (Resolve-Path $candidate).Path
}
$bin = Resolve-RepoPath $BinDir
$dll = Resolve-RepoPath $Plugin
$out = Resolve-RepoPath $OutputRoot
$run = Join-Path $out ('published-player-' + [guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($run)
$source = Join-Path $run 'source'
foreach ($dir in @('Scenes', 'Gameplay', 'Plugins')) {
    [void][IO.Directory]::CreateDirectory((Join-Path $source $dir))
}
$utf8 = New-Object Text.UTF8Encoding($false)
function Save-Text($path, $text) { [IO.File]::WriteAllText($path, $text, $utf8) }
Save-Text (Join-Path $source 'project.ron') @'
(
    name: "PublishedAcceptance",
    entry_scene: "Scenes/0.ron",
    window: (title: "XGame Published Acceptance", width: 960, height: 540, resizable: true, vsync: true),
    content: (scenes_dir: "Scenes", models_dir: "Resources", textures_dir: "Resources", audio_dir: "Resources", scripts_dir: "Gameplay", search_paths: []),
    build: (version: "g0-acceptance", scenes: ["Scenes/0.ron", "Scenes/1.ron", "Scenes/2.ron"]),
)
'@
$key = if ($Fault -eq 'input') { 'KeyK' } else { 'KeyJ' }
Save-Text (Join-Path $source 'input.ron') @"
InputMap(actions: {"Advance": [Key($key)]}, axes: {"Travel": (positive: [Key($key)], negative: [Key(KeyL)])})
"@
Save-Text (Join-Path $source 'Gameplay/accept.lua') @'
function start(ctx)
    state_set(ctx, "initial_yaw", rot(ctx).y)
    log(ctx, "G0 START " .. scene_index(ctx) .. " " .. current_scene(ctx))
end
function update(ctx)
    local i = scene_index(ctx)
    if not state_get_or(ctx, "plugin", false) and math.abs(rot(ctx).y - state_get(ctx, "initial_yaw")) > 5 then
        state_set(ctx, "plugin", true)
        log(ctx, "G0 PLUGIN " .. i)
    end
    if action_pressed(ctx, "Advance") and action_down(ctx, "Advance") and axis(ctx, "Travel") == 1 then
        state_set(ctx, "pressed", true)
        log(ctx, "G0 PRESS " .. i)
    end
    if action_released(ctx, "Advance") and not action_down(ctx, "Advance") and axis(ctx, "Travel") == 0 and state_get_or(ctx, "pressed", false) and state_get_or(ctx, "plugin", false) then
        log(ctx, "G0 RELEASE " .. i)
        if i < 2 then
            load_scene_at(ctx, i + 1)
        else
            spawn_entity(ctx, "G0_COMPLETE", "")
            log(ctx, "G0 COMPLETE")
        end
    end
end
function on_destroy(ctx)
    log(ctx, "G0 DESTROY " .. scene_index(ctx))
end
'@
$scene = [IO.File]::ReadAllText((Join-Path $root 'projects/BehaviorsDemo/Scenes/main.ron'))
$scene = $scene.Replace('name: "RustSpinner",', 'name: "RustSpinner", scripts: ["Gameplay/accept.lua"],')
$scene = $scene.Replace('"guid://e1ea0ec292aab394364a94e4e17cc625"', '"Gameplay/Spinner.lua"')
foreach ($i in 0..2) { Save-Text (Join-Path $source "Scenes/$i.ron") ($scene.Replace('Behaviors Demo', "Acceptance $i")) }
Copy-Item (Join-Path $root 'projects/BehaviorsDemo/Scripts/Spinner.lua') (Join-Path $source 'Gameplay/Spinner.lua')
if ($Fault -ne 'plugin') { Copy-Item $dll (Join-Path $source 'Plugins/behaviors_demo.dll') }
$env:RUST_LOG = 'info'
$build = Start-Process (Join-Path $bin 'xgame-editor.exe') -ArgumentList @('--build', ('"' + $source + '"')) -Wait -PassThru -RedirectStandardOutput (Join-Path $run 'build.log') -RedirectStandardError (Join-Path $run 'build-stderr.log')
if ($build.ExitCode -ne 0) { throw "Build failed: $run" }
$package = Join-Path $source 'Build/PublishedAcceptance'
$exe = Join-Path $package 'PublishedAcceptance.exe'
foreach ($pair in @(@((Join-Path $bin 'xgame-game.exe'), $exe), @((Join-Path $source 'input.ron'), (Join-Path $package 'input.ron')))) {
    if ((Get-FileHash $pair[0]).Hash -ne (Get-FileHash $pair[1]).Hash) { throw 'Package hash mismatch' }
}
if ($Fault -ne 'plugin' -and (Get-FileHash $dll).Hash -ne (Get-FileHash (Join-Path $package 'Plugins/behaviors_demo.dll')).Hash) { throw 'Plugin hash mismatch' }
# 保留源项目，但让旧路径失效，验证包不依赖源目录或当前工作目录。
Move-Item $package (Join-Path $run 'package')
Move-Item $source (Join-Path $run 'source-preserved')
$exe = Join-Path $run 'package/PublishedAcceptance.exe'
if ($Mode -eq 'headless') {
    # Hosted CI 没有 GPU 窗口；按确定时间线把同一组 J 键边沿送进生产
    # InputState/InputMap。Lua 只在 Rust Spinner 已转动后切关，最终标记因此
    # 同时证明自定义输入、Rust 插件与三关切换确实组合运行。
    $events = Join-Path $run 'input-events.json'
    Save-Text $events @'
[
  {"frame": 8, "key": "KeyJ", "pressed": true},
  {"frame": 9, "key": "KeyJ", "pressed": false},
  {"frame": 17, "key": "KeyJ", "pressed": true},
  {"frame": 18, "key": "KeyJ", "pressed": false},
  {"frame": 26, "key": "KeyJ", "pressed": true},
  {"frame": 27, "key": "KeyJ", "pressed": false}
]
'@
    $reportPath = Join-Path $run 'headless-report.json'
    $dumpPath = Join-Path $run 'final-scene.ron'
    $headless = Start-Process (Join-Path $bin 'xgame-editor.exe') -ArgumentList @(
        '--headless', '--frames', '30', '--input-script', ('"' + $events + '"'),
        '--report', ('"' + $reportPath + '"'), '--dump-scene', ('"' + $dumpPath + '"'),
        ('"' + (Join-Path $run 'package') + '"')
    ) -WorkingDirectory $run -Wait -PassThru -RedirectStandardOutput (Join-Path $run 'headless-stdout.json') -RedirectStandardError (Join-Path $run 'headless-stderr.log')
    if ($headless.ExitCode -ne 0) { throw "Headless package run failed: $run ($($headless.ExitCode))" }
    $report = Get-Content $reportPath -Raw | ConvertFrom-Json
    if (-not $report.ok) { throw 'Headless report is not clean' }
    if ($report.input_events -ne 6) { throw "Expected 6 input events, got $($report.input_events)" }
    if ($report.scene_loads -ne 2) { throw "Expected 2 scene loads, got $($report.scene_loads)" }
    if ($report.scene_current -ne 'Scenes/2.ron') { throw "Expected final scene Scenes/2.ron, got $($report.scene_current)" }
    if ($report.entities -ne 5) { throw "Expected final completion entity, got $($report.entities) entities" }
    if (-not ([IO.File]::ReadAllText($dumpPath).Contains('name: "G0_COMPLETE"'))) { throw 'Final completion marker is missing' }
    if ($report.logs.error -ne 0) { throw "Expected zero runtime errors, got $($report.logs.error)" }
    Save-Text (Join-Path $run 'result.json') (@{
        ok = $true
        mode = 'headless'
        scenes = 3
        scene_loads = $report.scene_loads
        input_events = $report.input_events
        final_marker = 'G0_COMPLETE'
        executable_sha256 = (Get-FileHash $exe).Hash
        plugin_sha256 = (Get-FileHash $dll).Hash
    } | ConvertTo-Json)
    Write-Output "PASS: $run"
    Write-Output "Evidence: $run"
    return
}
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class PublishedPlayerWindow {
    [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(uint access, bool inherit, int id);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] public static extern bool GetExitCodeProcess(IntPtr h, out uint code);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint kind);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
}
'@
$stderr = Join-Path $run 'player.log'
$p = Start-Process $exe -WorkingDirectory $run -PassThru -RedirectStandardOutput (Join-Path $run 'stdout.log') -RedirectStandardError $stderr
# 提前持有进程句柄，避免快速退出后 .NET 无法取得退出码。
$processHandle = [PublishedPlayerWindow]::OpenProcess(0x1000, $false, $p.Id)
if ($processHandle -eq [IntPtr]::Zero) { throw 'Cannot open player process handle' }
function Read-PlayerLog {
    $stream = [IO.File]::Open($stderr, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    $reader = New-Object IO.StreamReader($stream)
    try { return $reader.ReadToEnd() } finally { $reader.Dispose() }
}
function Wait-Marker([string]$marker) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $p.Refresh()
        $text = Read-PlayerLog
        if ($text.Contains($marker)) { return }
        if ($p.HasExited) { throw "Player exited before $marker ($($p.ExitCode))" }
        if ([DateTime]::UtcNow -gt $deadline) { throw "Timed out waiting for $marker" }
        [Threading.Thread]::Sleep(50)
    } while ($true)
}
function Send-Key([bool]$down) {
    $scan = [PublishedPlayerWindow]::MapVirtualKey(0x4A, 0)
    $bits = [long](1 + ($scan -shl 16))
    $message = 0x100
    if (-not $down) { $message = 0x101; $bits = $bits -bor 0xC0000000L }
    if (-not [PublishedPlayerWindow]::PostMessage($p.MainWindowHandle, $message, [IntPtr]0x4A, [IntPtr]$bits)) { throw 'Key message failed' }
}
function Save-Window([int]$index) {
    $r = New-Object PublishedPlayerWindow+Rect
    if (-not [PublishedPlayerWindow]::GetWindowRect($p.MainWindowHandle, [ref]$r)) { throw 'Window rectangle unavailable' }
    $bitmap = New-Object Drawing.Bitmap(($r.Right - $r.Left), ($r.Bottom - $r.Top))
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $dc = $graphics.GetHdc()
    try { if (-not [PublishedPlayerWindow]::PrintWindow($p.MainWindowHandle, $dc, 2)) { throw 'Window capture failed' } }
    finally { $graphics.ReleaseHdc($dc); $graphics.Dispose() }
    try { $bitmap.Save((Join-Path $run "scene-$index.png"), [Drawing.Imaging.ImageFormat]::Png) }
    finally { $bitmap.Dispose() }
}
try {
    foreach ($i in 0..2) {
        Wait-Marker "G0 START $i Scenes/$i.ron"
        Wait-Marker "G0 PLUGIN $i"
        Save-Window $i
        Send-Key $true
        Wait-Marker "G0 PRESS $i"
        Send-Key $false
        Wait-Marker "G0 RELEASE $i"
    }
    Wait-Marker 'G0 COMPLETE'
    [void][PublishedPlayerWindow]::PostMessage($p.MainWindowHandle, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
    if (-not $p.WaitForExit(10000)) { throw 'Player did not close normally' }
    [uint32]$exitCode = 259
    if (-not [PublishedPlayerWindow]::GetExitCodeProcess($processHandle, [ref]$exitCode) -or $exitCode -ne 0) { throw "Player exit code $exitCode" }
    $text = Read-PlayerLog
    if ($text -match '(?m)\bERROR\b|panicked at') { throw 'Player reported an error' }
    foreach ($i in 0..2) {
        foreach ($tag in @('START', 'PLUGIN', 'PRESS', 'RELEASE', 'DESTROY')) {
            if ([regex]::Matches($text, "G0 $tag $i\b").Count -ne 1) { throw "Expected exactly one $tag for scene $i" }
        }
    }
    Save-Text (Join-Path $run 'result.json') (@{ ok = $true; scenes = 3; assertions = 15; exit_code = $exitCode; input = 'WM_KEYDOWN/WM_KEYUP via winit'; executable_sha256 = (Get-FileHash $exe).Hash; plugin_sha256 = (Get-FileHash $dll).Hash } | ConvertTo-Json)
    Write-Output "PASS: $run"
} catch {
    Save-Text (Join-Path $run 'failure.txt') $_.ToString()
    throw
} finally {
    $p.Refresh()
    if (-not $p.HasExited) {
        [void][PublishedPlayerWindow]::PostMessage($p.MainWindowHandle, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
        [void]$p.WaitForExit(10000)
    }
    [void][PublishedPlayerWindow]::CloseHandle($processHandle)
    Write-Output "Evidence: $run"
}
