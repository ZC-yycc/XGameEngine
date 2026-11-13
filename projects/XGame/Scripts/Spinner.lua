-- 绕 Y 轴匀速自转。
--
-- 演示脚本的基本形态：一个 `update` 钩子 + 读写自身变换。
-- 挂在实体上之后，它的 `rot` / `set_rot` 操作的就是那个实体本身。
--
-- 顶层 `local` 是这份脚本共享的（同一脚本的所有实体看到同一个值），
-- 在函数体里天然可见 —— 像别的语言里的模块级静态。

local SPIN_SPEED = 75.0 -- 度 / 秒

function update(ctx)
    -- 帧率无关：乘以 delta 才是"每秒 75 度"，否则机器越快转得越快。
    local r = rot(ctx)
    set_rot(ctx, r.x, r.y + SPIN_SPEED * delta(ctx), r.z)
end
