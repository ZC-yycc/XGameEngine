//! 角色控制器参数（纯数据，可序列化）。
//!
//! # 为什么这一层在 core 而不是 physics
//!
//! 与 [`crate::physics::PhysicsBody`] 完全同款的分层理由：core 不认识 rapier，
//! 只描述「这个实体是一个什么样的角色」—— 多高、多粗、能上多高的台阶、
//! 能爬多陡的坡。`xgame-physics` 负责把这份声明翻译成 rapier 的
//! `KinematicCharacterController` 配置。
//!
//! # 与 PhysicsBody 的关系
//!
//! 两者**正交**，不是替代关系：
//!
//! - `physics: Some(PhysicsBody::kinematic_capsule(..))` 描述碰撞体本身
//!   （胶囊有多大、什么运动学类型）；
//! - `controller: Some(CharacterController { .. })` 描述**怎么移动**它
//!   （台阶、坡度、贴地吸附）。
//!
//! 运行时看到 `controller` 时，会**接管**这个实体的刚体类型（强制为
//! `KinematicPositionBased`，与 `PhysicsBody` 里声明的运动学类型解耦）——
//! 因为 rapier 的角色控制器要求位置驱动，而 `body_from_scene` 默认映射成
//! `KinematicVelocityBased`（那是为移动平台设计的）。
//!
//! # 数值约定
//!
//! 全部用**米**与**度**（`slope_limit` 是唯一的例外），与场景里其它长度字段
//! 一致。弧度换算只发生在 `xgame-physics` 适配 rapier API 的边界上。

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// 胶囊体默认半径（米）。略小于 0.5 的半格走廊，保证能穿过 1 米宽的门。
fn default_radius() -> f32 {
    0.3
}

/// 角色默认总高度（米，含上下两个半球）。1.8 米是常见第三人称/第一人称身高。
fn default_height() -> f32 {
    1.8
}

/// 自动台阶高度（米）。0.35 ≈ 一级常见楼梯踏步高。
fn default_step_offset() -> f32 {
    0.35
}

/// 可攀爬的最大坡度（度）。45° 是「能走上去」与「会滑下来」的直觉分界。
fn default_slope_limit() -> f32 {
    45.0
}

/// 皮肤宽度（米）：胶囊与障碍物之间保持的微小间隙。
///
/// 完全为 0 时，贴墙站立会因浮点抖动反复进入/离开接触状态（表现是角色
/// 高频颤动或卡进墙里）。0.02 足够小（2 厘米）不会被玩家看见。
fn default_skin_width() -> f32 {
    0.02
}

/// 贴地吸附距离（米）。走下小台阶或缓坡时，向下多探这个距离并吸附，
/// 避免角色「腾空—重力—落地」的弹跳感。
fn default_snap_to_ground() -> f32 {
    0.3
}

/// 一个实体的角色控制器参数。
///
/// 挂在 [`Entity::controller`](crate::Entity) 上；为 `None` 表示这个实体
/// 不做角色移动（普通动态/运动学物体，或纯视觉）。
///
/// # 参数合法性
///
/// 场景文件是**作者手写**的，任何字段都可能是 0、负数或天文数字。加载后应
/// 经过 [`sanitized`](Self::sanitized) 钳制：运行时直接信任原始值会让
/// `height < 2 * radius`（胶囊线段长度为负）之类的坏数据变成 NaN 变换，
/// 而 NaN 一旦写进物理世界，整个模拟就再也回不来了。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CharacterController {
    /// 胶囊体半径（米）。
    #[serde(default = "default_radius")]
    pub radius: f32,
    /// 角色总高度（米，含两端半球）。
    ///
    /// 胶囊体的**半高**（线段长度的一半）由 `(height - 2·radius)/2` 推导，
    /// 不在场景文件里单独存一份 —— 存两份就意味着它们可能对不上。
    #[serde(default = "default_height")]
    pub height: f32,
    /// 自动台阶高度（米）。0 = 不上任何台阶。
    #[serde(default = "default_step_offset")]
    pub step_offset: f32,
    /// 可攀爬的最大坡度（度）。0 = 任何斜面都上不去。
    #[serde(default = "default_slope_limit")]
    pub slope_limit: f32,
    /// 皮肤宽度（米），见 [`default_skin_width`] 的说明。
    #[serde(default = "default_skin_width")]
    pub skin_width: f32,
    /// 贴地吸附距离（米）。0 = 不吸附（走下台阶必然有一段腾空）。
    #[serde(default = "default_snap_to_ground")]
    pub snap_to_ground: f32,
}

