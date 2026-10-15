-- 按「跳跃」键（默认空格）把自己弹起来。
--
-- 演示脚本能拿到三样东西：
--   1. 输入动作（`action_pressed`）—— 用的是项目根目录 `input.ron` 里的映射；
--   2. 自己的实体 ID 与刚体（`self_id` + `velocity` / `set_velocity`）；
--   3. 时间（`delta`）。
--
-- 换按键不用改这个脚本：改 `input.ron` 里 "Jump" 的绑定即可。

local JUMP_SPEED = 6.5
local JUMP_SOUND = "Resources/Audio/Beep.wav"

function update(ctx)
    if not action_pressed(ctx, "Jump") then
        return
    end

    local me = self_id(ctx)
    local v = velocity(ctx, me)
    -- 保留水平速度，只把垂直速度设成跳跃速度 —— 跳起来还能继续横向跑。
    set_velocity(ctx, me, v.x, JUMP_SPEED, v.z)
    -- 起跳给个一次性音效（pitch 拉高听起来更"弹"）；音频设备不可用时
    -- 这些调用照样安全，只是不出声（引擎在设备失败时无声降级）。
    play_sound(ctx, JUMP_SOUND, 0.6, 1.5)
end
