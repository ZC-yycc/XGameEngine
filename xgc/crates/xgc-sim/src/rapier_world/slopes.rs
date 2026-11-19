//! 坡面（`physics.slope`）在 rapier 里的几何：静态楔形碰撞体。
//!
//! `Slope` 本身是**纯解析高度函数**（`Slope::height_at`），角色贴地与陡坡判定都读它，
//! 那也是 ECS 侧唯一的口径。但 rapier 侧（KCC 的水平扫掠、动态刚体、以及下一片要把
//! 角色竖直搬进 KCC 之后的自实现探地）看不见解析函数，只会看到碰撞体——所以这里按同
//! 一个坡面参数补一份薄壳楔形碰撞体，让两套口径落在同一个形状上。
//!
//! 形状：坡面沿 +X 抬升 `rise`，于是取一个**绕 Z 轴旋转**的长方体——局部 +X 指向坡面
//! 上升方向（长度 = 斜长 `hypot(size[0], rise)`），局部 +Y 是坡面法线（厚度
//! `SLOPE_THICKNESS`，碰撞体必须有体积），中心放在坡面中点的法线负方向 `thickness/2`
//! 处，让**上表面正好贴在解析坡面上**。
//!
//! 与刚体表的关系：实体上若另有 `physics.body`（显式动力学声明），坡面碰撞体让位——
//! 否则两份几何叠在一起，扫掠会有双重接触点。

use rapier3d::na::{Isometry3, Point3, Translation3, UnitQuaternion, Vector3};
use rapier3d::prelude::{ColliderBuilder, QueryFilter, Ray, RigidBodyBuilder, RigidBodyHandle};

use xgc_base::EntityKey;
use xgc_world::{PhysicsBody, Slope, World};

use super::RapierWorld;

/// 楔形碰撞体的厚度（米）。
///
/// 解析坡面没有厚度，碰撞体必须有：太厚会把坡底的立面整体往前推（角色离坡面还有一段
/// 就被挡住），太薄则在高速扫掠下容易一步跨过去。0.1 m 对 1.8 m 高的角色相当于脚掌厚度。
pub const SLOPE_THICKNESS: f32 = 0.1;

/// 向下探地射线的最大长度：坡面最多只有几百米，这个量级足够且不会跟远景几何打架。
const GROUND_RAY_MAX_TOI: f32 = 1_000.0;

/// 一个坡面实体的运行时托管：静态刚体（碰撞体挂在它上面，删刚体时连带删掉）。
pub(super) struct SlopeLink {
    pub(super) body: RigidBodyHandle,
}

/// 坡面签名的一项：实体 ID + 句柄（含代际）+ 建几何用到的全部浮点位模式。
pub(super) type SlopeSignature = Vec<(u64, EntityKey, [u32; 6])>;
/// 坡面收集阶段的临时表（含组件值，重建时直接用）。
pub(super) type SlopeEntry = (u64, EntityKey, Slope);

impl RapierWorld {
    /// 按当前世界建 / 重建坡面碰撞体（`physics.slope` 且没有 `physics.body` 的实体）。
    pub(super) fn ensure_slopes(&mut self, world: &World) {
        let mut entries = std::mem::take(&mut self.slope_entries);
        entries.clear();
        world.for_each::<Slope>(|key, def| {
            // 显式刚体优先：同一个实体不该有两套几何（与角色托管同一条规则）。
            if world.get::<PhysicsBody>(key).is_some() {
                return;
            }
            // 退化 / 非有限的坡面解析上也探不到地面（`height_at` 返回 None），不建碰撞体。
            if !valid(def) {
                return;
            }
            if let Some(id) = world.entity(key) {
                entries.push((id.raw(), key, *def));
            }
        });
        // 与刚体表同一条确定性理由：按实体 ID 排序建体，句柄顺序不随 archetype 变。
        entries.sort_unstable_by_key(|(id, _, _)| *id);
        // 签名必须覆盖建几何用到的每一个输入（含句柄代际）：漏一个，运行期改坡面就是静默 no-op。
        let mut signature = std::mem::take(&mut self.slope_probe);
        signature.clear();
        signature.extend(entries.iter().map(|(id, key, def)| {
            (
                *id,
                *key,
                [
                    def.origin[0].to_bits(),
                    def.origin[1].to_bits(),
                    def.origin[2].to_bits(),
                    def.size[0].to_bits(),
                    def.size[1].to_bits(),
                    def.rise.to_bits(),
                ],
            )
        }));
        if self.slope_signature.as_ref() == Some(&signature) {
            self.slope_probe = signature;
            self.slope_entries = entries;
            return;
        }
        // 只重排坡面这一片：`physics.body` 与角色的刚体都不动。
        for link in std::mem::take(&mut self.slopes) {
            self.bodies.remove(
                link.body,
                &mut self.islands,
                &mut self.colliders,
                &mut self.impulse_joints,
                &mut self.multibody_joints,
                true,
            );
        }
        for (_, _, def) in &entries {
            let body = self.bodies.insert(RigidBodyBuilder::fixed().build());
            self.colliders.insert_with_parent(
                // 摩擦取大值：坡面是地面，站在上面不该被微小水平力推走；
                // 反弹为 0，与旧端地形碰撞体的默认口径一致。
                ColliderBuilder::cuboid(half_length(def), SLOPE_THICKNESS * 0.5, def.size[1] * 0.5)
                    .friction(1.0)
                    .restitution(0.0)
                    .build(),
                body,
                &mut self.bodies,
            );
            // 刚体位姿带旋转：碰撞体的局部轴就按坡面摆好。
            if let Some(rigid_body) = self.bodies.get_mut(body) {
                rigid_body.set_position(slope_isometry(def), false);
            }
            self.slopes.push(SlopeLink { body });
        }
        // 新签名接管旧的探针缓冲（重建这条路也不另外分配）。
        self.slope_probe = self.slope_signature.replace(signature).unwrap_or_default();
        self.slope_entries = entries;
        // 新加 / 删过碰撞体，查询加速结构要显式重建，否则射线与扫掠看不到它们。
        self.query.update(&self.colliders);
    }