impl Default for CharacterController {
    fn default() -> Self {
        Self {
            radius: default_radius(),
            height: default_height(),
            step_offset: default_step_offset(),
            slope_limit: default_slope_limit(),
            skin_width: default_skin_width(),
            snap_to_ground: default_snap_to_ground(),
        }
    }
}

impl CharacterController {
    /// 胶囊体的**半高**（米）—— rapier 胶囊需要的是 `half_height + radius`。
    ///
    /// `height <= 2·radius` 时（完全压扁的退化配置）返回 0，即退化成球体。
    /// 这也让 [`sanitized`](Self::sanitized) 的钳制只需保证
    /// `height >= 2·radius + 1e-3`，半高自然非负。
    pub fn half_height(&self) -> f32 {
        ((self.height - 2.0 * self.radius) * 0.5).max(0.0)
    }

    /// 胶囊中心相对**实体原点**的高度（米）。
    ///
    /// 坐标约定：`Entity::transform.position` 是角色的**脚底**（Unity 同款），
    /// 胶囊中心 = 原点 + `center_height()` · 世界上方向。这样在编辑器里把角色
    /// 拖到 y=0 就是"站在地平面上"，而不是半个身子埋进地里；场景文件里的
    /// 角色高度也直接就是玩家看到的脚底位置。
    ///
    /// 目前只支持直立的角色（偏移沿世界 Y）：角色控制器不做翻滚，
    /// 把偏移改成沿刚体上方向只会让"角色被撞歪"时的高度算错。
    pub fn center_height(&self) -> f32 {
        self.half_height() + self.radius
    }

    /// 把作者可能写坏的值钳制回合法区间（就地修改）。
    ///
    /// 钳制策略：**保方向、压极值**。负数一律归零（负半径/负台阶没有物理意义），
    /// 上界取"正常人不会超过、但物理上仍然稳定"的值。调用点在
    /// `xgame-runtime` 建立物理世界时，保证物理层永远收到合法参数。
    ///
    /// 之所以不做成加载期的全局清洗而是按需调用：`sanitized()` 是纯函数语义，
    /// 编辑器里也能拿它做输入框的钳制提示，两边共用同一份规则。
    pub fn sanitized(&self) -> Self {
        let mut c = *self;
        // NaN/inf 必须先兜底再 clamp —— `f32::clamp` 对 NaN 的 self 会原样
        // 传染（返回 NaN），下一行的 clamp 下限由上一个字段算出，NaN 一泄
        // 进去就是 panic。作者手滑写 nan、或上游算崩传染过来，都不能让
        // 装配期炸掉。
        c.radius = finite_or(c.radius, default_radius()).clamp(0.05, 2.0);
        c.height = finite_or(c.height, default_height()).clamp(2.0 * c.radius + 1e-3, 5.0);
        c.step_offset = finite_or(c.step_offset, default_step_offset()).clamp(0.0, 1.0);
        // 89° 而不是 90°：90° 的余弦在 f32 里是 ~-6e-8，rapier 的坡度比较
        // 用 cos(limit) 与法线点乘直接比，边界值会给「垂直墙也算可爬」。
        c.slope_limit = finite_or(c.slope_limit, default_slope_limit()).clamp(0.0, 89.0);
        c.skin_width = finite_or(c.skin_width, default_skin_width()).clamp(1e-3, 0.5);
        c.snap_to_ground = finite_or(c.snap_to_ground, default_snap_to_ground()).clamp(0.0, 2.0);
        c
    }

    /// 坡度限制的余弦值（rapier `max_slope_climb_angle` 的输入单位）。
    ///
    /// 独立出来是为了把「度 ↔ 弧度」的换算钉在一个地方，并让非有限输入
    /// （NaN）在这里兜底成默认坡度，而不是传染给 rapier。
    pub fn slope_limit_cos(&self) -> f32 {
        let deg = finite_or(self.slope_limit, default_slope_limit()).clamp(0.0, 89.0);
        deg.to_radians().cos().max(0.0)
    }
}

/// 非有限值（NaN / ±inf）兜底成 `fallback`。
///
/// 之所以需要它而不是裸 `clamp`：`f32::clamp` 的 self 为 NaN 时**原样返回
/// NaN**（比较全是 false），而 `sanitized` 里后一个字段的 clamp 下限是用前
/// 一个字段算的，NaN 会一路漏到底限参数上，触发 clamp 自己的 panic。
fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

