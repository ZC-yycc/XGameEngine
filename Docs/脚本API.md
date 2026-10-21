# 脚本 API 参考

> 语言是 [Lua 5.4](https://www.lua.org/manual/5.4/)（经 mlua 嵌入，`vendored` 随引擎编译）。
> 脚本放在项目的 `Scripts/` 目录（`.lua`），实体用**项目相对路径**引用它。

---

## 一、最小示例

```lua
-- Scripts/Spinner.lua
local SPIN_SPEED = 75.0   -- 度 / 秒

function update(ctx)
    local r = rot(ctx)
    set_rot(ctx, r.x, r.y + SPIN_SPEED * delta(ctx), r.z)
end
```

挂到实体上：编辑器里选中实体 → Inspector 的 **Scripts** 区 → `+ Add Script`
（或直接改场景文件里的 `scripts: ["Scripts/Spinner.lua"]`）。

---

## 二、生命周期钩子

六个钩子，**全部可选**（没定义就不调用，不需要写空函数）。

| 钩子 | 时机 | 典型用途 |
|---|---|---|
| `function start(ctx)` | 实体进入运行时后**一次** | 初始化状态、找别的实体 |
| `function update(ctx)` | **每个渲染帧**一次 | 视觉、UI、非物理逻辑 |
| `function fixed_update(ctx)` | **每个物理固定步长**一次（60 Hz） | 与物理交互的逻辑 |
| `function on_collision_enter(ctx, other)` | 与另一个实体**开始**接触时一次 | 拾取、伤害、触发器 |
| `function on_collision_exit(ctx, other)` | **结束**接触时一次 | 离开触发区、停止持续伤害 |
| `function on_destroy(ctx)` | 实体被销毁**之前**一次 | 清理、生成特效 |

`other` 是**对方的实体 id**（整数，与 `self_id()` / `find_entity()` 同一种值）。
碰撞事件**双方各收一次**（Unity 的 `OnCollisionEnter` 同款语义），持续接触
不会每帧刷。对瞬时的"穿过就触发"，在 `on_collision_enter` 里判断
`entity_name(ctx, other)` 即可。

> `update` 的 `delta(ctx)` 是**真实帧间隔**，`fixed_update` 的 `delta(ctx)` 是
> **固定步长**。做匀速运动请在 `update` 里乘 `delta`；做物理相关的请在
> `fixed_update` 里做。

---

## 三、两个必须知道的语义

### 1. 顶层 `local` 是**脚本级共享**的

```lua
local speed = 5.0          -- 同一份脚本的所有实体看到同一个值（模块级静态）
```

顶层 `local` 在函数体里天然可见（upvalue），也可以被函数改写 —— 但它是
**这份脚本共享**的，不是每实体一份。想给"每个实体独立"的可变状态，
用下一节的 `state_*`。

### 2. 想存"每实体的状态"用 `state_*`，别用顶层变量

计时器 / 冷却 / 血量这类**每实体**可变状态必须走这组 API：

```lua
function update(ctx)
    -- 冷却：每 0.5 秒才能开一枪
    local cd = state_get_or(ctx, "cooldown", 0.0) - delta(ctx)
    state_set(ctx, "cooldown", cd)

    if cd <= 0.0 and action_pressed(ctx, "Fire") then
        log(ctx, "bang!")
        state_set(ctx, "cooldown", 0.5)
    end
end
```

`state_*` 存的东西按 **`(实体, 脚本)` 分开** —— 同一个脚本挂在 10 个实体上，
每个实体有自己独立的一份状态。

---

## 四、API 一览

所有函数都把 `ctx` 作为**第一个参数**。

### 4.1 自身变换

| 函数 | 说明 |
|---|---|
| `pos(ctx)` | 自身**世界**坐标 → `{x, y, z}` 表 |
| `set_pos(ctx, x, y, z)` | 设置**世界**坐标（有父节点时自动换算回局部） |
| `local_pos(ctx)` / `set_local_pos(ctx, x, y, z)` | **局部**坐标 |
| `translate(ctx, dx, dy, dz)` | 世界空间平移增量 |
| `rot(ctx)` | 自身**局部**欧拉角（度），顺序 Y-X-Z → `{x, y, z}` 表 |
| `set_rot(ctx, x, y, z)` | 设置局部欧拉角（度） |
| `rotate(ctx, dx, dy, dz)` | 叠加旋转增量（度） |
| `scale(ctx)` / `set_scale(ctx, x, y, z)` | 局部缩放 |
| `forward(ctx)` / `right(ctx)` / `up(ctx)` | 世界空间的朝向向量 |
| `self_id(ctx)` / `self_name(ctx)` | 自身标识 |

> **位置用世界、旋转用局部**：位置几乎总在"摆到哪里"的语境里（父节点已经摆好了），
> 旋转几乎总在"这个物体自己转多少"的语境里。

### 4.2 其它实体

| 函数 | 说明 |
|---|---|
| `entity_count(ctx)` | 场景实体数 |
| `entity_at(ctx, index)` | 索引 → 实体 ID |
| `entity_id(ctx, name)` | 名字 → 实体 ID（找不到返回 `0`） |
| `entity_name(ctx, id)` / `entity_model(ctx, id)` | 查询 |
| `set_entity_model(ctx, id, path)` | 换模型（`"builtin://cube"` 或项目相对路径） |
| `entity_pos(ctx, id)` / `set_entity_pos(ctx, id, x, y, z)` | 世界坐标 |
| `entity_rot(ctx, id)` / `set_entity_rot(ctx, id, x, y, z)` | 局部欧拉角（度） |
| `entity_forward(ctx, id)` | 该实体世界朝向的 +Z（绕到背后 / 判断"它背没背对我"） |
| `entity_scale(ctx, id)` / `set_entity_scale(ctx, id, x, y, z)` | 局部缩放 |
| `distance_to(ctx, id)` | 与目标的距离 |
| `set_parent(ctx, child, parent)` | 重挂父节点（`parent = 0` 表示回到根） |
| `destroy(ctx, id)` | 销毁实体**及其整棵子树** |

### 4.3 生成实体

| 函数 | 说明 |
|---|---|
| `spawn_entity(ctx, name, model)` | 生成一个纯视觉实体，返回它的 ID |
| `spawn_entity_at(ctx, name, model, x, y, z)` | 同上，指定位置 |
| `spawn_ball(ctx, name, radius, x, y, z)` | 生成一个动态球（带刚体与球碰撞体） |
| `spawn_static_box(ctx, name, hx, hy, hz, x, y, z)` | 生成一个静态盒子 |

> 返回的 ID **立刻可用** —— `local id = spawn_entity(...)` 之后马上
> `set_entity_pos(ctx, id, ...)` 是生效的。

> 函数名带 `_entity` 后缀是历史命名（早期脚本语言把 `spawn` 保留成了关键字），
> 现在保持不变以免破坏既有脚本。

### 4.4 物理

| 函数 | 说明 |
|---|---|
| `has_physics(ctx, id)` | 这个实体有没有刚体 |
| `raycast(ctx, ox, oy, oz, dx, dy, dz, max_dist)` | 射线检测，返回一个表 |
| `apply_force(ctx, id, x, y, z)` | 施力（持续，下一帧清零） |
| `apply_impulse(ctx, id, x, y, z)` | 施冲量（瞬间改速度） |
| `add_torque(ctx, id, x, y, z)` | 施加力矩 |
| `velocity(ctx, id)` / `set_velocity(ctx, id, x, y, z)` | 线速度 |
| `gravity(ctx)` / `set_gravity(ctx, x, y, z)` | 全局重力 |

`raycast` 返回的表字段：`hit`(bool)、`point`(`{x,y,z}`)、`normal`(`{x,y,z}`)、
`distance`(float)、`entity`(整数，0 表示没打中实体)、`is_sensor`(bool)。
**永远有全部字段**（没打中时 `hit = false`、其余为零值），所以脚本不必先判空。

```lua
function update(ctx)
    -- 脚下 0.6 米内有没有地？
    local p = pos(ctx)
    local r = raycast(ctx, p.x, p.y, p.z, 0.0, -1.0, 0.0, 0.6)
    if not r.hit then
        log(ctx, "踩空了")
    end
end
```

> `raycast` 会自动忽略**传感器**，并排除自身刚体（避免从自己内部出发打到自己）。

### 4.4.1 角色控制器

给实体挂上 `CharacterController`（场景里 `controller` 字段，或在编辑器
Inspector 的 **Character** 区块勾选）之后，用这组 API 驱动它：

| 函数 | 说明 |
|---|---|
| `character_simple_move(ctx, vx, vy, vz)` | **最常用**：给水平速度（米/秒），重力、坡面跟随、陡坡滑落由引擎负责 |
| `character_move(ctx, dx, dy, dz)` | 按**位移**移动（米），不施加重力；竖直速度自己管 |
| `character_jump(ctx, speed)` | 着地时起跳（米/秒），返回是否成功；空中调用返回 `false`（无二段跳） |
| `character_velocity(ctx)` | 角色当前速度 → `{x, y, z}`（**实际**速度：撞墙后水平分量归零） |
| `character_set_velocity(ctx, x, y, z)` | 直接改速度（击退 / 传送带） |
| `character_is_grounded(ctx)` | 是否着地 |
| `character_ground_normal(ctx)` | 脚下法线 → `{x, y, z}`（腾空时是 `{0,1,0}`） |
| `character_is_sliding(ctx)` | 是否正在陡坡上自动下滑 |

最小用法：

```lua
function fixed_update(ctx)
    local x = axis(ctx, "MoveX")
    local z = axis(ctx, "MoveZ")
    character_simple_move(ctx, x * 5.0, 0.0, z * 5.0)

    if action_pressed(ctx, "Jump") then
        character_jump(ctx, 5.0)
    end
end
```

> **坐标约定**：实体原点是角色的**脚底**（不是胶囊中心）。所以把一个角色
> 拖到 y = 0，它就正好站在地平面上。
>
> **想从上方俯视/第三人称跟随**：读 `pos(ctx)` 拿到的是脚底位置，
> 相机目标要自己加约 1.6m（身高）。

### 4.5 输入

| 函数 | 说明 |
|---|---|
| `action_down(ctx, "Jump")` | 动作是否按住 |
| `action_pressed(ctx, "Jump")` | 动作是否**本帧刚按下** |
| `action_released(ctx, "Jump")` | 动作是否**本帧刚松开** |
| `axis(ctx, "MoveX")` | 轴的值（-1 … 1） |
| `key_down(ctx, "W")` / `key_pressed(ctx, "W")` | 直接按物理键位查询 |
| `mouse_down(ctx, "Left")` / `mouse_pressed(ctx, "Left")` | 鼠标按键 |
| `mouse_pos(ctx)` / `mouse_delta(ctx)` / `scroll(ctx)` | 指针状态 → `{x, y}` 表 |

动作名来自项目根目录的 **`input.ron`**（见下一节）。改按键不用改脚本。

### 4.6 时间与日志

| 函数 | 说明 |
|---|---|
| `delta(ctx)` | 到上一帧的时间（秒） |
| `time(ctx)` | 游戏已运行的总时长（秒） |
| `log(ctx, msg)` / `warn(ctx, msg)` / `error(ctx, msg)` | 写进编辑器 Console |

### 4.7 每实体状态

| 函数 | 说明 |
|---|---|
| `state_get(ctx, key)` | 取值；没设过返回 `nil`（用 `== nil` 判空） |
| `state_get_or(ctx, key, default)` | 取值，缺省时返回 `default` |
| `state_set(ctx, key, value)` | 写入 |
| `state_has(ctx, key)` / `state_remove(ctx, key)` / `state_clear(ctx)` | 其它操作 |

### 4.8 数学

| 函数 | 说明 |
|---|---|
| `vec3(x, y, z)` / `vec2(x, y)` | 构造向量（Lua 表 `{x=, y=, z=}`） |
| `vlength(v)` / `vlength_squared(v)` / `vnormalized(v)` | 向量长度 / 归一化 |
| `vdot(a, b)` / `vcross(a, b)` / `vdistance_to(a, b)` | 向量运算 |
| `vadd(a, b)` / `vsub(a, b)` / `vmul(v, s)` | 加减 / 标量乘 |
| `vlerp(a, b, t)` | 插值 |
| `vtostring(v)` | 格式化成字符串（调试用） |
| `v.x` / `v.y` / `v.z` | 分量（可读写） |
| `clamp(v, lo, hi)` / `lerpf(a, b, t)` | 数值工具 |
| `deg_to_rad(d)` / `rad_to_deg(r)` | 角度换算 |

> 向量是普通 Lua 表，**没有 `a + b` 运算符重载** —— 算术走上面的 `v*` 具名函数，
> 或取分量做标量运算（`p.x + 1.0`）。Lua 标准库的 `math.sin` / `math.cos` /
> `math.sqrt` / `math.abs` / `math.min` / `math.max` 也可直接用。

### 4.9 动画状态机与叠加层

| 函数 | 说明 |
|---|---|
| `set_anim_param(ctx, name, value)` | 写**自己实体**的动画状态机参数（键不存在就创建） |
| `set_anim_additive_weight(ctx, value)` | 写**自己实体**叠加层的权重（自动钳在 0~1） |

状态机（`Animator.machine`）的转移求值在引擎侧每帧自动跑；脚本只负责把
"现在多快 / 要不要攻击"这类**信号**推进参数表。没挂动画组件或没配机器时
`set_anim_param` **静默无操作** —— 通用移动脚本可以挂在任意实体上，不用先探测组件。

叠加层（`Animator.additive`）是叠在基础姿势上的一层**增量**（增量 = 叠加剪辑
当前帧 − 它的第 0 帧），`weight` 决定幅度：`set_anim_additive_weight` 同样在
没配叠加层时**静默无操作**。经典用法是"受击晃动淡入再淡出"：平时 0，挨打瞬间
拉到 1 再随时间衰减回去。

```lua
-- 速度驱动 Idle/Walk/Run（阈值见场景 RON 里的 transitions），
-- 叠加层权重按 sin 波起伏（走动时的"呼吸/弹跳幅度"看得见地变化）
function update(ctx)
    set_anim_param(ctx, "speed", vlength(velocity(ctx, self_id(ctx))))
    set_anim_additive_weight(ctx, 0.3 + 0.3 * math.sin(time(ctx) * 4.0))
end
```

示例见 `projects/SampleScene/Scripts/FoxLocomotion.lua`。

### 4.10 混合层栈

| 函数 | 说明 |
|---|---|
| `set_anim_layer_weight(ctx, name, value)` | 写**自己实体**层栈中按层名匹配的层的权重（自动钳在 0~1） |

层栈（`Animator.layers`）里每层可以是 Additive（加增量）或 Override（整段覆盖），
配**骨骼遮罩**后就是"只管某个部位"的层（如只盖上半身、腿照走）。脚本按层的
**名字**查找：重名按栈内顺序取第一个命中，层名不存在 / 没挂动画组件时**静默无操作**
（同 `set_anim_param`，通用脚本不需要先探测组件）。

```lua
-- 走路段把 "upper" 层（Override + 遮罩 = 上半身）开到 0.6，其他段归 0 ——
-- 走路上半身张望、跑动时恢复自然摆臂（层配置见场景 RON 的 animator.layers）
function update(ctx)
    local t = time(ctx) % 7.5
    if t >= 2.5 and t < 5.0 then
        set_anim_layer_weight(ctx, "upper", 0.6)
    else
        set_anim_layer_weight(ctx, "upper", 0.0)
    end
end
```

示例见 `projects/SampleScene/Scripts/FoxLocomotion.lua`（按时间分段推
`"upper"` 层权重）。

### 4.11 音频

| 函数 | 说明 |
|---|---|
| `play_sound(ctx, clip, volume, pitch)` | 播一次**非空间化**音效（UI 音、提示音） |
| `play_sound_at(ctx, clip, x, y, z, volume)` | 播一次**空间化**音效（3D 定位，随距离衰减） |
| `stop_sound(ctx, clip)` | 停止某个 clip 的**全部**在播实例 |
| `stop_all_sounds(ctx)` | 停止所有声音（脚本播的 + 组件 autoplay 的） |
| `set_listener(ctx, x, y, z, fx, fy, fz)` | 设置听者位置与朝向（`fx/fy/fz` 是朝前向量） |
| `set_master_volume(ctx, v)` | 主音量（`0.0` 静音，`1.0` 原音量） |

`clip` 是**项目相对路径**（如 `"Resources/Audio/Beep.wav"`）。`volume` / `pitch`
都是 `1.0` 为基准（pitch 同时改速度与音高）。

```lua
-- 起跳给个一次性音效（pitch 拉高听起来更"弹"）
function update(ctx)
    if action_pressed(ctx, "Jump") then
        play_sound(ctx, "Resources/Audio/Beep.wav", 0.6, 1.5)
    end
end
```

> 没有音频设备时这些调用照样安全（引擎无声降级、照常受理），脚本不必探测。
> 听者默认跟随编辑器相机；纯脚本驱动的听者一般用在顶视角 / 固定机位玩法。
> 环境音 / 机关音更推荐挂 `AudioSource` 组件（Inspector 的 Audio 区块可视化配置），
> `play_sound` 系列适合"事件驱动"的一次性音效。

### 4.12 材质

材质分两层，互相正交：

- **共享资产**（`.mat`）：颜色 / 金属度 / 粗糙度 / albedo，跨实体复用；
- **实体覆盖**（`Entity.material`）：只对这个实例逐字段打补丁，在资产结果之上生效。

| 函数 | 说明 |
|---|---|
| `material_asset(ctx, id)` | 读实体引用的材质资产路径（没有 → 空串） |
| `set_material_asset(ctx, id, path)` | 指向一份 `.mat`（项目相对路径）；传 `""` 恢复模型自带材质 |
| `set_material(ctx, id, r, g, b, a, metallic, roughness)` | **一次性**写内联覆盖（颜色是线性 RGBA；金属度 / 粗糙度自动钳到 0..1） |
| `clear_material(ctx, id)` | 清掉内联覆盖（不影响共享资产） |

```lua
function start(ctx)
    local me = self_id(ctx)
    -- 复用项目里的材质资产（路径带 GUID 也行；建议写项目相对路径）
    set_material_asset(ctx, me, "Resources/Materials/Wood.mat")
    -- 只给这个实例压一点红 + 更粗糙
    set_material(ctx, me, 0.9, 0.35, 0.25, 1.0, 0.0, 0.8)
end
```

> 资产不存在不会报错：渲染侧退回模型材质，`--check` 会把悬空引用点名。
> 共享资产负责"这是什么材质"，覆盖负责"这个实例怎么微调"；调
> `set_material` 不会清掉 `set_material_asset` 设过的资产。

### 4.13 粒子

粒子是**声明 + 运行态请求**两层：发射器参数（`rate` / `lifetime` / `gravity` / ...）
在场景文件里，脚本只管"现在喷不喷、炸多少颗"。

| 函数 | 说明 |
|---|---|
| `particle_emitting(ctx, id)` | 读开关（没挂发射器 → false） |
| `set_particle_emitting(ctx, id, bool)` | 开 / 关持续发射 |
| `particle_burst(ctx, id, n)` | 立刻炸出 n 颗（可累积：连按两帧 = 两次相加） |
| `set_particle_rate(ctx, id, r)` | 改发射速率（颗/秒；非有限值钳 0） |

```lua
function start(ctx)
    local me = self_id(ctx)
    -- 默认持续喷；命中后改成一次性爆发
    set_particle_emitting(ctx, me, false)
end

function on_collision_enter(ctx, other)
    particle_burst(ctx, self_id(ctx), 30)  -- 撞一下炸一团火花
end
```

> 没挂发射器时全部静默 no-op（不会凭空创建组件）。`burst` 是**一次性**请求位：
> 模拟层当帧消费后清零，所以"按下开火键 → 这一帧看见火花"。参数在场景里调，
> 运行期改请改字段本身（`Entity::particle`）。

### 4.14 游戏 UI

UI 元素也是实体（`Entity::ui`），id 与其它实体通用。颜色参数是 **sRGB 0..1**
（引擎内部转线性）；坐标是**视口物理像素**，锚点是父矩形的归一化坐标（0..1）。

| 函数 | 说明 |
|---|---|
| `ui_set_text(ctx, id, text)` | 改文本 |
| `ui_text(ctx, id)` | 读文本（不存在返回空串） |
| `ui_set_text_size(ctx, id, size)` | 字号（像素） |
| `ui_set_text_color(ctx, id, r, g, b, a)` | 文字颜色 |
| `ui_set_text_align(ctx, id, "left"\|"center"\|"right")` | 水平对齐 |
| `ui_set_bg_color(ctx, id, r, g, b, a)` | 背景色 |
| `ui_set_radius(ctx, id, radius)` | 圆角半径 |
| `ui_set_visible(ctx, id, visible)` / `ui_visible(ctx, id)` | 显隐（连带整棵子树） |
| `ui_set_enabled(ctx, id, enabled)` / `ui_enabled(ctx, id)` | 是否接受点击（按钮禁用态半透明） |
| `ui_has(ctx, id)` | 这个实体是不是 UI 元素 |
| `ui_clicked(ctx, id)` | **本帧**完成了一次"在元素上按下并在元素上松开" |
| `ui_hovered(ctx, id)` | 本帧指针停在元素上 |
| `ui_set_rect(ctx, id, ax0, ay0, ax1, ay1, ox0, oy0, ox1, oy1)` | 一次设定锚点 + 偏移 |
| `ui_set_anchor(ctx, id, ax0, ay0, ax1, ay1)` | 只改锚点 |
| `ui_set_offset(ctx, id, ox0, oy0, ox1, oy1)` | 只改偏移 |
| `ui_set_size(ctx, id, w, h)` | 改尺寸（左上角不动） |
| `ui_set_pos(ctx, id, x, y)` | 改位置（相对父矩形左上角） |
| `ui_set_image(ctx, id, path)` | 换图片 |
| `ui_set_image_tint(ctx, id, r, g, b, a)` | 图片着色 |
| `ui_set_fill(ctx, id, t)` | 填充比例 `0..1`（血条：从左往右裁剪） |
| `ui_spawn_label(ctx, text, x, y, size)` | 生成文本，返回 id（父 = 屏幕根） |
| `ui_spawn_button(ctx, text, x, y, w, h)` | 生成按钮，返回 id |
| `ui_spawn_panel(ctx, x, y, w, h)` | 生成面板，返回 id |
| `ui_spawn_image(ctx, path, x, y, w, h)` | 生成图片元素，返回 id |

```lua
-- HUD：按钮点击计数 + 血条呼吸
local fill, count = 0, 0

function start(ctx)
    fill = entity_id(ctx, "Bar.Fill")
    count = entity_id(ctx, "Click.Count")
end

function update(ctx)
    if ui_clicked(ctx, self_id(ctx)) then
        ui_set_text(ctx, count, "点击: " .. tostring(os.time()))
    end
    local pulse = math.sin(time(ctx) * 2.2) * 0.5 + 0.5
    ui_set_size(ctx, fill, 4.0 + 300.0 * pulse, 14.0)
    ui_set_bg_color(ctx, fill, 0.95 - pulse * 0.6, 0.25 + pulse * 0.5, 0.28, 1.0)
end
```

> **点击语义**：按下之后指针被"捕获"——拖出元素再拖回来松开仍然算点击，
> 拖到别处松开才算取消（桌面按钮的惯例）。
>
> **本帧状态**：`ui_clicked` / `ui_hovered` 只在**当帧**为真，必须在 `update`
> 里读；`fixed_update` 里读会漏（固定步长一帧可能跑 0 次或多次）。
>
> **没有 UI 时**这一切都是空操作：`ui_set_*` 找不到实体就静默返回，
> 不报错（与其它"容错优先"的 API 一致）。

### 4.15 相机与朝向

相机是**普通实体组件**（`Entity::camera`），不是引擎特例：位置走 `set_pos` /
`set_entity_pos`，朝向走 `look_at`。宿主（独立游戏 / 编辑器 Play / 无头截图）
从同一处解析主相机，所以"你在这里看到的就是玩家看到的"。

| 函数 | 说明 |
|---|---|
| `look_at(ctx, x, y, z)` | 让**自己**的 +Z 指向世界坐标点（只改朝向、不改位置） |
| `look_at_entity(ctx, id)` | 让**自己**看向另一个实体（目标不存在则不动） |
| `active_camera(ctx)` | 当前主相机的实体 id（`0` = 场景里没有相机实体） |
| `set_active_camera(ctx, id)` | 把 `id` 设为**唯一**主相机（其它相机全部关掉） |
| `camera_fov(ctx, id)` | 垂直视场角（度）；不是相机 → `nil` |
| `set_camera_fov(ctx, id, deg)` | 改视场角（钳在 1..179），变焦自己按 `delta` 插值 |

语义要点：

- **`look_at` 只改朝向**。跟随相机是"先摆位置、再看向目标"两步；合成一个 API
  的话，相机每帧都会被拽到目标旁边。
- **相机本地的 +Z 是镜头朝向**（glTF 的模型正面也是 +Z，与 `forward(ctx)` 一致）。
  单位旋转的相机看向世界 +Z。
- **`set_active_camera` 是独占式的**：解析规则是"第一个 `active` 的相机"，
  只置 true 的话第二台永远轮不到。
- 相机**能被父节点带着走**：挂在角色下面就是肩视角 —— 位置自动跟随父层级，
  `look_at` 会把父节点的旋转一起抵消掉。

跟随相机的最小写法：

```lua
local OFFSET = { x = 4.2, y = 2.8, z = 3.6 }

function update(ctx)
    local target = entity_id(ctx, "Fox")
    if target == 0 then return end
    local p = entity_pos(ctx, target)
    set_pos(ctx, p.x + OFFSET.x, p.y + OFFSET.y, p.z + OFFSET.z)
    look_at(ctx, p.x, p.y + 1.0, p.z)   -- 看向胸口
end
```

完整样例见 `projects/SampleScene/Scripts/CameraFollow.lua`（场景里的 "Main Camera"
实体就是它驱动的，Play 态视口显示的正是这台相机）。

> **想让相机绕到角色背后**（真正的第三人称）：把偏移换成"朝向的反方向"——
> `local f = entity_forward(ctx, target)` 之后
> `set_pos(ctx, p.x - f.x * 5.0, p.y + 2.6, p.z - f.z * 5.0)`。
> 样例场景没用这个写法，因为正中有座箱塔会挡住视线（实测截图确认过）。

---

## 五、输入映射（`input.ron`）

放在**项目根目录**，编辑器 Play 与打包出去的游戏都会读它；没有就用引擎内置默认表。

```ron
InputMap(
    actions: {
        "Jump": [Key(Space)],
        "Fire": [Mouse(Left), Key(ControlLeft)],   // 一个动作可以绑多个键
    },
    axes: {
        "MoveX": (positive: [Key(KeyD)], negative: [Key(KeyA)]),
    },
)
```

- `Key(...)` 里是**物理键位**（与键盘布局无关），命名与 W3C `KeyboardEvent.code` 一致：
  `KeyW` / `Digit1` / `Space` / `ArrowUp` / `ShiftLeft` / `Numpad0` …
- `Mouse(...)`：`Left` / `Right` / `Middle` / `Back` / `Forward`
- `Combo(key: KeyS, ctrl: true)`：修饰键组合，四个修饰位是**精确匹配**
- `axes` 里一条轴的值 = `positive 是否按住` − `negative 是否按住`（-1 … 1），
  正负同时按住会**抵消为 0**

---

## 六、引擎的容错行为

脚本是用户内容，一个笔误不该让游戏挂掉：

| 情况 | 表现 |
|---|---|
| 语法错误 | 编译失败，Console 报错；该脚本整个不生效，游戏继续 |
| 顶层语句抛错 | 同上 |
| 运行时错误（如除零、索引 nil） | **这一帧**这个钩子不生效，Console 报错，游戏继续 |
| 死循环 / 运算爆炸 | 单次钩子调用的**指令预算**（20 万条）耗尽后被强制中断（记一条错误），游戏继续 |

**热重载**：Play 模式下点工具栏的 `⟳ Scripts`，全部脚本重新编译，不用重启。

---

## 七、和其它系统的分工

- **动态刚体由物理驱动**：`set_pos` 会被物理下一帧覆盖。
  想影响动态体用 `set_velocity` / `apply_impulse` / `apply_force`。
- **运动学刚体由脚本驱动**：`set_pos` / `set_rot` 每帧推给物理，它再去推动别人。
  想让脚本控制位置，就把实体的物理类型设成 `Kinematic`。
- **纯视觉实体**（`physics: None`）**不被物理碰**：脚本想怎么摆就怎么摆。
