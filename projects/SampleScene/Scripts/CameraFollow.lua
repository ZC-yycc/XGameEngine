-- 跟随相机（演示"相机是一个实体"这条路）。
--
-- 挂在**相机实体**上：每帧把机位摆到目标 + 固定偏移，再 `look_at` 目标胸口。
-- 引擎里没有任何"跟随相机"的特例代码 —— 相机就是普通实体：位置走 `set_pos`
-- （世界坐标），朝向走 `look_at`。
--
-- 两个设计点值得照抄：
--  1. `look_at` **只改朝向、不改位置**。合成一个 API 的话，跟随相机每帧都会
--     被拽到目标旁边 —— 这两件事必须分开。
--  2. 机位用**世界空间固定偏移**，不用"角色背后"。样例场景正中是一座箱塔，
--     正后方的视线正好被它挡住（实测）。要改成"绕到背后"只需两行：
--        local f = entity_forward(ctx, target)
--        set_pos(ctx, p.x - f.x * 5.0, p.y + 2.6, p.z - f.z * 5.0)

local TARGET_NAME = "Fox"
local OFFSET = { x = 4.2, y = 2.8, z = 3.6 }   -- 机位相对目标的世界偏移
local AIM_HEIGHT = 1.0                          -- 看向目标的哪个高度（胸口）

function update(ctx)
    local target = entity_id(ctx, TARGET_NAME)
    if target == 0 then
        -- 目标被销毁 / 改名：保持原地不动，不报错（容错优先，与其它 API 一致）。
        return
    end

    local p = entity_pos(ctx, target)
    set_pos(ctx, p.x + OFFSET.x, p.y + OFFSET.y, p.z + OFFSET.z)
    look_at(ctx, p.x, p.y + AIM_HEIGHT, p.z)
end