/// 一次移动里最多记录几个接触刚体。
///
/// 8 个足够覆盖"站在地上 + 靠着墙 + 挤在两个箱子中间"这类常见情形；
/// 溢出时**丢弃多出来的**（而不是扩容）：查询结果只用于碰撞事件，
/// 漏报极端的第 9 个接触面，好过每次移动都分配一块堆内存。
pub const MAX_CHARACTER_CONTACTS: usize = 8;

/// 角色一次移动中碰到的刚体集合（固定容量、零分配、`Copy`）。
///
/// # 为什么角色需要自己记接触面
///
/// rapier 的碰撞事件来自**真实接触**。而角色控制器为了让扫掠稳定，永远
/// 停在离障碍物 `skin_width` 的地方 —— 它和墙 / 地板从来就没真正"接触"过，
/// 物理引擎自然一个事件都不会发（运动学 vs 静态本来也不生成事件）。所以
/// 角色的接触面只能由角色控制器自己上报，这也正是 Unity 的做法
/// （`CharacterController` 有一套独立于物理接触的 collision flags）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharacterContacts {
    ids: [u64; MAX_CHARACTER_CONTACTS],
    len: u8,
}

impl Default for CharacterContacts {
    fn default() -> Self {
        Self {
            ids: [0; MAX_CHARACTER_CONTACTS],
            len: 0,
        }
    }
}

impl CharacterContacts {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一个刚体 id（重复的忽略，超出容量的丢弃）。
    pub fn push(&mut self, body: u64) {
        if self.contains(body) {
            return;
        }
        let len = self.len as usize;
        if len >= MAX_CHARACTER_CONTACTS {
            return;
        }
        self.ids[len] = body;
        self.len += 1;
    }

    /// 遍历本次移动碰到的刚体 id。
    pub fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.ids[..self.len as usize].iter().copied()
    }

    pub fn contains(&self, body: u64) -> bool {
        self.iter().any(|b| b == body)
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// 角色控制器的**运行时状态**（每实体一份，不序列化）。
///
/// 与 [`CharacterController`] 的分工：那份是**怎么走**（作者在场景里配的
/// 参数），这份是**走到了哪**（引擎每帧算出来的结果）。挂在实体的
/// `ComponentMap` 上，由第一次 `character_simple_move` 惰性建立。
///
/// 为什么不放进场景文件：速度、着地状态是纯运行期量，存进场景只会在
/// 下次打开时把"上一次退出瞬间的速度"恢复成初始值 —— 那是 bug 而不是特性。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterState {
    /// 最近一次移动反推出来的速度（米/秒）。
    ///
    /// 不是输入速度：撞墙后水平分量会被投影掉、上坡会被压短 —— 这里记的
    /// 是**实际发生**的速度，脚本读它做动画混合才不会"对着墙还在跑步"。
    pub velocity: Vec3,
    /// 是否着地（最近一次移动结束后）。
    pub grounded: bool,
    /// 地面法线（世界坐标）。腾空时为 `Vec3::Y`。
    pub ground_normal: Vec3,
    /// 是否正在陡坡上自动下滑。
    pub sliding_down_slope: bool,
    /// 最近一次移动碰到的刚体（供运行时做 enter/exit 差分）。
    pub contacts: CharacterContacts,
}

impl Default for CharacterState {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            grounded: false,
            // 零向量归一化会出 NaN；脚本拿 `.y` 判坡度时默认值必须是"平地"。
            ground_normal: Vec3::Y,
            sliding_down_slope: false,
            contacts: CharacterContacts::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_documented_values() {
        let c = CharacterController::default();
        assert_eq!(c.radius, 0.3);
        assert_eq!(c.height, 1.8);
        assert_eq!(c.step_offset, 0.35);
        assert_eq!(c.slope_limit, 45.0);
        assert_eq!(c.skin_width, 0.02);
        assert_eq!(c.snap_to_ground, 0.3);
    }

    #[test]
    fn half_height_derives_from_height_and_radius() {
        let c = CharacterController {
            radius: 0.3,
            height: 1.8,
            ..Default::default()
        };
        // (1.8 - 0.6) / 2 = 0.6
        assert!((c.half_height() - 0.6).abs() < 1e-6);
    }

