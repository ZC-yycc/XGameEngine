# 脚本 API 参考

> 语言是 [Lua 5.4](https://www.lua.org/manual/5.4/)（经 mlua 嵌入，`vendored` 随引擎编译）。
> 脚本放在项目的 `Scripts/` 目录（`.lua`），实体用**项目相对路径**引用它。
>
> **机器可读清单**：`xgame-editor --export-script-api [--out api.json]`，或 MCP
> `script_api_read`。本文件的 179 个函数名与清单由测试钉住；参数 / 返回值 /
> 全局约定以清单为准，本文件继续承担叙述、示例与易错点说明。

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

七个钩子，**全部可选**（没定义就不调用，不需要写空函数）。

| 钩子 | 时机 | 典型用途 |
|---|---|---|
| `function start(ctx)` | 实体进入运行时后**一次** | 初始化状态、找别的实体 |
| `function update(ctx)` | **每个渲染帧**一次 | 视觉、UI、非物理逻辑 |
| `function fixed_update(ctx)` | **每个物理固定步长**一次（60 Hz） | 与物理交互的逻辑 |
| `function on_collision_enter(ctx, other)` | 与另一个实体**开始**接触时一次 | 拾取、伤害、触发器 |
| `function on_collision_exit(ctx, other)` | **结束**接触时一次 | 离开触发区、停止持续伤害 |
| `function on_anim_event(ctx, name)` | 基础通道剪辑跨过事件时刻时一次 | 脚步声、打击判定、特效挂点 |
| `function on_destroy(ctx)` | 实体被销毁**之前**一次 | 清理、生成特效 |

`name` 是**动画事件名**（`animator.events` 里写的字符串）。事件表挂在 `Animator`
上，时间按**基础通道当前剪辑**的本地时间轴解释：换剪辑 / 换状态时归零重新计时；
循环剪辑每圈到点各来一次，一帧跨多个事件时按时间轴顺序派发。分层与叠加层不触发。
静帧（`autoplay = false`）不派发；速度为 0 的剪辑在**起步那一帧**仍会派发
`time = 0` 的事件。

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
| `set_rot(ctx, x, y, z)` | 设置局部欧拉角（度，顺序同 `rot`：Y-X-Z；与 Inspector / MCP 同一约定） |
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
| `spawn_prefab(ctx, path, x, y, z)` | **实例化一个预制体资产**，返回实例根 ID（失败返回 `nil`） |
| `spawn_prefab_under(ctx, path, parent, x, y, z)` | 同上，但把实例根挂到 `parent` 底下，坐标为**局部** |

> 返回的 ID **立刻可用** —— `local id = spawn_entity(...)` 之后马上
> `set_entity_pos(ctx, id, ...)` 是生效的。

> 函数名带 `_entity` 后缀是历史命名（早期脚本语言把 `spawn` 保留成了关键字），
> 现在保持不变以免破坏既有脚本。

```lua
-- 敌人 / 子弹 / 掉落物：预制体里有什么就生成什么（层级 / 变换 / 物理 /
-- 粒子 / 音频 / 相机 / UI 全带上），子节点相对位置原样保留。
local e = spawn_prefab(ctx, "Prefabs/Enemy.prefab.ron", 0.0, 1.0, 0.0)
if e ~= nil then
    set_entity_pos(ctx, e, 4.0, 1.0, 0.0)      -- 立刻可用
else
    log(ctx, "生成失败：预制体读不到（见 error 日志）")
end

-- 挂在玩家手上的子弹：跟着玩家走，不用每帧自己追。
spawn_prefab_under(ctx, "Prefabs/Bullet.prefab.ron", self_id(ctx), 0.0, 1.5, 0.0)
```

- **位置语义**：`spawn_prefab` 的坐标**覆盖**根的**世界**位置（Unity
  `Instantiate(prefab, pos, rot)` 同款）；`spawn_prefab_under` 的坐标是**局部**
  偏移。两者都不会叠加到预制体自己写的位置上。
- **失败三种，日志分开说**：宿主没接装载器（`World` 没 `with_scene_loader`，
  只有自建宿主 / 单测会碰到）、文件读不到或 RON 坏、预制体是空的。三种都返回
  `nil` 并留一条日志 —— 不静默、不生成半个实例。
- 每个新实体的刚体由运行时自动补建（与 `spawn_ball` 同一条路径），脚本侧
  不需要任何额外调用。

### 4.4 物理

| 函数 | 说明 |
|---|---|
| `has_physics(ctx, id)` | 这个实体有没有刚体 |
| `raycast(ctx, ox, oy, oz, dx, dy, dz, max_dist)` | 射线检测，返回一个表 |
| `raycast_masked(ctx, ox, oy, oz, dx, dy, dz, max_dist, layer_mask)` | 只命中 `layer_mask` 里那些层（第 N 位 = 第 N 层）；**不看碰撞矩阵** |
| `set_layer(ctx, id, layer)` | 把实体换到某一层（0–31，越界取模）；碰撞组当场改写 |
| `layer(ctx, id)` | 实体所在的层；实体不存在返回 `nil` |
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

**层掩码**（`raycast_masked`）只筛"目标在哪一层"，与碰撞矩阵无关：摄像机射线
只打几何体层时，即便几何体层和摄像机层被设成互不碰撞，射线照样打得中 —— 矩阵
管接触，查询掩码管"我想看什么"。掩码是 32 位整数，第 N 位为 1 表示"包含第 N 层"：

```lua
-- 只看第 1、3 层（0b1010）
local r = raycast_masked(ctx, p.x, p.y, p.z, 0.0, -1.0, 0.0, 5.0, 0xA)
```

`set_layer` 改的是**实体属性**：场景里 `layer` 字段立刻写入，物理侧碰撞组由运行时
消费 `Relayered` 通知后立刻改写（同一帧内，不必等下一帧）。层号与"谁和谁碰"的
矩阵在 `project.ron` 的 `layers` 节，编辑器「⚙ 项目设置 → 层与碰撞矩阵」可改。

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
| `pad_button_down(ctx, "south")` | 手柄按键是否按住（任意一台；`south`/`east`/`west`/`north`、`left_bumper`、`left_trigger`、`dpad_up`…） |
| `pad_axis(ctx, "left_x")` | 手柄轴：摇杆 `left_x`/`left_y`/`right_x`/`right_y` 为 -1…1（`+y` 向上），扳机 `left_trigger`/`right_trigger` 为 0…1 |
| `pad_connected(ctx)` | 是否有手柄连着 |
| `touch_count(ctx)` | 当前按着的手指数量 |
| `touch(ctx, id)` | 触点表 `{x, y, force, phase}`（`phase` = `started`/`moved`/`ended`/`cancelled`）；不在时返回 nil。**第一根手指同时驱动鼠标**，`ui_*` 按钮在触摸屏上照常可点 |
| `key_down(ctx, "W")` / `key_pressed(ctx, "W")` | 直接按物理键位查询 |
| `mouse_down(ctx, "Left")` / `mouse_pressed(ctx, "Left")` | 鼠标按键 |
| `mouse_pos(ctx)` / `mouse_delta(ctx)` / `scroll(ctx)` | 指针状态 → `{x, y}` 表 |

动作名来自项目根目录的 **`input.ron`**（见下一节）。改按键不用改脚本。

### 4.6 时间与日志

| 函数 | 说明 |
|---|---|
| `delta(ctx)` | 到上一帧的时间（秒，**已按时间缩放**；暂停时恒为 0） |
| `time(ctx)` | 游戏已运行的总时长（秒，同样已缩放；暂停时不增长） |
| `time_scale(ctx)` | 当前时间缩放系数（默认 `1.0`） |
| `set_time_scale(ctx, scale)` | 设置时间缩放；`0` = 暂停模拟，`0.3` = 慢动作，`2.0` = 二倍速 |
| `log(ctx, msg)` / `warn(ctx, msg)` / `error(ctx, msg)` | 写进编辑器 Console |

#### 时间缩放（`set_time_scale`）

一次设置，**物理固定步 / `delta` / `time` / 粒子模拟 / 动画钟**同帧同源变慢，
和 Unity `Time.timeScale` 一个意思：

```lua
-- 按 Esc 暂停：模拟停住，但 update 仍然在跑（所以还能按键恢复）
function update(ctx)
    if key_pressed(ctx, "Escape") then
        set_time_scale(ctx, 1.0 - time_scale(ctx))   -- 0 ↔ 1 切换
    end
end
```

四条语义要记清：

1. **`0` 只是停模拟，不是停引擎**：`update` 照旧每帧调用、`delta` 恒为 0，
   而 `fixed_update` **一次都不调用**（没有固定步可跑，物理也不步进）。
   所以暂停菜单的输入、UI、`load_scene` 都还能用 —— 这就是暂停菜单能工作的原因。
   与编辑器 Play 工具栏的 Pause 不同：那个是干脆不推进整个 `World`（连 `update`
   都不跑），属于调试器语义。
2. **生效在下一帧**：本帧的固定步循环已经跑完，写进去的系数从下一个 `update` 起算。
3. **不动音频与 UI**：音频走自己的时钟，UI 交互不受影响。想暂停 BGM 请显式调
   `stop_sound`。
4. **口径是"游戏时间"**：暂停菜单里要做呼吸动画这类"照常走"的效果，别用 `time(ctx)`，
   自己累加未缩放的墙钟差值。

非法值处理：负值钳到 `0`，超过 `100` 钳到 `100`，`NaN` / `∞` 直接忽略（保住上一档，
不会把游戏静默冻住）。

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

### 4.9 动画：剪辑切换、状态机与叠加层

| 函数 | 说明 |
|---|---|
| `set_anim_clip(ctx, clip)` | 直接切**基础层**剪辑（走与 Inspector / RON 相同的交叉淡入路径） |
| `set_anim_speed(ctx, speed)` | 基础层倍率（钳在 0~100；0 = 冻结；状态机模式下由状态表覆盖） |
| `set_anim_looping(ctx, looping)` | 基础层是否循环（状态机模式下由状态表覆盖） |
| `set_anim_fade_time(ctx, seconds)` | 换剪辑 / 换状态的淡入时长（钳在 0~10；0 = 硬切） |
| `set_anim_param(ctx, name, value)` | 写**自己实体**的动画状态机参数（键不存在就创建） |
| `set_anim_additive_clip(ctx, clip)` | 切叠加层剪辑（硬切；没配叠加层 = no-op） |
| `set_anim_additive_weight(ctx, value)` | 写**自己实体**叠加层的权重（自动钳在 0~1） |

**直接控制 vs 状态机**：配了可用状态机（`Animator.machine` 的 entry 指得到状态）
时，`clip` / `speed` / `looping` 由状态表决定 —— 前四个 setter 仍会写字段，但
实际播放不跟着变（与 Inspector 的语义一致）。要让脚本驱动状态机，用
`set_anim_param` 推转移条件；要直接切剪辑，先把机器配置去掉。

`set_anim_clip` 只改**基础层剪辑**，不碰 `autoplay` / `speed` / `looping`，所以
`autoplay = false` 的实体会切完仍停在第一帧。淡入时长统一由 `fade_time` 决定
（`set_anim_fade_time` 写的就是它）：下一帧动画钟看到剪辑名变化，就从旧剪辑交叉
淡入；`fade_time = 0` 或切换时没在播 → 硬切。这是一条**与编辑器 / RON 完全同源**
的路径，脚本层不自己算混合。

所有 setter 只作用于**调用者自己**的实体；没挂动画组件 → 静默 no-op（通用移动
脚本可以挂在任意实体上，不用先探测组件）。`speed` / `fade_time` 的越界值会被钳住，
非有限值（NaN / inf）直接忽略。

```lua
-- 没有状态机时直接切：进门播 Attack（不循环、淡入 0.1s），0.8 秒后回 Idle。
-- 配了状态机时请改用 set_anim_param 驱动转移（直接 setter 会被状态表覆盖）。
function start(ctx)
    set_anim_fade_time(ctx, 0.1)
    set_anim_looping(ctx, false)
    set_anim_clip(ctx, "Attack")
end

function update(ctx)
    local t = state_get_or(ctx, "attack_t", 0.0) + delta(ctx)
    state_set(ctx, "attack_t", t)
    if t >= 0.8 then
        set_anim_looping(ctx, true)
        set_anim_clip(ctx, "Idle")
        state_set(ctx, "attack_t", 0.0)
    end
end
```

状态机（`Animator.machine`）的转移求值在引擎侧每帧自动跑；脚本只负责把
"现在多快 / 要不要攻击"这类**信号**推进参数表。没挂动画组件或没配机器时
`set_anim_param` **静默无操作** —— 通用移动脚本可以挂在任意实体上，不用先探测组件。

叠加层（`Animator.additive`）是叠在基础姿势上的一层**增量**（增量 = 叠加剪辑
当前帧 − 它的第 0 帧），`weight` 决定幅度：`set_anim_additive_weight` 同样在
没配叠加层时**静默无操作**。经典用法是"受击晃动淡入再淡出"：平时 0，挨打瞬间
拉到 1 再随时间衰减回去。`set_anim_additive_clip` 换叠加剪辑是**硬切**（叠加层
没有交叉淡入）；想平滑就先拉权重到 0、换好再拉回。

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
| `set_anim_layer_clip(ctx, name, clip)` | 按层名切该层剪辑（硬切；层名不存在 = no-op） |
| `set_anim_layer_param(ctx, layer, name, value)` | 写指定混合层自己的状态机参数（只影响该层；机器不存在 = no-op） |

### 4.11 形变目标（morph target）

| 函数 | 说明 |
|---|---|
| `morph(ctx, id, target)` | 读实体第 `target` 个形变目标的权重；没有这个目标返回 `0` |
| `set_morph(ctx, id, target, value)` | 写形变目标权重（钳在 0~1，可稀疏写；返回是否写入） |

形变目标（blend shape）是顶点级的表情 / 形变：`0` = 基准形状，`1` = 完全变成
那个目标。`target` 是**下标**，顺序与模型里 `targets[]` 一致（Inspector 的 Morph
区块按同样的顺序列出名字，照那边数）。

- **钳位**：写 `5.0` 存 `1.0`，`NaN` / `inf` 直接拒绝（返回 `false`），
  越界下标（≥256）也拒绝 —— 脚本算错不该把场景文件撑爆。
- **稀疏写**：`set_morph(ctx, id, 5, 1.0)` 会把 0..5 补成 0，其余表达式不用先
  把整张表铺平。
- **立刻生效**：渲染每帧读场景里的权重，不走命令队列（与 `set_layer` 不同）。
- **动画优先**：剪辑里带权重轨道时（口型 / 表情动画），动画值**覆盖**这里写的
  值 —— 与 Unity 一致，被动画驱动的属性不听手动覆盖。要"动画之上再叠一层"，
  把这个目标做成单独的剪辑，别指望手动相加。

```lua
-- 受击时闭眼 0.2 秒（假设模型的目标 0 是 blink）
function start(ctx)
    state_timer = 0
end

function update(ctx)
    state_timer = state_timer + delta(ctx)
    set_morph(ctx, self_id(ctx), 0, state_timer < 0.2 and 1.0 or 0.0)
end
```

层栈（`Animator.layers`）里每层可以是 Additive（加增量）或 Override（整段覆盖），
配**骨骼遮罩**后就是"只管某个部位"的层（如只盖上半身、腿照走）。每层还可配
自己的 `machine`：参数、当前状态、播放时钟与交叉淡入都互相独立；基础机器管腿，
`upper` 机器可以单独在 Aim / Fire 间转移。脚本用
`set_anim_layer_param(ctx, "upper", "fire", 1)` 只推这一台机器，不会改基础层或别层。
脚本按层的
**名字**查找：重名按栈内顺序取第一个命中，层名不存在 / 没挂动画组件时**静默无操作**
（同 `set_anim_param`，通用脚本不需要先探测组件）。`set_anim_layer_clip` 只改
**没有可用机器的直接剪辑层**，并把该层时钟归零；配了可用机器时实际剪辑仍由
状态表覆盖。直接换层剪辑保持硬切；机器转移则按该层 `fade_time` 或逐条 `duration`
交叉淡入。

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
| `play_sound_bus(ctx, clip, bus, volume, pitch)` | 播一次非空间化音效并归入音量总线（`bus`：`sfx` / `bgm` / `voice`） |
| `play_sound_at_bus(ctx, clip, x, y, z, bus, volume)` | 播一次空间化音效并归入音量总线 |
| `set_bus_volume(ctx, bus, v)` | 设一条音量总线的音量（`0.0` 静音 ~ `1.0` 原音量），立即作用于该组**已在响**的声音 |
| `bus_volume(ctx, bus)` | 读一条音量总线的当前音量；总线名未知返回 `nil` |

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

**音量总线**：声音按用途分三组（`sfx` 音效 / `bgm` 背景音乐 / `voice` 对白），
一组一个旋钮。`set_bus_volume` 立即作用于该组**已在响**的声音（游戏设置菜单里
"音乐音量"滑条要的就是这个）；主音量是所有组之上再乘一次。`AudioSource` 组件
在 Inspector 里也能选总线（缺省 `sfx`）。总线名大小写不敏感，`music` 是 `bgm`
的别名；未知名字会告警并**忽略**（不会悄悄归到 `sfx`）。

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

### 4.16 场景切换

| 函数 | 说明 |
|---|---|
| `current_scene(ctx)` | 当前场景的**项目相对路径**（如 `"Scenes/Level2.ron"`；`""` = 宿主没告诉运行时） |
| `load_scene(ctx, path)` | 请求切换到 `path`（项目相对路径）。**本帧末尾**才生效 |
| `scene_count(ctx)` | 发布清单里的关卡数（`0` = 没配清单） |
| `scene_index(ctx)` | 当前场景在清单里的下标（**0 起**；`nil` = 没配清单，或当前场景不在清单里） |
| `load_scene_at(ctx, i)` | 切到清单里的第 `i` 关（**0 起**）。越界记一条 error，场景不动 |

语义要点（这几条决定了它能不能用来做"菜单 → 关卡"）：

- **参数是项目相对路径**，不是场景的 `name`。写 `"Level2"` 会找不到文件并记一条
  错误日志 —— 不做"猜扩展名 / 猜目录"的模糊匹配，因为那会给 CI 与智能体埋不确定性。
- **不在调用点生效**。钩子执行期间场景正被可变借用，请求只是登记；`World::update`
  在**本帧末尾**统一应用（此时所有钩子已返回、固定步长循环也已结束）。
  新场景的 `start` 在**下一帧**才跑（与"脚本刚生成出来的实体"同一时机）。
- **切换会重置与实体绑定的运行时状态**：物理刚体、粒子、音频 autoplay 记录、
  UI 悬停/按下捕获、组件的 `start` 标记、每实体 `state_*`。脚本的**模块级全局
  变量保留**（Unity 不重载 domain 的语义），"随实体走"的数据请用 `state_*` 或
  存进场景。
- **一帧只切一次**：同一帧多次请求，只有最后一个生效。
- **装载失败不换场景**，记一条 error 日志后继续跑旧场景 —— 路径写错不该让游戏停摆。
  `--headless` 会把这条错误计入 `logs.error` 并用退出码 2 报出来。
- **运行期切换不写项目文件**：编辑器 Play 里切过去的场景在 Stop 时随
  `play_snapshot` 一起丢弃，磁盘上的场景文件一个字节都不动。

### 关卡顺序：把"下一关"变成数据

`project.ron` 的 `build.scenes` 是一份**有序**清单（`⚙ 项目设置 → 构建` 里上下移
就能改），顺序即关卡索引。于是"进下一关"不需要在每张关卡的脚本里写死路径：

```lua
function update(ctx)
    if reached_goal and key_pressed(ctx, "Confirm") then
        local i = scene_index(ctx)          -- 当前是第几关（0 起）
        if i then load_scene_at(ctx, i + 1) end   -- 下一关；越界会记 error
    end
end
```

几条必须知道的语义：

- **下标 0 起**，与 `load_scene_at` 直接配对（`scene_index(ctx) + 1` 就是下一关）。
  Lua 惯例是 1 起，这里故意不折 —— 折一次就要在每个调用点 `+1 / -1` 往返。
- **未配置清单时 `scene_count` 是 0、`scene_index` 是 `nil`**，`load_scene_at` 任何
  下标都算越界。单场景项目不要指望拿到 `0`。
- **入口场景可以不在清单里**（比如入口是个"启动器"场景），此时它的
  `scene_index` 也是 `nil`。`--check` 只提示不报错。
- 顺序**不会自动推进**：它只是数据，"什么时候进下一关"永远由脚本决定。
- 越界（比如最后一关 `i + 1`）记一条 **error** 日志、场景原地不动 —— 别让它
  表现成"卡在关底不动"。

菜单 → 关卡的最小写法：

```lua
-- Scripts/Menu.lua（挂在按钮所在实体上）
function start(ctx)
    log(ctx, "当前场景：" .. current_scene(ctx))
end

function update(ctx)
    if key_pressed(ctx, "Confirm") then
        load_scene(ctx, "Scenes/Level1.ron")
    end
end
```

> 宿主必须接入**场景装载器**才会真的换场景（独立游戏 / 编辑器 Play / 无头运行
> 都已接）。没接的运行时（例如单元测试直接 `World::new`）会记一条
> "宿主没有接入场景装载器"的错误 —— 静默失败会让人以为脚本写错了。

### 4.17 存档与变量（SaveGame）

| 函数 | 说明 |
|---|---|
| `save_game(ctx, slot)` | 请求存档到 `Saves/<slot>.sav`。**本帧末尾**才写盘 |
| `load_game(ctx, slot)` | 请求读档。**本帧末尾**生效；存档场景与当前不同时先切场景 |
| `has_save(ctx, slot)` | 这个槽有没有存档（立即判，不加载） |
| `save_slots(ctx)` | 全部槽名（升序数组式表，`#slots` 可用） |
| `save_set(ctx, key, v)` / `save_get(ctx, key)` / `save_remove(ctx, key)` | 全局变量 |
| `entity_save_set(ctx, id, k, v)` / `entity_save_get(ctx, id, k)` | 每实体变量 |

**存什么**：每个实体的局部变换 + 刚体速度，加上你自己写进来的变量。**不存**脚本
`state_*`（那是 Lua 值，可能是表 / 函数）、不存时钟、也不"复活"存档之后才生成的
实体 —— 读档是"把现在这些实体的状态拨回去"，不是"重建那一刻"。

```lua
-- 存档：血量这种玩法数据自己写进变量，引擎不知道它的语义
function save_now(ctx)
    local me = self_id(ctx)
    entity_save_set(ctx, me, "hp", 75)
    save_set(ctx, "coins", 12)
    save_set(ctx, "checkpoint", "gate")
    save_game(ctx, "slot1")
end

-- 读档：变量会在**同一帧末尾**恢复，下一帧的钩子里就能读到
function load_now(ctx)
    load_game(ctx, "slot1")
end

function start(ctx)
    local hp = entity_save_get(ctx, self_id(ctx), "hp")
    if hp then set_health(hp) end
end
```

语义要点：

- **请求式**：`save_game` / `load_game` 只登记，帧末统一结算（与 `load_scene`
  同一时机）。所以"存完立刻读回来验证"要等下一帧 —— 用 `has_save` 判存在性。
- **变量只收数字 / 布尔 / 字符串**，其它类型 `save_set` 回 `false` 且不写。
  `NaN` / `∞` 一律拒绝（读回来会让物理与渲染出怪）。
- **全局变量跨场景保留**（与脚本全局变量同款语义），**实体变量跟着实体走**：
  切场景、实体销毁时清空。
- **读档是替换不是合并**：存档里没有的键会被清掉（否则"读档后血条显示上一次的数"）。
- **实体对不上就跳过**，数量写进日志（`跳过 N 个`）和无头报告；不会生成新实体。
- 存档路径同样**只认项目相对路径**：`slot` 里写 `../x`、盘符、`a:b`、`CON`
  会被拒绝（错误日志），`slot1` 与 `slot1.sav` 落到同一个文件。

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
