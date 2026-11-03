param(
    [string]$BinDir = 'dist',
    [string]$OutputRoot = 'outputs',
    [int]$TimeoutSeconds = 15
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
function Resolve-RepoPath([string]$path) {
    if (-not [IO.Path]::IsPathRooted($path)) { $path = Join-Path $root $path }
    return (Resolve-Path $path).Path
}
$bin = Resolve-RepoPath $BinDir
$run = Join-Path (Resolve-RepoPath $OutputRoot) ('player-focus-' + [guid]::NewGuid().ToString('N'))
$source = Join-Path $run 'source'
foreach ($dir in @('Scenes', 'Scripts')) { [void][IO.Directory]::CreateDirectory((Join-Path $source $dir)) }
$utf8 = New-Object Text.UTF8Encoding($false)
function Save-Text($path, $text) { [IO.File]::WriteAllText($path, $text, $utf8) }
Save-Text (Join-Path $source 'project.ron') @'
(
    name: "FocusAcceptance",
    entry_scene: "Scenes/main.ron",
    window: (title: "XGame Focus Acceptance", width: 640, height: 360, resizable: false, vsync: true),
)
'@
Save-Text (Join-Path $source 'input.ron') @'
InputMap(actions: {"Hold": [Mouse(Left)], "Background": [Key(KeyB)], "Resume": [Key(KeyK)]}, axes: {})
'@
Save-Text (Join-Path $source 'Scenes/main.ron') @'
(
    name: "Focus Acceptance",
    entities: [(
        name: "Button", id: (1), scripts: ["Scripts/focus.lua"],
        ui: Some((kind: Button, visible: true, interactable: true,
            anchor_max: (1.0, 1.0), offset_max: (0.0, 0.0),
            style: (background: (0.04, 0.12, 0.25, 1.0)),
            text: (content: "Focus acceptance", size: 24.0))),
    )],
)
'@
Save-Text (Join-Path $source 'Scripts/focus.lua') @'
local stage = 0
local quiet_frames = 0
local observed = ""
function start(ctx) log(ctx, "FOCUS READY") end
function update(ctx)
    local id = self_id(ctx)
    if stage == 0 then
        local state = tostring(action_down(ctx, "Hold")) .. "/" .. tostring(ui_hovered(ctx, id))
        if state ~= observed then
            observed = state
            local p = mouse_pos(ctx)
            log(ctx, "FOCUS INITIAL down/hover=" .. state .. " cursor=" .. p.x .. "," .. p.y)
        end
    end
    if ui_clicked(ctx, id) and stage ~= 5 then error("unexpected click during focus cancellation") end
    if stage == 0 and action_down(ctx, "Hold") and ui_hovered(ctx, id) then
        stage = 1
        log(ctx, "FOCUS HELD")
    elseif stage == 1 and action_released(ctx, "Hold") then
        assert(not action_down(ctx, "Hold") and not ui_hovered(ctx, id))
        stage = 2
        log(ctx, "FOCUS CANCELLED")
    elseif stage == 2 or stage == 3 then
        if action_pressed(ctx, "Resume") then
            assert(stage == 3)
            stage = 4
            log(ctx, "FOCUS RESTORED")
        else
            assert(not action_down(ctx, "Hold") and not action_pressed(ctx, "Hold"))
            assert(not action_down(ctx, "Background") and not action_pressed(ctx, "Background"))
            assert(not ui_hovered(ctx, id))
            quiet_frames = quiet_frames + 1
            if quiet_frames == 30 then
                stage = 3
                log(ctx, "FOCUS BACKGROUND")
            end
        end
    elseif stage == 4 and action_pressed(ctx, "Hold") and ui_hovered(ctx, id) then
        stage = 5
        log(ctx, "FOCUS HELD_AGAIN")
    elseif stage == 5 and ui_clicked(ctx, id) then
        stage = 6
        ui_set_text(ctx, id, "PASS: focus cancelled and restored")
        log(ctx, "FOCUS COMPLETE")
    end
end
'@

# 使用真实发布包，避免源码 cwd 或未更新的播放器掩盖宿主接线问题。
$build = Start-Process (Join-Path $bin 'xgame-editor.exe') -ArgumentList @('--build', ('"' + $source + '"')) -Wait -PassThru -RedirectStandardOutput (Join-Path $run 'build.log') -RedirectStandardError (Join-Path $run 'build-stderr.log')
if ($build.ExitCode -ne 0) { throw "Build failed: $run" }
Move-Item (Join-Path $source 'Build/FocusAcceptance') (Join-Path $run 'package')
Move-Item $source (Join-Path $run 'source-preserved')
$exe = Join-Path $run 'package/FocusAcceptance.exe'
if ((Get-FileHash $exe).Hash -ne (Get-FileHash (Join-Path $bin 'xgame-game.exe')).Hash) { throw 'Player hash mismatch' }

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class FocusPlayerWindow {
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out Point p);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref Point p);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [StructLayout(LayoutKind.Sequential)] public struct Point { public int X, Y; }
    [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint kind);
    [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(uint access, bool inherit, int id);
    [DllImport("kernel32.dll")] public static extern bool GetExitCodeProcess(IntPtr h, out uint code);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
}
'@
$env:RUST_LOG = 'info'
$stderr = Join-Path $run 'player.log'
$p = Start-Process $exe -WorkingDirectory $run -PassThru -RedirectStandardOutput (Join-Path $run 'stdout.log') -RedirectStandardError $stderr
$handle = [FocusPlayerWindow]::OpenProcess(0x1000, $false, $p.Id)
if ($handle -eq [IntPtr]::Zero) { throw 'Cannot open player process handle' }
$oldCursor = New-Object FocusPlayerWindow+Point
$haveCursor = [FocusPlayerWindow]::GetCursorPos([ref]$oldCursor)
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
        if ($text -match '(?m)\bERROR\b|panicked at') { throw "Player error before $marker" }
        if ($text.Contains($marker)) { return }
        if ($p.HasExited) { throw "Player exited before $marker" }
        if ([DateTime]::UtcNow -gt $deadline) { throw "Timed out waiting for $marker" }
        [Threading.Thread]::Sleep(25)
    } while ($true)
}
function Send-Message([uint32]$message, [long]$w = 0, [long]$l = 0) {
    $p.Refresh()
    if (-not [FocusPlayerWindow]::PostMessage($p.MainWindowHandle, $message, [IntPtr]$w, [IntPtr]$l)) { throw 'Window message failed' }
}
function Send-Key([uint32]$key, [bool]$down) {
    $bits = [long](1 + ([FocusPlayerWindow]::MapVirtualKey($key, 0) -shl 16))
    if ($down) { Send-Message 0x100 $key $bits }
    else { Send-Message 0x101 $key ($bits -bor 0xC0000000L) }
}
try {
    Wait-Marker 'FOCUS READY'
    $p.Refresh()
    [void][FocusPlayerWindow]::SetForegroundWindow($p.MainWindowHandle)
    # 系统光标必须在客户区，否则 TrackMouseEvent 会给合成移动追加 CursorLeft。
    $cursor = New-Object FocusPlayerWindow+Point
    $cursor.X = 120
    $cursor.Y = 80
    if (-not [FocusPlayerWindow]::ClientToScreen($p.MainWindowHandle, [ref]$cursor) -or
        -not [FocusPlayerWindow]::SetCursorPos($cursor.X, $cursor.Y)) { throw 'Cannot position cursor' }
    # 注入窗口消息，经过 winit 的正式翻译路径；不是物理 Alt+Tab 或设备验收。
    Send-Message 0x86 1
    Send-Message 0x7
    Send-Message 0x200 0 ((80 -shl 16) -bor 120)
    Wait-Marker 'FOCUS INITIAL down/hover=false/true'
    Send-Message 0x201 1 ((80 -shl 16) -bor 120)
    Wait-Marker 'FOCUS HELD'
    Send-Message 0x8
    Wait-Marker 'FOCUS CANCELLED'
    Send-Message 0x200 0 ((200 -shl 16) -bor 300)
    Send-Message 0x201 1 ((200 -shl 16) -bor 300)
    Send-Message 0x202 0 ((200 -shl 16) -bor 300)
    Send-Key 0x42 $true
    Send-Key 0x42 $false
    Wait-Marker 'FOCUS BACKGROUND'
    Send-Message 0x7
    Send-Key 0x4B $true
    Wait-Marker 'FOCUS RESTORED'
    Send-Key 0x4B $false
    Send-Message 0x200 0 ((80 -shl 16) -bor 120)
    Send-Message 0x201 1 ((80 -shl 16) -bor 120)
    Wait-Marker 'FOCUS HELD_AGAIN'
    Send-Message 0x202 0 ((80 -shl 16) -bor 120)
    Wait-Marker 'FOCUS COMPLETE'
    Send-Message 0x10
    if (-not $p.WaitForExit(10000)) { throw 'Player did not close normally' }
    [uint32]$exitCode = 259
    if (-not [FocusPlayerWindow]::GetExitCodeProcess($handle, [ref]$exitCode) -or $exitCode -ne 0) { throw "Player exit code $exitCode" }
    $text = Read-PlayerLog
    foreach ($tag in @('READY', 'HELD', 'CANCELLED', 'BACKGROUND', 'RESTORED', 'HELD_AGAIN', 'COMPLETE')) {
        if ([regex]::Matches($text, "FOCUS $tag\b").Count -ne 1) { throw "Expected exactly one $tag" }
    }
    Save-Text (Join-Path $run 'result.json') (@{
        ok = $true; exit_code = $exitCode; assertions = 7
        input = 'Windows focus/key/mouse messages via winit'
        executable_sha256 = (Get-FileHash $exe).Hash
    } | ConvertTo-Json)
    Write-Output "PASS: $run"
} catch {
    Save-Text (Join-Path $run 'failure.txt') $_.ToString()
    throw
} finally {
    $p.Refresh()
    if (-not $p.HasExited) {
        Send-Message 0x10
        [void]$p.WaitForExit(10000)
    }
    [void][FocusPlayerWindow]::CloseHandle($handle)
    if ($haveCursor) { [void][FocusPlayerWindow]::SetCursorPos($oldCursor.X, $oldCursor.Y) }
    Write-Output "Evidence: $run"
}