    /// 退化配置（height ≤ 2·radius，作者把高度写成了直径甚至更小）必须
    /// 退化成球体而不是负半高 —— 负半高会让 rapier 建出 NaN 胶囊。
    #[test]
    fn half_height_never_negative() {
        let c = CharacterController {
            radius: 0.3,
            height: 0.5, // 比 2*radius 还小
            ..Default::default()
        };
        assert_eq!(c.half_height(), 0.0);
    }

    /// RON 只写少数字段时其余落默认值（场景文件手写友好性）。
    #[test]
    fn serde_defaults_for_missing_fields() {
        let c: CharacterController = ron::from_str("(radius: 0.4)").unwrap();
        assert_eq!(c.radius, 0.4);
        assert_eq!(c.height, 1.8, "没写 height 应落默认");
        assert_eq!(c.step_offset, 0.35, "没写 step_offset 应落默认");
        assert_eq!(c.slope_limit, 45.0);
        assert_eq!(c.skin_width, 0.02);
        assert_eq!(c.snap_to_ground, 0.3);
    }

    #[test]
    fn serde_roundtrip() {
        let c = CharacterController {
            radius: 0.25,
            height: 2.0,
            step_offset: 0.5,
            slope_limit: 60.0,
            skin_width: 0.01,
            snap_to_ground: 0.4,
        };
        let text = ron::to_string(&c).unwrap();
        let back: CharacterController = ron::from_str(&text).unwrap();
        assert_eq!(c, back);
    }

    /// 坏数据钳制：每一项都写崩，钳回来后必须全部合法，且
    /// half_height 非负（NaN 传染的第一道关口）。
    #[test]
    fn sanitized_clamps_garbage() {
        let garbage = CharacterController {
            radius: -1.0,
            height: 0.0,
            step_offset: -3.0,
            slope_limit: 180.0,
            skin_width: 0.0,
            snap_to_ground: -0.5,
        };
        let c = garbage.sanitized();
        assert_eq!(c.radius, 0.05);
        assert!(c.height >= 2.0 * c.radius, "height 必须容得下胶囊");
        assert_eq!(c.step_offset, 0.0, "负台阶归零（不上台阶）");
        assert_eq!(c.slope_limit, 89.0, "坡度钳到 89 而非 90");
        assert_eq!(c.skin_width, 1e-3);
        assert_eq!(c.snap_to_ground, 0.0);
        assert!(c.half_height() >= 0.0);
        assert!(c.slope_limit_cos().is_finite());
    }

    /// NaN 输入（作者手滑写了 nan，或上游算崩了传染过来）必须被兜底掉。
    /// 注意 `f32::clamp` 对 NaN 的 self 会**原样传染 NaN**，所以 sanitized
    /// 必须先过 `finite_or` 再 clamp —— 否则 height 的 clamp 下限（由 radius
    /// 算出）会变成 NaN，直接踩 clamp 自己的 panic。这条用例就是那个 bug
    /// 的回归网。
    #[test]
    fn sanitized_clamps_nan() {
        let c = CharacterController {
            radius: f32::NAN,
            height: f32::NAN,
            step_offset: f32::NAN,
            slope_limit: f32::NAN,
            skin_width: f32::NAN,
            snap_to_ground: f32::NAN,
        }
        .sanitized();
        assert!(c.radius.is_finite());
        assert!(c.height.is_finite());
        assert!(c.half_height() >= 0.0);
        assert!(c.slope_limit_cos().is_finite());
    }

    /// ±inf 同样必须被兜底（clamp 本身能压 inf，但压完前要先过 finite_or，
    /// 这里钉住 end-to-end 行为）。
    #[test]
    fn sanitized_clamps_infinities() {
        let c = CharacterController {
            radius: f32::INFINITY,
            height: f32::INFINITY,
            step_offset: f32::INFINITY,
            slope_limit: f32::INFINITY,
            skin_width: f32::INFINITY,
            snap_to_ground: f32::INFINITY,
        }
        .sanitized();
        assert!(c.radius.is_finite());
        assert!(c.height.is_finite());
        assert!(c.half_height() >= 0.0);
        assert!(c.slope_limit_cos().is_finite());
    }

    #[test]
    fn slope_limit_cos_converts_degrees() {
        let c = CharacterController {
            slope_limit: 60.0,
            ..Default::default()
        };
        assert!((c.slope_limit_cos() - 0.5).abs() < 1e-6, "cos60° = 0.5");
    }
}
