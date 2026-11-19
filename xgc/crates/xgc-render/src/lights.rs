//! 多光源打包：单帧最多 8 盏方向光 / 点光，与旧引擎 `GpuLight` / `build_lights` 同口径。
//!
//! 旧端把光源塞进一个 `LightsUniform`（rgb 环境光 + 有效盏数、8 组 `GpuLight`），
//! 片元按盏循环累加；XGC 沿用同一布局，让"多盏叠加 / 点光衰减 / 哪盏灯用阴影图"
//! 这三件事都只有一个真相源：这张表。
//!
//! 打包是纯 CPU 计算，因此可以脱离 GPU 单测——着色器里那套循环不好搭像素级期望值，
//! 而"顺序、盏数、标记位"恰恰是最容易写错又最该钉住的部分。

use xgc_world::{LightKind, SceneEnvironment};

/// 单帧最多参与着色的光源数；与旧端 `MAX_LIGHTS` 一致。
pub(crate) const MAX_LIGHTS: usize = 8;

/// 一盏灯在 uniform 里占 48B（3 个 vec4）；与 WGSL `GpuLight` 逐字段对应。
pub(crate) const LIGHT_UNIFORM_BYTES: u64 = 48;

/// GPU 侧的一盏灯。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct GpuLight {
    /// 方向光：xyz = 光线传播方向（归一化）、w = 0；点光：xyz = 世界坐标、w = 1。
    pub(crate) pos_type: [f32; 4],
    /// rgb = 光色，a = 强度。
    pub(crate) color_intensity: [f32; 4],
    /// x = 这盏灯是否用方向光阴影图（1 / 0），y = 点光影响半径；z / w 预留。
    pub(crate) shadow_params: [f32; 4],
}

