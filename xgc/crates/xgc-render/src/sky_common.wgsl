// 解析天空的 WGSL 公共片段：`SkyUniform` / `SkySH` 两个结构 + `sky_radiance` /
// `sky_sh` 两个纯函数。
//
// **刻意不声明任何绑定**：天空盒、场景片元、环境预过滤三处的布局各不相同，公共
// 片段只带类型与函数，uniform 由调用方以参数传入。这样"天空辐射公式只有一份"
// 就不靠人肉同步 —— 旧端 `sky.rs` 的 `SKY_COMMON_WGSL` 同一条纪律。
//
// 与 `sky.rs` 的 CPU `sky_radiance` / `sh_project` 必须逐式对应：天空盒背景、
// 物体环境照明、预过滤环境立方体三处读的必须是同一份辐射。

struct SkyUniform {
    // xyz = 指向太阳的单位向量，w = 天空是否开启（1 / 0）
    sun_dir: vec4<f32>,
    // rgb = 太阳颜色，a = 太阳强度乘子
    sun_color: vec4<f32>,
    // rgb = 天顶色，a = 整体强度乘子
    zenith: vec4<f32>,
    // rgb = 地平线色，a = 太阳盘半径（度）
    horizon: vec4<f32>,
    // rgb = 地面色，a = 预留（写 0）
    ground: vec4<f32>,
};

struct SkySH { sh: array<vec4<f32>, 9> }

const SKY_PI: f32 = 3.141592653589793;
// 太阳盘的面辐射亮度系数：真实太阳盘远超"白面 1.0"，40 让它色调映射后是一小块
// 近白、并让光晕有东西可抓。
const SKY_SUN_DISK_RADIANCE: f32 = 40.0;
// 太阳周围的光晕（宽而软的近似二次散射）。
const SKY_SUN_GLOW: f32 = 0.35;
// 预过滤环境立方体的最大 mip（= 级数 - 1，与 `env_map.rs` 的 ENV_MIPS 一致）。
const ENV_MAX_LOD: f32 = 4.0;

// 天空辐射亮度（线性 HDR）。
//
// 渐变在 dir.y == 0 处连续：天空段与地面段都收敛到地平线色，没有缝合线。
fn sky_radiance(u: SkyUniform, dir: vec3<f32>) -> vec3<f32> {
    let t = clamp(dir.y, -1.0, 1.0);
    let up_t = pow(max(t, 0.0), 0.5);
    let down_t = pow(max(-t, 0.0), 0.5);
    let sky_c = mix(u.horizon.rgb, u.zenith.rgb, up_t);
    let gnd_c = mix(u.horizon.rgb, u.ground.rgb, down_t);
    var c = select(gnd_c, sky_c, t > 0.0);

    let cos_ang = clamp(dot(dir, u.sun_dir.xyz), -1.0, 1.0);
    let ang = acos(cos_ang);
    let sun_rad = max(SKY_PI / 180.0 * u.horizon.w, 1e-4);
    let disk = 1.0 - smoothstep(sun_rad * 0.6, sun_rad, ang);
    let glow = SKY_SUN_GLOW * pow(max(cos_ang, 0.0), 16.0);
    c = c + u.sun_color.rgb * u.sun_color.a * (SKY_SUN_DISK_RADIANCE * disk + glow);
    return c * u.zenith.a;
}

// E/π 的球谐求值（系数里已折好余弦卷积）；基函数与 `sky.rs` 的 `sh_basis` 逐项
// 一致（[Y00, Y1-1(y), Y10(z), Y11(x), Y2-2, Y2-1, Y20, Y21, Y22]）。
fn sky_sh(sh: SkySH, n: vec3<f32>) -> vec3<f32> {
    return sh.sh[0].rgb * 0.282095
         + sh.sh[1].rgb * (0.488603 * n.y)
         + sh.sh[2].rgb * (0.488603 * n.z)
         + sh.sh[3].rgb * (0.488603 * n.x)
         + sh.sh[4].rgb * (1.092548 * n.x * n.y)
         + sh.sh[5].rgb * (1.092548 * n.y * n.z)
         + sh.sh[6].rgb * (0.315392 * (3.0 * n.z * n.z - 1.0))
         + sh.sh[7].rgb * (1.092548 * n.x * n.z)
         + sh.sh[8].rgb * (0.546274 * (n.x * n.x - n.y * n.y));
}