    /// 从 `(x, from_y, z)` 竖直向下打一条射线，返回命中点的 `y`。
    ///
    /// 给测试（把碰撞体高度与 `Slope::height_at` 对齐）与下一片自实现的探地
    /// （`probe_ground`）共用：旧端也是"从胶囊中心竖直向下打射线取法向间隙"。
    ///
    /// 注意这里用的是 `QueryFilter::default()`：**任何**碰撞体都会命中，包括传感器与
    /// 调用者自己的胶囊（射线从体内出发时 `solid = true` 会直接返回 `toi = 0`）。
    /// 变成真正的 `probe_ground` 时必须换成排除自身 / 传感器的过滤器，否则会探到
    /// 自己身上。
    pub fn ground_under(&self, x: f32, z: f32, from_y: f32) -> Option<f32> {
        let ray = Ray::new(Point3::new(x, from_y, z), Vector3::new(0.0, -1.0, 0.0));
        self.query
            .cast_ray(
                &self.bodies,
                &self.colliders,
                &ray,
                GROUND_RAY_MAX_TOI,
                true,
                QueryFilter::default(),
            )
            .map(|(_, toi)| from_y - toi)
    }
}

/// 坡面是否可建几何：尺寸为正、所有输入有限，且斜长本身不会溢出成 `inf`。
///
/// 最后一条是防御：`hypot(size[0], rise)` 对巨大但有限的输入会溢出，把无限大的
/// 长方体交给 rapier 没有意义（解析坡面在那个尺度上也不可用了）。
fn valid(slope: &Slope) -> bool {
    slope.size[0].is_finite()
        && slope.size[1].is_finite()
        && slope.rise.is_finite()
        && slope.origin.iter().all(|value| value.is_finite())
        && slope.size[0] > 0.0
        && slope.size[1] > 0.0
        && half_length(slope).is_finite()
}

/// 楔形长方体的半长：坡面斜长的一半。
fn half_length(slope: &Slope) -> f32 {
    (slope.size[0] * slope.size[0] + slope.rise * slope.rise).sqrt() * 0.5
}

/// 坡面中点的位姿：绕 Z 轴转 `atan2(rise, size[0])`，中心退回法线方向半个厚度。
fn slope_isometry(slope: &Slope) -> Isometry3<f32> {
    let angle = slope.rise.atan2(slope.size[0]);
    let (sin, cos) = angle.sin_cos();
    // 坡面上表面中点。
    let surface = Vector3::new(
        slope.origin[0] + slope.size[0] * 0.5,
        slope.origin[1] + slope.rise * 0.5,
        slope.origin[2] + slope.size[1] * 0.5,
    );
    // 局部 +Y 旋转后就是坡面法线 `(-sin, cos, 0)`；中心放在法线负方向，上表面才贴合坡面。
    let center = surface - Vector3::new(-sin, cos, 0.0) * (SLOPE_THICKNESS * 0.5);
    Isometry3::from_parts(
        Translation3::from(center),
        UnitQuaternion::from_axis_angle(&Vector3::z_axis(), angle),
    )
}
