-- 触发区：传感器不挡路，但角色走进去会收到 on_collision_enter。
function on_collision_enter(ctx, other)
    log(ctx, "进入触发区：" .. entity_name(ctx, other))
end

function on_collision_exit(ctx, other)
    log(ctx, "离开触发区：" .. entity_name(ctx, other))
end
