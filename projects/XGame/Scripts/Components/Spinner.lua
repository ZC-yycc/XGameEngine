-- 绕 Y 轴匀速自转。
--
-- 本项目里它与 Rust 插件的 `Spinner` 组件**同值同款**（75°/s）——
-- 场景里两个实体各挂一个，跑起来应当完全一致。这是"一个运行时、
-- 两种写法"最直观的验收方式：同一个行为，Rust 组件与 Lua 脚本并存。
--
-- 顶层 `local` 是这份脚本共享的（同一脚本的所有实体看到同一个值），
-- 在函数体里天然可见 —— 像别的语言里的模块级静态。

local SPIN_SPEED = 75.0 -- 度 / 秒

function update(ctx)
    -- 帧率无关：乘以 delta 才是"每秒 75 度"，否则机器越快转得越快。
    local r = rot(ctx)
    set_rot(ctx, r.x, r.y + SPIN_SPEED * delta(ctx), r.z)
end