impl GpuLight {
    /// 按 WGSL 字段顺序写 12 个 f32。
    pub(crate) fn write(&self, bytes: &mut Vec<u8>) {
        for value in self
            .pos_type
            .iter()
            .chain(self.color_intensity.iter())
            .chain(self.shadow_params.iter())
        {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
}

/// 打包结果：定长表（空位留 0）+ 有效盏数。
pub(crate) struct PackedLights {
    pub(crate) lights: [GpuLight; MAX_LIGHTS],
    pub(crate) count: u32,
}

/// 把环境里的**可用**光源按原顺序装进定长表。
///
/// 三条与旧端一致的规则：
/// - `intensity <= 0` 视为关灯，直接跳过（它不占槽位）；
/// - 超过 8 盏的部分丢掉——9 盏不可能都塞进 uniform，旧端同样 `take(MAX_LIGHTS)`；
/// - 只有**第一盏投射阴影的方向光**打上阴影标记（XGC 的 CSM 只为它拟合，
///   标记打错会让别的方向光去采样一张不属于自己的阴影图）。
///
/// 点光的 `range` 非有限时打包成 0：着色器见到 0 就把这盏灯整个跳过（旧端同口径）。
pub(crate) fn pack(environment: &SceneEnvironment) -> PackedLights {
    let mut packed = PackedLights {
        lights: [GpuLight::default(); MAX_LIGHTS],
        count: 0,
    };
    let mut shadow_marked = false;
    for light in environment.lights.iter().filter(|light| light.is_active()) {
        if packed.count as usize >= MAX_LIGHTS {
            break;
        }
        let mut gpu = GpuLight {
            color_intensity: [
                light.color[0],
                light.color[1],
                light.color[2],
                light.intensity,
            ],
            ..GpuLight::default()
        };
        match light.kind {
            LightKind::Directional => {
                let [x, y, z] = light.normalized_direction();
                gpu.pos_type = [x, y, z, 0.0];
                if light.casts_shadow && !shadow_marked {
                    gpu.shadow_params[0] = 1.0;
                    shadow_marked = true;
                }
            }
            LightKind::Point => {
                gpu.pos_type = [light.position[0], light.position[1], light.position[2], 1.0];
                gpu.shadow_params[1] = if light.range.is_finite() {
                    light.range.max(0.0)
                } else {
                    0.0
                };
            }
        }
        packed.lights[packed.count as usize] = gpu;
        packed.count += 1;
    }
    packed
}

/// 本帧主阴影光：唯一会在 [`pack`] 里拿到 `shadow_params.x = 1` 的那盏方向光。
///
/// 与 `pack` 走同一组判据（可用光源、最多 8 盏、方向光、`casts_shadow`），
/// 所以"CSM 为谁拟合"与"着色器里谁带阴影标记"永远指同一盏灯。分开各扫一遍
/// 环境表会漏掉"主阴影光排在第 9 盏之后"的情形：那时着色器里没有任何光源带标记，
/// 却仍然白跑一次 2048² 的级联深度 pass。
pub(crate) fn primary_shadow_light(
    environment: &SceneEnvironment,
) -> Option<&xgc_world::SceneLight> {
    environment
        .lights
        .iter()
        .filter(|light| light.is_active())
        .take(MAX_LIGHTS)
        .find(|light| light.kind == LightKind::Directional && light.casts_shadow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgc_world::SceneLight;

    fn point(position: [f32; 3], intensity: f32, range: f32) -> SceneLight {
        SceneLight {
            name: "lamp".to_string(),
            kind: LightKind::Point,
            position,
            intensity,
            range,
            ..SceneLight::default()
        }
    }

    #[test]
    fn packs_directional_and_point_lights_in_order() {
        let mut environment = SceneEnvironment::default();
        environment.lights.push(point([1.0, 2.0, 3.0], 2.5, 7.0));
        let packed = pack(&environment);

        assert_eq!(packed.count, 2);
        // 方向光：传播方向进 xyz、w = 0；只有它打阴影标记。
        let sun = packed.lights[0];
        let [x, y, z] = environment.lights[0].normalized_direction();
        assert_eq!(sun.pos_type, [x, y, z, 0.0]);
        assert_eq!(sun.shadow_params[0], 1.0);
        assert_eq!(sun.shadow_params[1], 0.0);
        assert_eq!(sun.color_intensity[3], environment.lights[0].intensity);
        // 点光：位置进 xyz、w = 1；半径进 shadow_params.y。
        let lamp = packed.lights[1];
        assert_eq!(lamp.pos_type, [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(lamp.color_intensity, [1.0, 0.98, 0.95, 2.5]);
        assert_eq!(lamp.shadow_params, [0.0, 7.0, 0.0, 0.0]);
    }

    #[test]
    fn inactive_lights_do_not_take_a_slot() {
        let mut environment = SceneEnvironment::default();
        for _ in 0..9 {
            environment.lights.push(point([0.0, 2.0, 0.0], 0.0, 10.0));
        }
        environment.lights.push(point([0.0, 3.0, 0.0], 1.0, 10.0));
        let packed = pack(&environment);

        assert_eq!(packed.count, 2);
        assert_eq!(packed.lights[1].pos_type, [0.0, 3.0, 0.0, 1.0]);
    }

    #[test]
    fn the_eighth_light_is_the_last_one_kept() {
        let mut environment = SceneEnvironment::default();
        for index in 0..8 {
            environment
                .lights
                .push(point([index as f32, 1.0, 0.0], 1.0, 10.0));
        }
        let packed = pack(&environment);
        // 缺省方向光 + 8 盏点光 = 9 盏，第 9 盏（index 8）被丢掉。
        assert_eq!(packed.count, 8);
        assert_eq!(packed.lights[7].pos_type, [6.0, 1.0, 0.0, 1.0]);
        assert_eq!(packed.lights[0].pos_type[3], 0.0);
    }

    #[test]
    fn only_the_first_shadow_caster_is_marked() {
        let mut environment = SceneEnvironment {
            lights: vec![
                SceneLight {
                    casts_shadow: false,
                    ..SceneLight::default()
                },
                SceneLight::directional("second", [0.0, -1.0, 0.0], [1.0; 3], 2.0),
                SceneLight::directional("third", [0.0, -1.0, 1.0], [1.0; 3], 2.0),
            ],
            ..SceneEnvironment::default()
        };
        environment.lights[1].casts_shadow = true;
        environment.lights[2].casts_shadow = true;
        let packed = pack(&environment);

        assert_eq!(packed.count, 3);
        // 第一盏不投影、第二盏投影：标记必须在第二盏上，且只有一处。
        assert_eq!(packed.lights[0].shadow_params[0], 0.0);
        assert_eq!(packed.lights[1].shadow_params[0], 1.0);
        assert_eq!(packed.lights[2].shadow_params[0], 0.0);
    }

    #[test]
    fn a_non_finite_point_range_is_packed_as_zero() {
        let mut environment = SceneEnvironment {
            lights: vec![point([0.0, 1.0, 0.0], 1.0, f32::INFINITY)],
            ..SceneEnvironment::default()
        };
        assert_eq!(pack(&environment).lights[0].shadow_params[1], 0.0);
        environment.lights[0].range = f32::NAN;
        assert_eq!(pack(&environment).lights[0].shadow_params[1], 0.0);
    }

    #[test]
    fn the_default_environment_packs_one_sun() {
        let packed = pack(&SceneEnvironment::default());
        assert_eq!(packed.count, 1);
        assert_eq!(packed.lights[0].pos_type[3], 0.0);
        assert_eq!(packed.lights[0].shadow_params[0], 1.0);
    }

    #[test]
    fn a_shadow_caster_past_the_eighth_slot_is_not_a_primary_shadow_light() {
        // 8 盏点光排在前面，投阴影的方向光只能排第 9：它既进不了 uniform，
        // 也就不该有谁为它拟合 CSM（否则每帧白跑一次级联深度 pass）。
        let mut environment = SceneEnvironment::default();
        environment.lights.clear();
        for index in 0..MAX_LIGHTS {
            environment
                .lights
                .push(point([index as f32, 1.0, 0.0], 1.0, 10.0));
        }
        environment.lights.push(SceneLight::directional(
            "late sun",
            [0.0, -1.0, 0.0],
            [1.0; 3],
            1.0,
        ));

        let packed = pack(&environment);
        assert_eq!(packed.count as usize, MAX_LIGHTS);
        assert!(packed
            .lights
            .iter()
            .all(|light| light.shadow_params[0] == 0.0));
        assert!(primary_shadow_light(&environment).is_none());

        // 把方向光挪进前 8 盏，同一个判据立刻认出它。
        environment.lights.swap(0, MAX_LIGHTS);
        assert!(primary_shadow_light(&environment).is_some());
        assert_eq!(pack(&environment).lights[0].shadow_params[0], 1.0);
    }
}
