# 脚本 API 参考

> 语言是 [Rhai](https://rhai.rs/)（类 Rust 语法的嵌入式脚本）。
> 脚本放在项目的 `Scripts/` 目录，实体用**项目相对路径**引用它。

---

## 一、最小示例

```rhai
// Scripts/Spinner.rhai
const SPIN_SPEED = 75.0;   // 度 / 秒

fn update(ctx) {
    let r = rot(ctx);
    set_rot(ctx, r.x, r.y + SPIN_SPEED * delta(ctx), r.z);
}
```

挂到实体上：编辑器里选中实体 → Inspector 的 **Scripts** 区 → `+ Add Script`
（或直接改场景文件里的 `scripts: ["Scripts/Spinner.rhai"]`）。

---

## 二、生命周期钩子

四个钩子，**全部可选**（没定义就不调用，不需要写空函数）。

| 钩子 | 时机 | 典型用途 |
|---|---|---|
| `fn start(ctx)` | 实体进入运行时后**一次** | 初始化状态、找别的实体 |
| `fn update(ctx)` | **每个渲染帧**一次 | 视觉、UI、非物理逻辑 |
| `fn fixed_update(ctx)` | **每个物理固定步长**一次（60 Hz） | 与物理交互的逻辑 |
| `fn on_destroy(ctx)` | 实体被销毁**之前**一次 | 清理、生成特效 |

> `update` 的 `delta(ctx)` 是**真实帧间隔**，`fixed_update` 的 `delta(ctx)` 是
> **固定步长**。做匀速运动请在 `update` 里乘 `delta`；做物理相关的请在
> `fixed_update` 里做。

---

## 三、两个必须知道的语法坑

### 1. 全局 `let` 在函数里看不见 —— 用 `const`

```rhai
let speed = 5.0;            // ✗ 函数里引用会报 "Variable not found"
const SPEED = 5.0;          // ✓
```

Rhai 的函数没有闭包，读不到顶层 `let`。引擎会把脚本**顶层语句先执行一遍**
（这样 `const` 才进得了作用域），但**顶层 `let` 依然不能被函数体写入** ——
这是 Rhai 的语义，不是本引擎的限制。

### 2. 想存"每实体的状态"用 `state_*`，别用全局变量

因为上面那条，计时器 / 冷却 / 血量这类可变状态没有别的落地方式：

```rhai
fn update(ctx) {
    // 冷却：每 0.5 秒才能开一枪
    let cd = state_get_or(ctx, "cooldown", 0.0) - delta(ctx);
    state_set(ctx, "cooldown", cd);

    if cd <= 0.0 && action_pressed(ctx, "Fire") {
        log(ctx, "bang!");
        state_set(ctx, "cooldown", 0.5);
    }
}
```

`state_*` 存的东西按 **`(实体, 脚本)` 分开** —— 同一个脚本挂在 10 个实体上，
每个实体有自己独立的一份状态。

---

## 四、API 一览

所有函数都把 `ctx` 作为**第一个参数**；Rhai 允许方法式调用，所以
`set_pos(ctx, x, y, z)` 和 `ctx.set_pos(x, y, z)` 都合法。

### 4.1 自身变换

| 函数 | 说明 |
|---|---|
| `pos(ctx)` | 自身**世界**坐标 → `Vec3` |
| `set_pos(ctx, x, y, z)` | 设置**世界**坐标（有父节点时自动换算回局部） |
| `local_pos(ctx)` / `set_local_pos(ctx, x, y, z)` | **局部**坐标 |
| `translate(ctx, dx, dy, dz)` | 世界空间平移增量 |
| `rot(ctx)` | 自身**局部**欧拉角（度），顺序 Y-X-Z → `Vec3` |
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

> 返回的 ID **立刻可用** —— `let id = spawn_entity(...)` 之后马上
> `set_entity_pos(ctx, id, ...)` 是生效的。

> 函数名不叫 `spawn` 是因为 Rhai 把 `spawn` 保留成了关键字。

### 4.4 物理

| 函数 | 说明 |
|---|---|
| `has_physics(ctx, id)` | 这个实体有没有刚体 |
| `raycast(ctx, ox, oy, oz, dx, dy, dz, max_dist)` | 射线检测，返回一个 Map |
| `apply_force(ctx, id, x, y, z)` | 施力（持续，下一帧清零） |
| `apply_impulse(ctx, id, x, y, z)` | 施冲量（瞬间改速度） |
| `add_torque(ctx, id, x, y, z)` | 施加力矩 |
| `velocity(ctx, id)` / `set_velocity(ctx, id, x, y, z)` | 线速度 |
| `gravity(ctx)` / `set_gravity(ctx, x, y, z)` | 全局重力 |

`raycast` 返回的 Map 字段：`hit`(bool)、`point`(Vec3)、`normal`(Vec3)、
`distance`(float)、`entity`(i64，0 表示没打中实体)、`is_sensor`(bool)。
**永远有全部字段**（没打中时 `hit: false`、其余为零值），所以脚本不必先判空。

```rhai
fn update(ctx) {
    // 脚下 0.6 米内有没有地？
    let p = pos(ctx);
    let r = raycast(ctx, p.x, p.y, p.z, 0.0, -1.0, 0.0, 0.6);
    if !r.hit {
        log(ctx, "踩空了");
    }
}
```

> `raycast` 会自动忽略**传感器**，并排除自身刚体（避免从自己内部出发打到自己）。

### 4.5 输入

| 函数 | 说明 |
|---|---|
| `action_down(ctx, "Jump")` | 动作是否按住 |
| `action_pressed(ctx, "Jump")` | 动作是否**本帧刚按下** |
| `action_released(ctx, "Jump")` | 动作是否**本帧刚松开** |
| `axis(ctx, "MoveX")` | 轴的值（-1 … 1） |
| `key_down(ctx, "W")` / `key_pressed(ctx, "W")` | 直接按物理键位查询 |
| `mouse_down(ctx, "Left")` / `mouse_pressed(ctx, "Left")` | 鼠标按键 |
| `mouse_pos(ctx)` / `mouse_delta(ctx)` / `scroll(ctx)` | 指针状态 → `Vec2` |

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
| `state_get(ctx, key)` | 取值；没设过返回 `()` |
| `state_get_or(ctx, key, default)` | 取值，缺省时返回 `default` |
| `state_set(ctx, key, value)` | 写入 |
| `state_has(ctx, key)` / `state_remove(ctx, key)` / `state_clear(ctx)` | 其它操作 |

### 4.8 数学

| 函数 | 说明 |
|---|---|
| `vec3(x, y, z)` / `vec2(x, y)` | 构造向量 |
| `a + b`、`a - b`、`a * 2.0`、`-a` | 向量运算 |
| `v.length()` / `v.length_squared()` / `v.normalized()` | 向量属性 |
| `a.dot(b)` / `a.cross(b)` / `a.distance_to(b)` / `a.lerp(b, t)` | 向量运算 |
| `v.x` / `v.y` / `v.z` | 分量（可读写） |
| `clamp(v, lo, hi)` / `lerpf(a, b, t)` | 数值工具 |
| `deg_to_rad(d)` / `rad_to_deg(r)` | 角度换算 |

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

```rhai
// 速度驱动 Idle/Walk/Run（阈值见场景 RON 里的 transitions），
// 叠加层权重按 sin 波起伏（走动时的"呼吸/弹跳幅度"看得见地变化）
fn update(ctx) {
    set_anim_param(ctx, "speed", velocity(ctx, self_id(ctx)).length());
    set_anim_additive_weight(ctx, 0.3 + 0.3 * sin(time(ctx) * 4.0));
}
```

示例见 `projects/SampleScene/Scripts/FoxLocomotion.rhai`。

Rhai 自带完整的 `if` / `while` / `for` / 数组 / Map / 字符串，
以及 `sin` / `cos` / `sqrt` / `abs` / `min` / `max` 等数学函数。

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
| 运行时错误（如除零、变量不存在） | **这一帧**这个钩子不生效，Console 报错，游戏继续 |
| 死循环 / 运算爆炸 | 超过 20 万次操作被强制中断（记一条错误），游戏继续 |
| 递归过深 | 超过 32 层调用被中断 |

**热重载**：Play 模式下点工具栏的 `⟳ Scripts`，全部脚本重新编译，不用重启。

---

## 七、和其它系统的分工

- **动态刚体由物理驱动**：`set_pos` 会被物理下一帧覆盖。
  想影响动态体用 `set_velocity` / `apply_impulse` / `apply_force`。
- **运动学刚体由脚本驱动**：`set_pos` / `set_rot` 每帧推给物理，它再去推动别人。
  想让脚本控制位置，就把实体的物理类型设成 `Kinematic`。
- **纯视觉实体**（`physics: None`）**不被物理碰**：脚本想怎么摆就怎么摆。
