-- HUD 演示：按钮点击 + 血条每帧动画
--
-- 这个脚本挂在 HUD.Button 上，用到的全部是**脚本层 UI API**：
--   ui_clicked / ui_set_text / ui_set_size / ui_set_bg_color / entity_id
-- 它同时是"运行时不碰渲染"的证明：血条宽度只是改场景里的 UI 组件字段，
-- 渲染器下一帧照常按锚点数出来。
--
-- 颜色参数是 **sRGB 0..1**（与美术 / CSS 的直觉一致），引擎内部会转线性。

local clicks = 0
local fill = 0
local counter = 0

function start(ctx)
    -- 只查一次：`entity_id` 是线性扫描，不必每帧查。
    fill = entity_id(ctx, "Bar.Fill")
    counter = entity_id(ctx, "HUD.ClickCount")
end

function update(ctx)
    -- 血条：正弦呼吸。宽度改的是"左边界不动、右边界跟着走"，
    -- 正是 `ui_set_size` 的语义（保持 offset_min）。
    if fill ~= 0 then
        local pulse = math.sin(time(ctx) * 2.2) * 0.5 + 0.5
        ui_set_size(ctx, fill, 4.0 + 300.0 * pulse, 14.0)
        -- 低血量偏红、高血量偏绿：三通道分别插值，不需要 HSL。
        ui_set_bg_color(ctx, fill, 0.95 - pulse * 0.6, 0.25 + pulse * 0.5, 0.28, 1.0)
    end

    -- 按钮：本帧"在按钮上按下并在按钮上松开"才算一次点击。
    if ui_clicked(ctx, self_id(ctx)) then
        clicks = clicks + 1
        ui_set_text(ctx, self_id(ctx), "点击测试 (" .. clicks .. ")")
        if counter ~= 0 then
            ui_set_text(ctx, counter, "点击: " .. clicks)
        end
    end
end
