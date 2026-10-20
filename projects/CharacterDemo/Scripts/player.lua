-- 角色控制器：WASD 移动 + 空格跳跃。
--
-- 引擎约定：实体原点是**脚底**，所以这个脚本不需要关心"胶囊中心在哪"。
-- `character_simple_move` 只吃水平速度；重力、坡面跟随、台阶、贴地吸附
-- 全部由引擎负责。

local SPEED = 5.0
local JUMP_SPEED = 5.5

function fixed_update(ctx)
    -- 先起跳再移动：跳跃只是把竖直速度塞进状态，真正离地发生在 move 时。
    if action_pressed(ctx, "Jump") and character_is_grounded(ctx) then
        character_jump(ctx, JUMP_SPEED)
    end

    local x = axis(ctx, "MoveX")
    local z = axis(ctx, "MoveY")
    character_simple_move(ctx, x * SPEED, 0.0, z * SPEED)

    -- 每秒一次落地状态日志，方便确认物理真的在跑。
    local t = state_get_or(ctx, "log_t", 0.0) + delta(ctx)
    if t >= 1.0 then
        t = 0.0
        local p = pos(ctx)
        log(ctx, string.format(
            "pos=(%.2f, %.2f) grounded=%s vel=(%.2f, %.2f)",
            p.x, p.y, tostring(character_is_grounded(ctx)),
            character_velocity(ctx).x, character_velocity(ctx).z))
    end
    state_set(ctx, "log_t", t)
end
