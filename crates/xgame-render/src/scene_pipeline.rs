//! 共享 3D 场景渲染管线。
//!
//! 封装 wgpu 渲染管线、着色器、uniform 缓冲区和逐实体绘制逻辑。
//! 可在编辑器离屏渲染器和独立游戏二进制中复用。
//!
//! # 着色模型
//!
//! 采用 **PBR（metallic-roughness 工作流）**：Cook-Torrance 的 GGX 分布 +
//! Smith 几何 + Schlick 菲涅尔。glTF 的 `metallicFactor` / `roughnessFactor`
//! 一路原样送进着色器，金属是有色反射、绝缘体有 4% 基础反射，粗糙度连续
//! 控制高光形状 —— 这是"材质身份"的语义层，不再有"高光强度 / 光泽度"
//! 这类翻译损耗。
//!
//! 两个**刻意的取舍**（防好心人翻案）：
//!
//! - **环境光是"均匀环境辐射"**：全局 `ambient` 直接当作 IBL 的最低阶近似
//!   （漫反射按金属度分流、镜面按菲涅尔加权）。金属因此能反射环境色，但
//!   看不出形状 —— 真环境反射（预过滤贴图 / 天空盒）留给后续轮次，出口
//!   已经收在 `fs_main` 的环境项里。
//! - **π 并进 radiance**：物理正确的漫反射带 `1/π`，但历史场景的光强是按
//!   "Lambert 直乘"手调的（SampleScene 太阳 = 3.2）。照搬会把全场砍掉
//!   3.14 倍、老场景集体欠曝，所以在光源侧乘 π 把观感还回来（详见
//!   `fs_main` 里的"π 校准"注释）。
//!
//! 单帧最多 [`MAX_LIGHTS`] 盏方向光 / 点光叠加。
//!
//! # 绑定组布局
//!
//! ```text
//! group(0)  全局，整帧一份
//!   binding 0  camera    uniform（view_proj + inv_view_proj + 相机位置）
//!   binding 1  lights    uniform（环境光 + 光源数组）
//!   binding 2  draws     storage（逐实例：模型矩阵 + 蒙皮槽位 + 材质；
//!                        一个实例一段、紧密排布，实例下标 =
//!                        `@builtin(instance_index)`，见"实例化"一节）
//!   binding 3  shadow    texture_depth_2d_array（方向光级联阴影，一层一个级联）
//!   binding 4  shadow_smp    comparison sampler
//!   binding 5  shadow_ub     uniform（级联矩阵数组 + 切分表 + 偏置/开关）
//!   binding 6  bones     storage（蒙皮骨骼矩阵，见下）
//!   binding 7  sky       uniform（天空参数；太阳转向/强度都在这）
//!   binding 8  sky_sh    uniform（漫反射环境的 L2 球谐，E/π 口径）
//!   binding 9  env_cube  texture_cube（预过滤镜面环境，5 级 mip）
//!   binding 10 env_smp   sampler（立方体与 BRDF LUT 共用）
//!   binding 11 brdf_lut  texture_2d（split-sum 的 BRDF 积分表）
//!
//! group(1)  材质，每个材质槽一份
//!   binding 0  base color texture
//!   binding 1  sampler
//! ```
//!
//! # 实例化（draw batching）
//!
//! 成百上千个实体共用同一张网格时，"一个实体一次 draw call"的固定成本
//! （命令校验、绑定组、顶点/索引缓冲切换）会直接压垮 CPU 侧 —— 实测
//! 2000 实体 ≈ 2.2 ms/帧，而 GPU 只花 0.3 ms。本管线把**同图元**的绘制项
//! 合成一批：
//!
//! - 逐实例数据（模型矩阵 / 蒙皮槽位 / 材质）走 `draw_buffer` 这块 **storage
//!   缓冲**，一个实例一段、按 112 B 紧密排布（不需要 uniform 动态偏移的
//!   256 B 对齐填充）；顶点着色器用 `@builtin(instance_index)` 直接索引
//!   `draws[...]`；
//! - 一批 = 一次 `draw_indexed(indices, 0, first..first+count)`：`first` 是
//!   这一批在缓冲里的**起始槽位**。实例下标从 `first` 起单调递增，与实例
//!   数据在缓冲里的紧密排布正好对上（这是"批次占连续槽位"的落脚点）；
//! - 材质参数（base_color / metallic / roughness）是**逐实例**的（实体级
//!   覆盖），随实例数据走 —— 同一网格的两个实体仍然各调各的颜色；只有
//!   材质绑定组（贴图 + 采样器）按批切换，一批一次 `set_bind_group`；
//! - 半透明**不参与合批**：混合结果与绘制顺序相关（远→近），每项独立一次
//!   draw call。阴影 pass 复用同一批批次（同一份实例数据），无需另行准备。
//!
//! # 视锥剔除
//!
//! 每帧收集绘制项之前，先按实体**网格级世界包围盒**与相机视锥求交；
//! 完全在视锥外的实体整段跳过（不算蒙皮骨骼、不合并材质、不写实例数据）。
//! 判据是 [`crate::frustum::Frustum`] 的六个归一化平面 —— 纯几何、可单测。
//!
//! 两个必须守住的正确性细节（否则"优化"会变成画面 bug）：
//!
//! - **阴影补投**：相机看不见、但仍会把影子投进画面的物体不能一起剔掉。
//!   这类实体进 `shadow_batches`，只参与阴影 pass（不占主 pass 的实例槽位，
//!   也不需要材质绑定组）。相机视锥 / 阴影视锥都命不中的才真的跳过。
//! - **蒙皮余量**：蒙皮网格的包围盒是**绑定姿态**的（顶点在 VS 里才被骨骼
//!   拉走），动画能把肢体甩出盒外。给这类实体留 50% 半径余量，宁可少剔
//!   几个角色，也不能让"走路时手臂在屏幕边缘忽然消失"。
//!
//! 剔除是**保守**的（包围盒相交即保留）：画面必须与不剔除逐位一致，这点由
//! `screenshot.rs` 的 GPU 等价测试钉死。相机矩阵退化时整段不剔除（宁可白画，
//! 不能黑屏）。合批的分组键 / 批次顺序不受影响 —— 剔除只让某些批次少几个
//! 实例，或整批不出现，不重排剩余项。
//!
//! **分组键是图元地址、批次顺序是"首次遇到"**（刻意的）：图元地址只用于
//! 相等判定（同一网格在不同实体间共享同一份 `GltfPrimitive`，地址相等 ⇔
//! 顶点/索引缓冲与材质槽全同），排序一律按实体遍历顺序 —— 指针值每次运行
//! 都不同，拿它排序会让槽位布局随机漂移，"同机逐字节确定"的截图契约
//! （[`crate::screenshot`]）就没了。
//!
//! # 天空盒与环境光照（IBL）
//!
//! 天空（[`crate::sky`]）一开，环境照明从"均匀 ambient"升级为"解析天空的
//! 积分"：漫反射走 CPU 投影的 L2 球谐，镜面走 GPU 预过滤立方体 + BRDF LUT。
//! 天空盒本身是场景 pass 里的一个全屏三角形，画在**不透明之后、半透明之前**
//! （z = 1.0 + `LessEqual` + 不写深度）：被几何盖住的像素失败、空像素作画，
//! 半透明仍混在天空之上。它画进 MSAA 附件 ⇒ 物体轮廓与天空的交界有正确
//! 的多采样覆盖，而不是"先铺一张背景图再让 MSAA 只作用于物体"。
//!
//! 关着时整段跳过（无 pass、无采样），与天空上线前的画面逐位一致 —— 亮度
//! 基线纪律与 π 标定同款（见 `fs_main` 的"π 校准"一节）。
//!
//! 材质绑定组**惰性创建并缓存**（键是 `GltfPrimitive::material_id`），
//! 所以一次模型加载之后，每帧只是查表 + `set_bind_group`，不会重复建对象。
//! **颜色 / 金属度 / 粗糙度不在这个绑定组里**——它们是逐 draw 的（实体级
//! 材质覆盖），随 group(0) binding 2 的动态偏移走。
//!
//! # 阴影贴图（级联 CSM）
//!
//! 方向光走经典的 **shadow mapping**：先用一盏灯（第一盏开了 `casts_shadow`
//! 的方向光）把场景深度画进深度图，主 pass 里再把片元变换回光源裁剪空间、
//! 与深度图比较（PCF 3×3 软化边缘）。
//!
//! 深度图是 **4 层深度数组**，一层一个级联：相机视线方向按 practical split
//! （λ = 0.7，见 [`cascade_splits`]）切成 4 段，每段各自拟合一个正交视锥
//! （[`cascade_shadow_matrices`]）。片元按自己的视线深度选层，于是同样一张
//! 2048² 的图，近处拿到的纹素密度比"整场景一个视锥"细一个量级。
//!
//! 几个刻意的取舍：
//! - **只做方向光**。点光阴影要 6 面立方体贴图，成本高得多，留到以后。
//! - **只做一盏**。多盏灯各自的阴影贴图意味着每盏一次全场景重绘 + 一张贴图，
//!   收益远不如先把一盏做扎实。第一盏生效，后面的 `casts_shadow` 暂时忽略。
//! - **覆盖距离 = min(相机远平面, 场景延伸, 120)**，不是相机远平面：编辑器远
//!   平面常是 1000，照它切等于把 99% 的纹素花在看不见的地方。
//! - **每层吸附到纹素网格**：否则相机一动，影子边缘就随纹素相位抖（shimmer）。
//! - 拟合是**纯函数**，因此能单测（见模块尾部那组 `shadow_cascades_*`）。
//! - 深度图**常驻一张**（默认 [`DEFAULT_SHADOW_RESOLUTION`]），不随场景变化重建；
//!   灯里的 `shadow_resolution` 改了才会重建（走 [`ScenePipeline::render`] 里那条
//!   "分辨率变了就换图"的分支）。
//! - 单级拟合 [`fit_directional_shadow`] 保留着：它是"关掉 CSM"的等价退路，
//!   也是 CSM 精度账的基准（测试里拿两者的纹素尺寸直接比）。

use std::collections::{HashMap, HashSet};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use xgame_math::glam::{Mat4, Vec3};

use crate::animation::AnimationDriver;
use crate::frustum::Frustum;
use crate::gltf_loader::{Bounds, GltfMesh, GltfPrimitive, GltfVertex, MaterialParams};
use crate::material_library::{MaterialKey, MaterialLibrary};
use crate::sky::{self, SkyResources, SkyUniform};
use xgame_core::{Entity, Light, LightKind, ProjectHandle, SkyConfig};

/// 每帧最多渲染的实体数量。
///
/// 绘制数据缓冲按 [`MAX_RENDER_DRAWS`] 一次性分配（≈2 MB 显存，常驻可忽略）。
/// 超出部分仍然静默不画 —— 真到这个规模，该上的是实例化/分批，而不是继续
/// 放大这张表。
const MAX_RENDER_ENTITIES: usize = 4096;

/// 每帧最多提交的**图元绘制**（draw）数量。
///
/// 实体与 draw 不是一对一：一个多材质的 glTF 网格（材质不同 → 图元不同）
/// 单只实体就能出十几个 draw，材质覆盖后每个图元还要各自携带材质参数。
/// 给实体上限的 2 倍余量；超出静默丢弃（动态偏移越过缓冲会在校验层整帧
/// 报错，宁缺勿炸）。
const MAX_RENDER_DRAWS: usize = MAX_RENDER_ENTITIES * 2;

/// 每个蒙皮槽位预留的骨骼数（矩阵）。**必须与 WGSL 的 `BONES_PER_SLOT` 一致。**
///
/// 槽位是定长的：一条存储缓冲里所有动画实体的骨骼首尾相接，着色器靠
/// `slot * BONES_PER_SLOT + joint` 直接定位，CPU 侧也就不需要为每个实体
/// 记录偏移表。256 覆盖绝大多数人形/动物骨架；超过的在解析时告警并截断。
pub const BONES_PER_SLOT: usize = 256;

/// 骨骼缓冲的初始槽位数（约 128 KB）。不够时翻倍重建。
const MIN_BONE_SLOTS: u32 = 8;

/// 默认阴影贴图分辨率（正方形边长）。
///
/// 2048² 是"看起来干净"和"不吃太多显存"之间一个稳妥的折中：一张
/// Depth32Float 的 2048² 约 16 MB，单次全场景深度重绘也远轻于主 pass。
pub const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;

/// 首选的多重采样数（MSAA）。
///
/// 没有它，模型与地面的交界全是**硬台阶**（俗称"锯齿"）—— 一个像素要么是物体、
/// 要么是背景，没有中间值。4× 是画质与开销的常规折中（显存 ×4、光栅化贵一些，
/// 但 resolve 之后带宽回到 1×）。
pub const PREFERRED_SAMPLE_COUNT: u32 = 4;

/// 在目标格式上挑一个可用的采样数：优先 [`PREFERRED_SAMPLE_COUNT`]，不支持就退回 1。
///
/// 采样数必须**由适配器能力决定**：某些格式/后端不支持 4×，硬开会让管线创建直接
/// 失败（表现为启动就崩）。所以这里问一次能力，选好之后一路传下去。
pub fn supported_sample_count(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> u32 {
    let flags = adapter.get_texture_format_features(format).flags;
    if flags.sample_count_supported(PREFERRED_SAMPLE_COUNT) {
        PREFERRED_SAMPLE_COUNT
    } else {
        1
    }
}

/// 阴影深度比较的基础偏置（NDC 深度单位）。
///
/// shadow mapping 的经典自遮挡（acne）来自"同一根光线上的表面深度量化误差"，
/// 加一个朝着光源方向的小偏置即可。这个值在拟合出来的正交视锥下约等于
/// 零点几个世界单位 —— 宁可有一点 peter-panning，也不要满屏条纹。
const SHADOW_DEPTH_BIAS: f32 = 0.0022;

/// 阴影覆盖距离的上限（世界单位，沿**相机视线**方向量）。
///
/// # 为什么必须封顶
///
/// 级联把"相机看得见的这段距离"切成若干段，每段一张独立的深度图。段的总长
/// （[`ShadowCascades`] 的覆盖距离）直接决定最后一级的纹素尺寸：范围拉得越长，
/// 远处越糊。相机远平面在编辑器里可能是 1000，照它切就是把 99% 的纹素浪费在
/// 玩家根本看不清的地方。
///
/// 所以覆盖距离取 **min(相机远平面, 场景在视线方向的延伸, 这个上限)**：
/// 小场景自动收紧（纹素密度白拿），大场景不会无限稀释（≤120 单位）。
/// 超出的物体不投影 —— 比整屏阴影一起烂掉要好。Unity 的 Shadow Distance、
/// Godot 的 `directional_shadow_max_distance` 是同一个旋钮，将来接到质量设置里。
const MAX_SHADOW_DISTANCE: f32 = 120.0;

/// 级联层数。
///
/// 4 级是"阴影距离 120 / 2048²"这组参数下的常规折中（Unity 默认 4 级、
/// Godot 默认 4 级）：再多一级要多一次全场景深度重绘，少一级则每级跨度太大、
/// 近处纹素密度不够。上限见 [`MAX_SHADOW_CASCADES`]（= uniform 数组长度）。
pub const SHADOW_CASCADES: usize = 4;

/// 级联层的**最大**数量。
///
/// 这个值必须在三处一致：本常量、[`ShadowUniform`] 里矩阵数组的长度、
/// WGSL 的 `array<mat4x4<f32>, 4>`。改的时候别漏（漏了是"着色器读到越界
/// 内存"，画面表现为随机的阴影跳变）。
pub const MAX_SHADOW_CASCADES: usize = 4;

/// 级联切分的"对数 / 均匀"混合系数（practical split scheme）。
///
/// 纯对数切分（λ=1）让近处的段极窄、远处极宽，符合透视精度分布，但第一级
/// 常常窄到连脚下的地面都装不下；纯均匀切分（λ=0）又浪费了近处的精度。
/// 0.7 是 PSSM 论文推荐的折中值，也是各引擎的常用值。
const SHADOW_SPLIT_LAMBDA: f32 = 0.7;

/// 单帧支持的最大光源数。
///
/// 这个值在三处必须一致：本常量、[`LightsUniform`] 的数组长度、WGSL 里的
/// `MAX_LIGHTS`。改的时候别漏。
pub const MAX_LIGHTS: usize = 8;

/// 一帧的绘制统计（合批效果与剔除效果的读数口）。
///
/// 存在的理由：实例化把"绘制项"与"draw call"解耦之后，画面正确性可以靠像素
/// 测试守，但"合批到底有没有生效"只有计数看得见 —— 基准与测试都需要一个
/// 能断言 `opaque_batches == 1` 的地方，而不是从帧耗时反推。
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// 不透明批次（一批 = 一次实例化 `draw_indexed`）。
    pub opaque_batches: usize,
    /// 不透明实例数（= 主 pass 的不透明绘制项数；批次数 ≤ 实例数）。
    pub opaque_instances: usize,
    /// 半透明绘制项数。半透明要按距离排序，逐项一次 draw call，不参与合批。
    pub blended_draws: usize,
    /// 因整个实体落在相机视锥外（且不投影）而跳过的实体数。
    pub culled_entities: usize,
    /// 被剔除的绘制项数（按图元计，≈ 省下的实例槽位）。
    pub culled_instances: usize,
    /// 只在阴影 pass 出现的不透明批次：相机看不见、但影子会进画面。
    pub shadow_only_batches: usize,
    /// 只在阴影 pass 出现的不透明实例数。
    pub shadow_only_instances: usize,
}

impl FrameStats {
    /// 本帧**主 pass** 真正发出的 draw call 数（阴影 pass 复用不透明批次，
    /// 不重复计；阴影专用批次单独记在 [`Self::shadow_only_batches`]）。
    pub fn draw_calls(&self) -> usize {
        self.opaque_batches + self.blended_draws
    }

    /// 被剔除的绘制项占"收集到的全部绘制项"的比例（0..=1）。
    ///
    /// 剔除是否真的在干活，看这一个数就够：相机一转，它就跟着动。
    pub fn cull_ratio(&self) -> f32 {
        let total = self.opaque_instances
            + self.culled_instances
            + self.shadow_only_instances
            + self.blended_draws;
        if total == 0 {
            0.0
        } else {
            self.culled_instances as f32 / total as f32
        }
    }
}

// ---------------------------------------------------------------------------
// GPU uniform 类型（必须与 WGSL 布局逐字节对齐）
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    /// 视图投影矩阵的逆：天空盒片元由 NDC 反推视线方向（见 sky.rs）。
    ///
    /// 逆矩阵每帧在 **CPU** 侧解算，而不是在着色器里逐像素求逆 —— 后者
    /// 每像素一次 4×4 求逆是纯浪费（全屏像素都算同一个矩阵）。
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
}

/// 单盏光源的 GPU 表示。
///
/// 方向光和点光共用一个结构体，靠 `pos_type.w` 区分（0=方向光，1=点光）——
/// 这样着色器里只需要一个循环，不必写两套分支逻辑。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct GpuLight {
    /// 方向光：xyz = 光线传播方向，w = 0。
    /// 点光：  xyz = 世界坐标位置，w = 1。
    pos_type: [f32; 4],
    /// rgb = 光色（线性），a = 强度。
    color_intensity: [f32; 4],
    /// x = 这盏灯是否使用阴影贴图（1 / 0），y = 点光范围，其余预留。
    ///
    /// 只允许**一盏**方向光取 1（见模块头的说明）；多盏同时标记会让它们
    /// 共享同一张深度图，后者的矩阵会覆盖前者，结果是错误但"看起来像阴影"
    /// 的画面 —— 所以在 CPU 侧就只标记第一盏。
    shadow_params: [f32; 4],
}

impl GpuLight {
    const ZERO: Self = Self {
        pos_type: [0.0; 4],
        color_intensity: [0.0; 4],
        shadow_params: [0.0; 4],
    };
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct LightsUniform {
    /// rgb = 环境光，a = 有效光源数量（着色器按它截断循环）。
    ambient: [f32; 4],
    lights: [GpuLight; MAX_LIGHTS],
}

/// 阴影相关的 uniform（主 pass 与阴影 pass 共用同一块缓冲）。
///
/// `params`：x = 一个纹素的 UV 尺寸（PCF 采样间隔），y = 基础深度偏置，
/// z = 阴影是否生效（0 会让片元着色器直接跳过采样），w = 预留。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ShadowUniform {
    /// 每层的光源视图投影矩阵（世界 → 该层光源裁剪空间）。
    ///
    /// 用**定长数组**而不是四个独立字段：着色器按 `cascade` 下标取用，
    /// 数组正好是"下标寻址"的表达；定长也保证 ABI 不随层数变。
    light_view_proj: [[[f32; 4]; 4]; MAX_SHADOW_CASCADES],
    /// 每层的远平面距离（沿相机视线方向的线性深度）：`splits[i]` 之后归下一层。
    splits: [f32; MAX_SHADOW_CASCADES],
    /// xyz = 相机前向（单位向量），w 预留。
    forward: [f32; 4],
    /// x = 1/分辨率（PCF 偏移），y = 深度偏置，z = 开关（>0.5 表示启用），
    /// w = 有效层数（0 = 没有阴影图可用）。
    params: [f32; 4],
}

/// 阴影 pass 的 uniform 步长（字节）。
///
/// 阴影 pass 每层只用一个矩阵，四层共用一条缓冲、靠**动态偏移**切换 ——
/// 比"每层一个绑定组"省三次绑定组创建，比 push constant 少一个设备特性依赖。
/// 步长必须是 `min_uniform_buffer_offset_alignment`（wgpu 保证 ≥256）的倍数，
/// 所以取 256 而不是 64。
const SHADOW_PASS_STRIDE: u64 = 256;

/// 一次绘制项（= 一个实例）的全部数据：模型矩阵 + 蒙皮槽位 + 材质。
///
/// 按**槽位**紧密排布在 `draw_buffer`（storage）里：一个图元一个实例，槽位
/// 由批次分组决定（见模块头"实例化"一节）。材质之所以放这里而不是"每材质
/// 一个绑定组"：**实体级材质覆盖**（[`xgame_core::MaterialOverride`]）让同一
/// 份 glTF 材质在不同实体上可以有不同颜色 / 金属度，材质参数因此是逐实例的
/// 而不是逐资产的；旧方案（贴图 + 参数打包进绑定组、按 material_id 缓存）
/// 在逐实体覆盖下要么每次改色重建绑定组、要么缓存键随数值无限膨胀。
///
/// 字节布局与三个 WGSL 着色器里的 `DrawUniform` 一一对应（`material` 在 WGSL
/// 侧展平为 `base_color` / `material` 两个 vec4）。改字段顺序 = 改着色器 ABI，
/// 两边必须一起改；大小还必须等于 WGSL 数组步长（`size_of` 已由测试钉死）。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct DrawUniform {
    model: [[f32; 4]; 4],
    /// `.x` = 蒙皮槽位 + 1（0 = 不蒙皮）。其余 3 个分量预留。
    params: [u32; 4],
    /// 本实例的有效材质（模型自带参数 ⊕ 实体级覆盖后的结果）。
    material: MaterialUniform,
}

/// `DrawUniform` 的 GPU 步长（字节）。WGSL 里 `array<DrawUniform>` 的元素步长
/// 由结构体布局决定 —— 必须与这里的值一致，运行期断言在单测里。
const DRAW_STRIDE: usize = std::mem::size_of::<DrawUniform>();

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct MaterialUniform {
    /// rgb = 基础色因子，a = 不透明度。
    base_color: [f32; 4],
    /// x = 金属度（0 = 绝缘体 / 1 = 金属），y = 粗糙度，zw 预留。
    params: [f32; 4],
}

/// 粗糙度下限。GGX 的 D 项在 α→0（完美镜面）时发散：除以零 → NaN 传染
/// 整帧（一个 `roughness: 0.0` 的材质能黑掉整个画面）。0.04 的镜面已经
/// 光滑到看不出和 0 的区别，但分母永远安全。
pub const MIN_ROUGHNESS: f32 = 0.04;

// ---------------------------------------------------------------------------
// 实例化：批次（一批 = 一次实例化 draw call）
// ---------------------------------------------------------------------------

/// 一批同图元的实例。
#[derive(Copy, Clone, Debug, Default)]
struct DrawBatch {
    /// 本批第一个实例在 `draw_buffer` 里的槽位（= `draw_indexed` 的 first_instance）。
    first: u32,
    /// 实例数。
    count: u32,
    /// 解析图元用：实体下标 + 图元下标（见下）。
    ///
    /// 批次表是**跨帧复用的字段**，不能驻留 `&GltfPrimitive` —— 那会把
    /// `render` 入参的生命周期绑进 `ScenePipeline` 自己（自引用）。绘制时经
    /// [`batch_prim`] 解析，每个批次一次（批次少的时候几乎零成本，批次多
    /// 的时候本来就省不下什么）。
    entity: u32,
    prim: u32,
    /// 这批用哪套材质绑定组。
    ///
    /// **批次键必须包含它**：同一个图元被两个实体分别指定不同 `.mat` 时，
    /// 颜色 / 贴图不同，合成一批会让后一个材质覆盖前一个。
    material: MaterialKey,
}

/// 待写的实例数据。
///
/// 为什么不能边收集边写槽位：槽位要在批次**计数**之后才能定（前缀和），
/// 而实例数据在收集时就已经算好了（材质合并、世界矩阵都在那个循环里）。
/// 攒进这里比"再遍历一遍实体重算"便宜（重算要再做一遍网格查找与材质合并）。
struct PendingDraw {
    /// 所属不透明批次。
    batch: u32,
    uniform: DrawUniform,
}

/// 半透明绘制项：距离是排序键（远→近），每项独立一次 draw call。
struct BlendedDraw {
    /// 到相机的距离平方（只比较大小，省一次开方）。
    dist: f32,
    batch: DrawBatch,
    uniform: DrawUniform,
}

/// 从批次记录解析出图元（见 [`DrawBatch::entity`] 的说明）。
fn batch_prim<'a>(
    entities: &[Entity],
    model_cache: &'a HashMap<String, GltfMesh>,
    batch: &DrawBatch,
) -> Option<&'a GltfPrimitive> {
    let entity = entities.get(batch.entity as usize)?;
    let mesh = model_cache.get(entity.model.as_str())?;
    mesh.primitives.get(batch.prim as usize)
}

/// 静态图元返回共享缓冲，形变图元返回该实体独占的缓冲。
/// wgpu 资源句柄克隆只增加引用计数，不复制显存。
fn batch_vertex_buffer<'a>(
    entities: &[Entity],
    buffers: &'a HashMap<(xgame_core::EntityId, usize), crate::morph::MorphBuffer>,
    prim: &'a GltfPrimitive,
    batch: &DrawBatch,
) -> wgpu::BufferSlice<'a> {
    let key = (
        entities[batch.entity as usize].id,
        prim as *const GltfPrimitive as usize,
    );
    buffers.get(&key).map_or_else(
        || prim.vertex_buffer.slice(..),
        |output| output.buffer.slice(..),
    )
}

impl MaterialUniform {
    /// 原样携带 glTF 的 metallic-roughness 参数，PBR BRDF 直接消费。
    ///
    /// 历史上这里做的是"翻译成 Blinn-Phong 的 specular/shininess"——
    /// 那次换算为了"看起来对"牺牲了材料身份（金属度成了高光强度、
    /// 粗糙度成了光泽度），反射行为必然错；换成 Cook-Torrance 之后
    /// 两个值就是自有语义，只做安全夹取。
    fn from_params(p: &MaterialParams) -> Self {
        Self {
            base_color: p.base_color,
            params: [
                p.metallic.clamp(0.0, 1.0),
                p.roughness.clamp(MIN_ROUGHNESS, 1.0),
                0.0,
                0.0,
            ],
        }
    }
}

/// 一帧的光照环境（环境光 + 光源列表）。
///
/// 渲染管线不直接依赖 `Scene`，而是收这个"视图"——编辑器、游戏、以及未来
/// 的预览窗口可以各自组装光照，不必都去构造一个完整的 `Scene`。
#[derive(Clone, Copy, Debug)]
pub struct SceneLighting<'a> {
    /// 全局环境光（线性 RGB）。
    pub ambient: [f32; 3],
    /// 场景光源。超过 [`MAX_LIGHTS`] 的部分会被忽略。
    pub lights: &'a [Light],
}

impl<'a> SceneLighting<'a> {
    pub fn new(ambient: [f32; 3], lights: &'a [Light]) -> Self {
        Self { ambient, lights }
    }
}

// ---------------------------------------------------------------------------
// 阴影拟合（纯函数，便于单测）
// ---------------------------------------------------------------------------

/// 求整场景的**世界空间**包围盒：把所有带模型的实体，按各自的网格本地包围盒
/// 经世界矩阵变换后并起来。
///
/// 变换包围盒只能"变换 8 个角点再重新求盒"——旋转之后对角线方向会变，
/// 不能直接用对角线推。这正是 [`Bounds::transformed`] 做的事；
/// 这里用它的解析式版本（[`Bounds::transformed_affine`]，对仿射矩阵逐值等价），
/// 每帧要跑一遍全实体，省下的 7 次点变换不是常数级冗余而是真实开销。
///
/// 场景里没有任何能画出来的东西时返回 `None`（调用方据此关闭阴影）。
pub fn scene_bounds(
    entities: &[Entity],
    world_matrices: &[Mat4],
    model_cache: &HashMap<String, GltfMesh>,
) -> Option<Bounds> {
    let mut bounds = Bounds::EMPTY;
    for (i, entity) in entities.iter().enumerate() {
        if entity.model.is_empty() {
            continue;
        }
        let Some(mesh) = model_cache.get(&entity.model) else {
            continue;
        };
        let world = world_matrices
            .get(i)
            .copied()
            .unwrap_or_else(|| entity.transform.local_matrix());
        bounds = bounds.union(&mesh.bounds.transformed_affine(&world));
    }
    if bounds.is_empty() {
        None
    } else {
        Some(bounds)
    }
}

/// 蒙皮网格的剔除余量（半长倍率）。
///
/// 蒙皮在 GPU 顶点着色器里发生，CPU 侧的 `GltfMesh::bounds` 只是**绑定姿态**
/// 的外壳。走路、挥拳、翻滚都会把肢体甩到盒外；50% 余量（3× 体积）能覆盖
/// 常见骨骼动画的摆幅，又不会让角色永远"不可剔除"。精确做法是逐帧用当前
/// 骨骼重新求蒙皮包围盒 —— 那要在 CPU 上再算一遍蒙皮，收益抵不上成本。
const SKINNED_BOUNDS_PADDING: f32 = 1.5;

/// 剔除用的世界包围盒（网格级）。
///
/// `animated` 只在**蒙皮网格**上生效：给绑定姿态包围盒加余量（见
/// [`SKINNED_BOUNDS_PADDING`]）。静态网格（含内置图元）走精确解析式。
fn cull_bounds(mesh: &GltfMesh, world: &Mat4, animated: bool) -> Bounds {
    let bounds = mesh.bounds.transformed_affine(world);
    if !animated || mesh.skins.is_empty() || bounds.is_empty() {
        return bounds;
    }
    let center = bounds.center();
    let extent = bounds.half_extent() * SKINNED_BOUNDS_PADDING;
    Bounds {
        min: center - extent,
        max: center + extent,
    }
}

/// 选出唯一一盏使用阴影贴图的方向光 —— 在**打包后**的光源列表里的下标。
///
/// `packed` 是"过滤掉强度为 0 的、截断到 [`MAX_LIGHTS`] 之后"的顺序，也就是
/// 着色器看到的数组顺序；标记必须落在同一个下标上，否则会给错误的灯打上阴影。
///
/// 只认方向光：点光阴影要 6 面立方体贴图，不在这一版的范围内。
fn pick_shadow_slot(packed: &[&Light]) -> Option<usize> {
    packed
        .iter()
        .position(|l| l.casts_shadow && l.kind == LightKind::Directional)
}

/// 一帧的级联阴影参数（CPU 拟合的结果）。
///
/// 一个索引对应一张阴影深度图的一层；`splits[i]` 是第 i 层的**远平面距离**
/// （沿相机视线方向量，也是着色器选层的唯一依据）。
#[derive(Copy, Clone, Debug)]
pub struct ShadowCascades {
    /// 实际使用的层数（1..=[`MAX_SHADOW_CASCADES`]）。
    pub count: usize,
    /// 每层的光源视图投影矩阵（世界 → 该层的光源裁剪空间）。
    pub view_proj: [Mat4; MAX_SHADOW_CASCADES],
    /// 每层的远平面距离（视线方向）：`splits[i]` 之后归下一层。
    pub splits: [f32; MAX_SHADOW_CASCADES],
    /// 相机前向（单位向量）。着色器用它把世界坐标换算成"视线方向深度"。
    pub forward: Vec3,
    /// 每层一个纹素覆盖的世界尺寸（米）。诊断 / 实测用：它直接决定影子边缘
    /// 能有多锐，也是"CSM 把精度提了多少倍"的唯一量化口径。
    pub texel_world: [f32; MAX_SHADOW_CASCADES],
}

/// 从**逆视图投影矩阵**反推视锥的 8 个角点（近平面 4 个 + 远平面 4 个）。
///
/// # 为什么从矩阵反推，而不是让宿主多传一份 near/far
///
/// 级联必须切**正在被投影的那个视锥**。让宿主另外传一份相机参数，就有"传的
/// 和实际用的不是同一组"的可能（编辑器自由相机、游戏相机、无头截图三套调用
/// 各自组装矩阵），错了的表现是阴影分层错位、或近处一段莫名没有影子 —— 都很
/// 难查。反推只需要 `view_proj`（渲染的输入之一）与 wgpu 的 `[0,1]` 深度约定。
///
/// 顺序固定为 (x,y) = (-1,-1), (1,-1), (1,1), (-1,1)；近平面在前。
fn frustum_corners(inv_view_proj: &Mat4) -> Option<[[Vec3; 4]; 2]> {
    let mut quads = [[Vec3::ZERO; 4]; 2];
    for (q, z) in [0.0f32, 1.0].into_iter().enumerate() {
        for (i, (x, y)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .into_iter()
            .enumerate()
        {
            let p = inv_view_proj.project_point3(Vec3::new(x, y, z));
            if !p.is_finite() {
                return None;
            }
            quads[q][i] = p;
        }
    }
    Some(quads)
}

/// 级联的远平面距离（practical split scheme，见 [`SHADOW_SPLIT_LAMBDA`]）。
///
/// 返回长度 = `count`；最后一项**恰好**等于 `far` —— 覆盖距离是硬边界，
/// 浮点误差让它少一点点，就会在正好那一圈留下一道"没有影子"的环。
pub fn cascade_splits(near: f32, far: f32, count: usize) -> Vec<f32> {
    if count == 0 {
        return Vec::new();
    }
    let near = near.max(1e-3);
    let far = far.max(near + 1e-3);
    let span = far - near;
    // 相邻两层的**最小**间距。用 span 的一个小份数（而不是固定 1e-3）：
    // near ≈ far（坏相机）时固定值会把三段挤到同一个数上，切分不再单调。
    let min_gap = span / (count as f32 * 4.0);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let p = (i + 1) as f32 / count as f32;
        let log = near * (far / near).powf(p);
        let uniform = near + span * p;
        let mut d = SHADOW_SPLIT_LAMBDA * log + (1.0 - SHADOW_SPLIT_LAMBDA) * uniform;
        // 单调性兜底：近平面很小时 (far/near)^p 与线性插值可能挤到一起，
        // 而"第 i 层的远平面 ≤ 第 i-1 层的"会让切分出现负跨度。
        if let Some(prev) = out.last() {
            d = d.max(*prev + min_gap);
        }
        out.push(d.min(far));
    }
    if let Some(last) = out.last_mut() {
        *last = far;
    }
    out
}

/// 拟合方向光的级联阴影（CSM）。
///
/// # 做法
///
/// 1. 从 `inv_view_proj` 反推相机视锥的 8 个角点；
/// 2. **覆盖距离** = min(相机远平面, 场景 8 角点在视线方向的最远投影,
///    [`MAX_SHADOW_DISTANCE`]) —— 小场景自动收紧，不让纹素浪费在空气里；
/// 3. 按 practical split 把 `[near, 覆盖距离]` 切成 `count` 段，每段取一个
///    子视锥（沿视锥棱边线性插值，透视 / 正交下都**精确**）；
/// 4. 每段各拟合一个紧致的正交视锥，并把范围**吸附到纹素网格**上
///    （否则相机一动，影子边缘就随纹素相位抖 —— 也就是常说的 shimmer）。
///
/// 返回 `None` 表示矩阵退化 / 场景为空 / 覆盖距离没有厚度，调用方应关闭阴影。
pub fn cascade_shadow_matrices(
    inv_view_proj: &Mat4,
    camera_pos: Vec3,
    direction: Vec3,
    scene: Bounds,
    resolution: u32,
    count: usize,
) -> Option<ShadowCascades> {
    if count == 0 || count > MAX_SHADOW_CASCADES || scene.is_empty() {
        return None;
    }
    let [near_q, far_q] = frustum_corners(inv_view_proj)?;

    let quad_center = |q: &[Vec3; 4]| (q[0] + q[1] + q[2] + q[3]) / 4.0;
    let near_center = quad_center(&near_q);
    let far_center = quad_center(&far_q);
    let axis = far_center - near_center;
    if !axis.is_finite() || axis.length_squared() < 1e-9 {
        return None;
    }
    let forward = axis.normalize();

    let near_d = (near_center - camera_pos).dot(forward).max(0.0);
    let far_plane_d = (far_center - camera_pos).dot(forward);
    if !far_plane_d.is_finite() || far_plane_d <= near_d + 1e-3 {
        return None;
    }

    // 覆盖距离：场景在视线方向上"用到"的最远处。
    let mut scene_far = f32::NEG_INFINITY;
    for corner in scene.corners() {
        scene_far = scene_far.max((corner - camera_pos).dot(forward));
    }
    let mut far_d = scene_far.min(MAX_SHADOW_DISTANCE).min(far_plane_d);
    // 场景整个在相机背后（自由相机看空处）时投影不出有效的覆盖距离：
    // 退回相机远平面 ---- 宁可多覆盖一段空气，也不要阴影凭空消失。
    if !far_d.is_finite() || far_d < near_d + 0.5 {
        far_d = far_plane_d.min(MAX_SHADOW_DISTANCE);
    }
    if far_d <= near_d + 1e-3 {
        return None;
    }

    let splits = cascade_splits(near_d, far_d, count);

    // 零向量方向（数据坏了）退化成"垂直向下"，与 `Light::normalized_direction` 一致。
    let dir = if direction.length_squared() > 1e-12 {
        direction.normalize()
    } else {
        Vec3::new(0.0, -1.0, 0.0)
    };
    // up 不能与视线平行（look_at 会算出 NaN）。光接近竖直时改用 Z 当上方向。
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };

    // 视锥棱边上"距离 d"的那一点 = 近角点与远角点的线性插值：透视下
    // `near + (d-near)/near·v_near` 与插值式恒等，正交下棱边本来就平行。
    let span = far_plane_d - near_d;
    let at = |d: f32| ((d - near_d) / span).clamp(0.0, 1.0);

    let res = resolution.max(1) as f32;
    let mut out = ShadowCascades {
        count,
        view_proj: [Mat4::IDENTITY; MAX_SHADOW_CASCADES],
        splits: [0.0; MAX_SHADOW_CASCADES],
        forward,
        texel_world: [0.0; MAX_SHADOW_CASCADES],
    };

    for c in 0..count {
        let d0 = if c == 0 { near_d } else { splits[c - 1] };
        let d1 = splits[c];
        let (t0, t1) = (at(d0), at(d1));

        let mut corners = [Vec3::ZERO; 8];
        for k in 0..4 {
            corners[k] = near_q[k].lerp(far_q[k], t0);
            corners[k + 4] = near_q[k].lerp(far_q[k], t1);
        }

        let center = corners.iter().fold(Vec3::ZERO, |a, c| a + *c) / 8.0;
        let radius = corners
            .iter()
            .fold(0.0f32, |m, c| m.max((*c - center).length()))
            .clamp(0.5, MAX_SHADOW_DISTANCE);
        let eye = center - dir * (radius * 2.0 + 1.0);
        let view = Mat4::look_at_rh(eye, center, up);

        // 紧致光束：逐轴求这 8 个角点在光源空间的范围。拿球半径当宽高会把
        // 纹素浪费在空角上（子视锥是梯形的，包围球比它胖得多）。
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for corner in corners {
            let p = view.transform_point3(corner);
            if !p.is_finite() {
                return None;
            }
            min = min.min(p);
            max = max.max(p);
        }

        // x / y 用同一个尺寸（正方形范围）：纹素在屏幕上是方的，PCF 的 3×3
        // 核不会在某个方向上被拉长成条。
        let half_raw = ((max.x - min.x).max(max.y - min.y) * 0.5).max(0.05);
        // 向上取整到"整数个纹素"，于是每层的正交宽度恰好是 resolution 个纹素。
        let texel_raw = 2.0 * half_raw / res;
        let half = (half_raw / texel_raw).ceil() * texel_raw;
        let texel = 2.0 * half / res;
        // 吸附：把盒子中心对齐到纹素边界。相机连续移动时，中心在"纹素格子"
        // 上跳，采样相位不变 ⇒ 影子边缘不抖（shimmer 的根因就是相位在漂）。
        let cx = ((min.x + max.x) * 0.5 / texel).floor() * texel;
        let cy = ((min.y + max.y) * 0.5 / texel).floor() * texel;

        // 深度方向按半径给余量：太紧会把投影物自己的影子裁掉（peter-panning
        // 反而更明显）。
        //
        // 注意符号：`look_at_rh` 的视空间里"前方"是 -z，而 glam 的正交投影要的是
        // **正的距离**（z_view = -near → 0，z_view = -far → 1）。所以最近 / 最远
        // 距离分别来自 -max.z / -min.z；直接把 z 值传进去会把整段深度推到
        // [0,1] 之外（表现是"全屏没有影子"）。
        let pad = radius * 0.05 + 1.0;
        let near_z = (-max.z - pad).max(0.01);
        let far_z = -min.z + pad;
        if far_z <= near_z + 1e-3 {
            return None;
        }

        let proj = Mat4::orthographic_rh(cx - half, cx + half, cy - half, cy + half, near_z, far_z);
        out.view_proj[c] = proj * view;
        out.splits[c] = d1;
        out.texel_world[c] = texel;
    }

    Some(out)
}

/// 拟合**单级**阴影视锥：整场景包围盒 → 光源裁剪空间。
///
/// 保留它有两个理由：级联层数为 1 时它就是唯一那层（A/B 基准的另一条腿），
/// 以及"关掉 CSM"的退路需要一条与上线前行为等价的路径。
///
/// 拟合一盏方向光的正交阴影视锥矩阵（世界 → 光源裁剪空间）。
///
/// # 做法
///
/// 阴影贴图是一张固定分辨率（比如 2048²）的深度图。它只能覆盖一个有限的
/// 立方体区域，所以视锥要**刚好罩住场景**：太大 → 每纹素覆盖的世界尺寸变大，
/// 精度被稀释成一团噪点；太小 → 边缘物体直接没影子。
///
/// 用一个**外接球**定尺寸（球半径 = 包围盒半对角线长）而不是直接用盒：
/// 光源方向是任意角度，球在任何方向上投影都一样大，不用为每个角度重算
/// 包围盒在该方向上的投影范围。代价是比紧致的盒视锥多浪费一些纹素 —— 对
/// "先把阴影做出来"这个阶段是划算的。
///
/// 返回 `None` 表示场景为空或退化，调用方应关闭阴影。
pub fn fit_directional_shadow(direction: Vec3, scene: Bounds) -> Option<Mat4> {
    if scene.is_empty() {
        return None;
    }

    // 零向量方向（数据坏了）退化成"垂直向下"，与 Light::normalized_direction 一致。
    let dir = if direction.length_squared() > 1e-12 {
        direction.normalize()
    } else {
        Vec3::new(0.0, -1.0, 0.0)
    };

    let center = scene.center();
    // 退化场景（只有一个点）时给个下限，否则正交矩阵会塌成一个奇异矩阵；
    // 上限见 `MAX_SHADOW_DISTANCE`（防"有物体飞出视野"把深度精度吃光）。
    let radius = scene.half_extent().length().clamp(0.5, MAX_SHADOW_DISTANCE);

    // up 不能与视线平行（look_at 会算出 NaN）。光接近竖直时改用 Z 当上方向。
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };

    let distance = radius * 2.0 + 1.0;
    // 光源在场景中心逆着光的方向退开，视线正对着它 —— 光从"相机"位置照出去。
    let eye = center - dir * distance;
    let view = Mat4::look_at_rh(eye, center, up);

    // 半径留 2% 余量，避免正好贴在边缘的物体被裁掉一角。
    let r = radius * 1.02;
    // near/far 要罩住整个球：最近点距 eye `distance - radius`，最远点 `distance + radius`。
    let near = (distance - radius - 1.0).max(0.01);
    let far = distance + radius + 1.0;
    let proj = Mat4::orthographic_rh(-r, r, -r, r, near, far);
    Some(proj * view)
}

// ---------------------------------------------------------------------------
// 内联 WGSL 着色器
// ---------------------------------------------------------------------------

const SCENE_VERT_SRC: &str = r#"
struct CameraUniform { view_proj: mat4x4<f32>, inv_view_proj: mat4x4<f32>, camera_pos: vec4<f32> }
// 逐实例数据：模型矩阵 + 蒙皮槽位（params.x）+ 材质（base_color / material）。
// 一个实例一段、紧密排布；下标就是 @builtin(instance_index)（= 本批起始槽位
// + 批内序号，见 CPU 侧 `draw_indexed` 的 first_instance）。
struct DrawUniform  {
    model: mat4x4<f32>,
    params: vec4<u32>,
    base_color: vec4<f32>,
    material: vec4<f32>,
}

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(2) var<storage, read> draws: array<DrawUniform>;
// 所有动画实体的骨骼矩阵排在同一条存储缓冲里，按槽位分段（见 BONES_PER_SLOT）。
@group(0) @binding(6) var<storage, read> bones: array<mat4x4<f32>>;

// 每个蒙皮槽位预留的骨骼数。必须与 CPU 侧的 `BONES_PER_SLOT` 一致。
const BONES_PER_SLOT: u32 = 256u;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) tex_coord: vec2<f32>,
    @location(4) joints: vec4<f32>,
    @location(5) weights: vec4<f32>,
}
struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) tex_coord: vec2<f32>,
    // 材质随实例数据来，但片元阶段拿不到 instance_index —— 用 flat 插值带过去
    // （同一实例的所有顶点取同一个值，provoking vertex 取谁都一样）。
    @location(4) @interpolate(flat) base_color: vec4<f32>,
    @location(5) @interpolate(flat) material: vec4<f32>,
}

// 手写 3x3 求逆：non-uniform scale 下法线必须用逆转置矩阵变换，
// 直接乘 model 会让法线歪掉、光照方向全错。
fn mat3_inverse(m: mat3x3<f32>) -> mat3x3<f32> {
    let a = m[0][0]; let b = m[0][1]; let c = m[0][2];
    let d = m[1][0]; let e = m[1][1]; let f = m[1][2];
    let g = m[2][0]; let h = m[2][1]; let i = m[2][2];
    let det = a * (e*i - f*h) - b * (d*i - f*g) + c * (d*h - e*g);
    let inv_det = 1.0 / det;
    return mat3x3<f32>(
        (e*i - f*h)*inv_det, (c*h - b*i)*inv_det, (b*f - c*e)*inv_det,
        (f*g - d*i)*inv_det, (a*i - c*g)*inv_det, (c*d - a*f)*inv_det,
        (d*h - e*g)*inv_det, (b*g - a*h)*inv_det, (a*e - b*d)*inv_det,
    );
}

@vertex
fn vs_main(input: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    let draw = draws[instance_index];
    var pos = input.position;
    var nrm = input.normal;

    // 蒙皮：skinned = Σ wₖ · boneₖ · position（权重归一化后）。
    // 骨骼矩阵是**模型空间**里的变换（关节全局 × 逆绑定矩阵），所以这里不碰
    // draw.model —— 世界变换照旧在下面统一做，与非蒙皮路径完全共用。
    // params.x == 0 时整段跳过：绑定姿态 = 原始顶点，画面与不开动画逐位一致。
    let skin_slot = draw.params.x;
    if (skin_slot > 0u) {
        let wsum = input.weights.x + input.weights.y + input.weights.z + input.weights.w;
        if (wsum > 0.0) {
            let base = (skin_slot - 1u) * BONES_PER_SLOT;
            let inv = 1.0 / wsum;
            let skin = (input.weights.x * bones[base + u32(input.joints.x)]
                      + input.weights.y * bones[base + u32(input.joints.y)]
                      + input.weights.z * bones[base + u32(input.joints.z)]
                      + input.weights.w * bones[base + u32(input.joints.w)]) * inv;
            pos = (skin * vec4<f32>(input.position, 1.0)).xyz;
            nrm = (skin * vec4<f32>(input.normal, 0.0)).xyz;
        }
    }

    let world_pos = draw.model * vec4<f32>(pos, 1.0);
    let m = draw.model;
    let m33 = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
    let normal_mat = transpose(mat3_inverse(m33));
    var out: VertexOutput;
    out.clip_pos = camera.view_proj * world_pos;
    out.world_normal = normal_mat * nrm;
    out.world_pos = world_pos.xyz;
    out.color = input.color;
    out.tex_coord = input.tex_coord;
    out.base_color = draw.base_color;
    out.material = draw.material;
    return out;
}
"#;

const SCENE_FRAG_SRC: &str = r#"
const MAX_LIGHTS: u32 = 8u;

struct CameraUniform { view_proj: mat4x4<f32>, inv_view_proj: mat4x4<f32>, camera_pos: vec4<f32> }
struct GpuLight      { pos_type: vec4<f32>, color_intensity: vec4<f32>, shadow_params: vec4<f32> }
struct LightsUniform { ambient: vec4<f32>, lights: array<GpuLight, 8> }
struct ShadowUniform {
    // 级联矩阵数组：下标 = 层号。长度必须等于 Rust 侧 `MAX_SHADOW_CASCADES`。
    light_view_proj: array<mat4x4<f32>, 4>,
    // 每层远平面（相机视线方向的线性深度）。用 vec4 而不是 array<f32, 4>：
    // uniform 里 array<f32,4> 的元素步长是 16，与 Rust 的 [f32;4] 对不上。
    splits: vec4<f32>,
    // xyz = 相机前向（单位向量）
    forward: vec4<f32>,
    // x = 1/分辨率, y = 深度偏置, z = 开关, w = 有效层数
    params: vec4<f32>,
}

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(1) var<uniform> lights: LightsUniform;
@group(0) @binding(3) var shadow_tex: texture_depth_2d_array;
@group(0) @binding(4) var shadow_smp: sampler_comparison;
@group(0) @binding(5) var<uniform> shadow: ShadowUniform;
// 7..11：天空（类型与函数在 sky.rs 的公共片段里，这里只声明绑定）。
// 关着天空时这几张表照常绑着（布局固定），片元按开关位整段跳过采样。
@group(0) @binding(7) var<uniform> sky: SkyUniform;
@group(0) @binding(8) var<uniform> skysh: SkySH;
@group(0) @binding(9) var env_cube: texture_cube<f32>;
@group(0) @binding(10) var env_smp: sampler;
@group(0) @binding(11) var brdf_lut: texture_2d<f32>;
@group(1) @binding(0) var base_color_tex: texture_2d<f32>;
@group(1) @binding(1) var base_color_smp: sampler;

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) tex_coord: vec2<f32>,
    // 逐实例材质（顶点阶段从 draws[] 取出后 flat 传下来，见顶点着色器）。
    @location(4) @interpolate(flat) base_color: vec4<f32>,
    @location(5) @interpolate(flat) material: vec4<f32>,
}

// 3x3 PCF：把 9 个比较结果平均一下，边缘不那么硬。
//
// 用 `textureSampleCompareLevel` 而不是 `textureSampleCompare`——后者要走隐式
// 导数，而它处在一个"按灯循环里的 if 分支"中（非一致控制流），WGSL 的一致性
// 分析会因此报错。深度图没有 mip，本来也只需要 level 0，正好。
fn shadow_factor(world_pos: vec3<f32>, ndotl: f32) -> f32 {
    // 选层：级联是按**相机视线方向的线性深度**切的，不是 NDC 深度。
    // 用线性深度是因为 `splits` 本身就是世界单位，跨相机/跨投影都可比。
    let count = u32(max(shadow.params.w, 0.0));
    if (count == 0u) {
        return 1.0;
    }
    let view_depth = dot(world_pos - camera.camera_pos.xyz, shadow.forward.xyz);
    var cascade = count - 1u;
    for (var i = 0u; i + 1u < count; i = i + 1u) {
        if (view_depth <= shadow.splits[i]) {
            cascade = i;
            break;
        }
    }

    let light_clip = shadow.light_view_proj[cascade] * vec4<f32>(world_pos, 1.0);
    let ndc = light_clip.xyz / light_clip.w;
    // NDC → 纹理 UV。y 要翻：NDC 的 y 朝上，纹理的 v 朝下。
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    // 落出光源视锥（含近平面之后）的部分当作"完全受光"，否则边缘会被裁出一条黑带。
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    // 掠射角下采样点跨度大、自遮挡更严重，所以偏置随 ndotl 变小而放大。
    let bias = shadow.params.y * (1.0 + 2.0 * (1.0 - ndotl));
    let depth = ndc.z;
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * shadow.params.x;
            sum = sum + textureSampleCompareLevel(
                shadow_tex,
                shadow_smp,
                uv + offset,
                i32(cascade),
                depth - bias,
            );
        }
    }
    return sum / 9.0;
}

// --- PBR（metallic-roughness 工作流）：GGX 分布 + Smith 几何 + Schlick 菲涅尔 ---
// 三个函数是 Cook-Torrance 的标准拼装（对照 Filament / learnopengl 的参考实现），
// 除了 fs_main 里「π 校准」一节注明的那处，没有自定义魔改。
const PI: f32 = 3.141592653589793;

fn distribution_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-6);
}

// Schlick-GGX（直接光的 k = α/2 形式）拆成两条可见性函数相乘。
fn smith_visibility(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let k = alpha * 0.5;
    let gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return gv * gl;
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - clamp(cos_theta, 0.0, 1.0), 5.0);
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // 反照率 = 贴图 × 材质因子 × 顶点色（三者都是"乘法叠加"的直觉语义）
    let sampled = textureSample(base_color_tex, base_color_smp, input.tex_coord);
    let albedo = sampled.rgb * input.base_color.rgb * input.color;
    let alpha = sampled.a * input.base_color.a;

    let metallic = input.material.x;
    let roughness = input.material.y;
    let n = normalize(input.world_normal);
    let v = normalize(camera.camera_pos.xyz - input.world_pos);
    let n_dot_v = max(dot(n, v), 1e-4);

    // 绝缘体的基础反射率 4%；金属的 F0 = 自身反照率（有色镜面反射）。
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let alpha_g = roughness * roughness;

    // 环境项①：全局 ambient 当作**均匀环境辐射**（IBL 的最低阶近似）。
    // - 漫反射环境按金属度分流：金属没有漫反射；
    // - 镜面环境按菲涅尔加权：金属靠它保住"反射环境色"这条最重要的感知
    //   线索（均匀环境看不出形状，但颜色对）。
    let f_env = fresnel_schlick(n_dot_v, f0);
    var color = lights.ambient.rgb * (albedo * (1.0 - metallic) + f_env);

    // 环境项②：天空 IBL（开着天空时与①**相加**）。split-sum 的标准两半：
    // - 漫反射 = L2 球谐（E/π，系数里已折好余弦卷积）× albedo × kD；
    // - 镜面 = 预过滤立方体（GGX 重要性采样，mip 由 roughness 选）× (F0·A + B)，
    //   A / B 来自 BRDF LUT。
    // 关着时整段跳过：不采样三张环境纹理，画面与天空上线前逐位一致（开关位
    // 是 uniform 里的常量，控制流一致，采样合法且便宜）。
    if (sky.sun_dir.w > 0.5) {
        let r = reflect(-v, n);
        let spec_env = textureSampleLevel(env_cube, env_smp, r, roughness * ENV_MAX_LOD).rgb;
        let brdf = textureSampleLevel(brdf_lut, env_smp, vec2<f32>(n_dot_v, roughness), 0.0).rg;
        let f_rough = f0 + (max(vec3<f32>(1.0 - roughness), f0) - f0) * pow(1.0 - n_dot_v, 5.0);
        let kd_env = (vec3<f32>(1.0) - f_rough) * (1.0 - metallic);
        color = color
            + sky_sh_eval(skysh, n) * albedo * kd_env
            + spec_env * (f0 * brdf.x + brdf.y);
    }

    let count = min(u32(lights.ambient.a), MAX_LIGHTS);
    let shadows_on = shadow.params.z > 0.5;

    for (var i = 0u; i < MAX_LIGHTS; i = i + 1u) {
        if (i >= count) { break; }
        let light = lights.lights[i];

        var l: vec3<f32>;
        var atten = 1.0;
        if (light.pos_type.w < 0.5) {
            // 方向光：pos_type.xyz 是光线传播方向，入射方向取反
            l = normalize(-light.pos_type.xyz);
        } else {
            // 点光：按到光源的距离做平滑衰减（1/(1+kd+kd²) 的标准近似）
            let to_light = light.pos_type.xyz - input.world_pos;
            let dist = length(to_light);
            l = to_light / max(dist, 1e-4);
            let range = light.shadow_params.y;
            if (range <= 0.0 || dist >= range) { continue; }
            // 保留近处旧亮度，仅在半径最后 20% 平滑收尾。
            let fade = 1.0 - smoothstep(0.8, 1.0, dist / range);
            atten = fade / (1.0 + 0.09 * dist + 0.032 * dist * dist);
        }

        let n_dot_l = max(dot(n, l), 0.0);
        // π 校准（刻意的）：历史场景的 intensity 是按"Lambert 直乘"手调的
        // （SampleScene 的太阳是 3.2）。物理正确的漫反射带 1/π，照搬会把
        // 全场砍掉 3.14 倍、老场景集体欠曝。这里把 π 并进 radiance：漫反射
        // 回到"intensity × 白面 ≈ 1.0"的老观感，镜面项一并受益（高光本来
        // 就该过曝，由后续 tonemap 收敛）。
        let radiance = light.color_intensity.rgb * light.color_intensity.a * atten * PI;

        // 只有被标记的那盏灯查阴影图；其余灯（含未受光的背光面）跳过采样。
        var shade = 1.0;
        if (shadows_on && light.shadow_params.x > 0.5 && n_dot_l > 0.0) {
            shade = shadow_factor(input.world_pos, n_dot_l);
        }

        let h = normalize(l + v);
        let n_dot_h = max(dot(n, h), 0.0);
        let v_dot_h = max(dot(v, h), 0.0);

        let d_term = distribution_ggx(n_dot_h, alpha_g);
        let g_term = smith_visibility(n_dot_v, n_dot_l, alpha_g);
        let f_term = fresnel_schlick(v_dot_h, f0);
        let specular = d_term * g_term * f_term / max(4.0 * n_dot_v * n_dot_l, 1e-4);

        // 漫反射：能量守恒地把分给镜面反射的部分扣掉（1-F），金属没有漫反射。
        let kd = (vec3<f32>(1.0) - f_term) * (1.0 - metallic);
        let diffuse = kd * albedo / PI;

        color = color + (diffuse + specular) * radiance * n_dot_l * shade;
    }

    return vec4<f32>(color, alpha);
}
"#;

/// 阴影 pass 的顶点着色器：只把顶点变换到光源裁剪空间，片元阶段不需要——
/// 深度图只关心深度，管线里 `fragment: None`。
const SHADOW_VERT_SRC: &str = r#"
// 阴影 pass 的 uniform：每层被**动态偏移**切一段，段内只有一个矩阵
// （见 Rust 侧的 `SHADOW_PASS_STRIDE`）。这里刻意不声明主 pass 的整组级联 ——
// 段长 256 字节，声明了多余字段只会读到未写入的填充。
struct ShadowPassUniform { light_view_proj: mat4x4<f32> }
// 与主 pass 同一份逐实例数据；阴影只要 model / params 两个字段，
// 但结构体声明保持与缓冲布局一致（字段齐全，未用的不读取）。
struct DrawUniform  {
    model: mat4x4<f32>,
    params: vec4<u32>,
    base_color: vec4<f32>,
    material: vec4<f32>,
}

@group(0) @binding(0) var<uniform> shadow: ShadowPassUniform;
@group(0) @binding(1) var<storage, read> draws: array<DrawUniform>;
@group(0) @binding(2) var<storage, read> bones: array<mat4x4<f32>>;

const BONES_PER_SLOT: u32 = 256u;

@vertex
fn vs_shadow(
    @location(0) position: vec3<f32>,
    @location(4) joints: vec4<f32>,
    @location(5) weights: vec4<f32>,
    @builtin(instance_index) instance_index: u32,
) -> @builtin(position) vec4<f32> {
    let draw = draws[instance_index];
    // 阴影也走蒙皮：不然跑动的角色会投出一具绑定姿态的静止影子。
    // 与主 pass 同一套分支（params.x == 0 → 原始顶点），两边永远一致。
    var pos = position;
    let skin_slot = draw.params.x;
    if (skin_slot > 0u) {
        let wsum = weights.x + weights.y + weights.z + weights.w;
        if (wsum > 0.0) {
            let base = (skin_slot - 1u) * BONES_PER_SLOT;
            let inv = 1.0 / wsum;
            let skin = (weights.x * bones[base + u32(joints.x)]
                      + weights.y * bones[base + u32(joints.y)]
                      + weights.z * bones[base + u32(joints.z)]
                      + weights.w * bones[base + u32(joints.w)]) * inv;
            pos = (skin * vec4<f32>(position, 1.0)).xyz;
        }
    }
    let world_pos = draw.model * vec4<f32>(pos, 1.0);
    return shadow.light_view_proj * world_pos;
}
"#;

// ---------------------------------------------------------------------------
// ScenePipeline
// ---------------------------------------------------------------------------

/// 共享 3D 场景渲染管线。
///
/// 创建一次，每帧调用 [`render`](Self::render)。不持有离屏纹理或交换链——
/// 由调用者传入。
pub struct ScenePipeline {
    /// 不透明变体。
    pipeline: wgpu::RenderPipeline,
    /// 半透明变体：开启 alpha 混合、关闭深度写入（否则半透明物体会
    /// 把它后面的东西提前挡掉）。
    blend_pipeline: wgpu::RenderPipeline,

    /// 全局绑定组布局。构造完 `global_bind_group` 之外，重建绑定组时也要用
    /// （阴影分辨率变化、骨骼缓冲扩容），所以去掉下划线成为实打实的字段。
    global_bgl: wgpu::BindGroupLayout,

    camera_buffer: wgpu::Buffer,
    lights_buffer: wgpu::Buffer,
    /// 逐实例数据（storage）。槽位紧密排布：一个绘制项一段 [`DRAW_STRIDE`] 字节，
    /// 实例下标由 `draw_indexed` 的 first_instance 与实例数据位置共同决定。
    draw_buffer: wgpu::Buffer,
    global_bind_group: wgpu::BindGroup,

    // -- 蒙皮 --
    /// 所有动画实体的骨骼矩阵（存储缓冲，按 [`BONES_PER_SLOT`] 定长分槽）。
    /// 扩容时重建，两个绑定组跟着换（见 [`ScenePipeline::ensure_bones_capacity`]）。
    bones_buffer: wgpu::Buffer,
    /// `bones_buffer` 当前能装下的槽位数。
    bone_slots: u32,
    /// 骨骼矩阵复用暂存（与 `draw_staging` 同理：一次 write 搞定整段）。
    bone_staging: Vec<[f32; 16]>,

    // -- 形变目标 --
    /// 本帧从剪辑里采样出来的权重（复用缓冲，避免每帧为每个角色新分配）。
    ///
    /// 只在**剪辑带权重轨道**时被写入；其余情况直接用实体的 `morph_weights`。
    morph_weights: Vec<f32>,
    /// （实体 ID，图元地址）→ 独占顶点缓冲。图元地址能区分模型热重载前后的
    /// 资源；每帧按实际使用者回收，实体删除或换模型不会留下无界 GPU 缓存。
    morph_buffers: HashMap<(xgame_core::EntityId, usize), crate::morph::MorphBuffer>,
    morph_buffers_used: HashSet<(xgame_core::EntityId, usize)>,

    // -- 阴影 --
    /// 阴影 pass 的布局：binding 0 = 阴影 uniform，binding 1 = 逐实例数据
    /// （storage 数组，阴影只用其中的模型矩阵与蒙皮槽位），binding 2 = 蒙皮骨骼。
    ///
    /// 刻意与 `global_bgl` **分开**：阴影 pass 一边把深度画进 `shadow_view`、
    /// 一边又要在 group(0) 里绑这张图 —— wgpu 会把"同一资源既当渲染目标又当
    /// 绑定资源"判成冲突。用一个不含深度贴图的小布局绕开它。
    shadow_bgl: wgpu::BindGroupLayout,
    shadow_pipeline: wgpu::RenderPipeline,
    /// 阴影 uniform（级联矩阵数组 + 切分表 + 偏置）。主 pass 与阴影 pass 都读它。
    ///
    /// 主 pass 一次读走**整组**级联（选层在片元里做）；阴影 pass 每层只取一个
    /// 矩阵，走另一条 `shadow_pass_buffer` 的动态偏移 —— 两个 pass 对"同一份
    /// 数据"的取法不同，混用一个缓冲会让两边都要迁就对方。
    shadow_buffer: wgpu::Buffer,
    /// 阴影 pass 的每层矩阵（步长 [`SHADOW_PASS_STRIDE`]，动态偏移选层）。
    shadow_pass_buffer: wgpu::Buffer,
    /// 阴影 pass 用的绑定组（阴影 uniform + draw 段）。draw 缓冲固定不变，建一次即可。
    shadow_pass_bind_group: wgpu::BindGroup,
    /// 深度比较采样器（PCF）。
    shadow_sampler: wgpu::Sampler,
    /// 阴影深度图（`MAX_SHADOW_CASCADES` 层深度数组）。分辨率变化时重建
    /// （连同 `global_bind_group` 与 `shadow_layer_views`）。
    shadow_texture: wgpu::Texture,
    /// 深度数组的整体视图：主 pass 采样用（dimension = D2Array）。
    shadow_view: wgpu::TextureView,
    /// 每层单独一张视图（dimension = D2）：阴影 pass 每层各画进一层。
    ///
    /// wgpu 不允许把"数组视图"当渲染目标的多层切片直接画 —— 只能给单层视图。
    shadow_layer_views: Vec<wgpu::TextureView>,
    shadow_resolution: u32,

    /// 天空盒 + 环境光照（IBL）资源。纹理常驻、内容按 key 重建（见
    /// [`crate::sky`] 的模块文档）；`global_bind_group` 持有它的三张表与
    /// 采样器，创建顺序上必须排在全局绑定组之前。
    sky: SkyResources,

    /// 材质绑定组 + 共享材质资产库（贴图 / 参数 / 失效代数）。
    ///
    /// 颜色参数不在这里 —— 它们逐 draw 变化（资产 + 实体级覆盖），随
    /// `draw_buffer` 每帧上传。这里只随**资产**增长，与实体数无关。
    materials: MaterialLibrary,

    /// 主 pass 的采样数（1 = 关闭 MSAA）。
    ///
    /// 主 pass 与阴影 pass 的采样数**可以不同**：阴影图只存深度、不需要抗锯齿，
    /// 所以它永远是 1。这个值只作用于两条主管线。
    sample_count: u32,

    /// 逐实例数据的**复用暂存**：按槽位顺序排好的 `DrawUniform` 字节。
    ///
    /// 为什么会需要它：`Queue::write_buffer` 每调一次都要从 staging belt 上领一段、
    /// 再记一条拷贝命令。逐实例调用的话，一个 2000 实体的场景每帧就是 2000 次小分配 +
    /// 2000 条命令。攒成一次写，开销与实例数解耦。
    ///
    /// 复用同一个 `Vec` 是为了稳态零分配：`clear()` + `resize(n, 0)` 会把要用的
    /// 那段显式清零，容量留着下次接着用。
    draw_staging: Vec<u8>,

    /// 本帧待写的实例数据（见 [`PendingDraw`]）。
    pending_draws: Vec<PendingDraw>,
    /// 本帧不透明批次。**顺序 = 首次遇到的实体顺序**（布局确定性的来源）。
    opaque_batches: Vec<DrawBatch>,
    /// 本帧半透明绘制项（每项一批，按距离远→近）。
    blended_draws: Vec<BlendedDraw>,
    /// （图元地址 + 材质 key）→ 不透明批次下标（本帧复用，`clear()` 保留容量）。
    ///
    /// 只按图元地址分组会让"同一个图元、不同 `.mat`"的两个实体被错误合批。
    batch_index: HashMap<(usize, MaterialKey, Option<xgame_core::EntityId>), u32>,
    /// 写实例数据时的每批游标（长度 = 当前正在填的批次表长度）。
    batch_cursor: Vec<u32>,

    /// 本帧**阴影专用**批次：相机视锥外、但阴影会进画面的不透明实例。
    ///
    /// 与 `opaque_batches` 分开是必须的 —— 主 pass 不能画它们（在相机外，
    /// 白费片元），阴影 pass 必须画它们。两张批次表在 `draw_buffer` 里各占
    /// 一段连续槽位，阴影 pass 依次遍历两表即可。
    shadow_batches: Vec<DrawBatch>,
    /// 与 `shadow_batches` 对应的待写实例数据。
    shadow_pending: Vec<PendingDraw>,
    /// 图元地址 → 阴影专用批次下标。
    shadow_batch_index: HashMap<(usize, Option<xgame_core::EntityId>), u32>,

    /// 视锥剔除开关（默认开）。见 [`ScenePipeline::set_frustum_culling`]。
    frustum_culling: bool,

    /// 上一帧的绘制统计（见 [`FrameStats`]）。合批与剔除是否生效的读数口。
    stats: FrameStats,

    /// 粒子 billboard 管线（见 [`crate::particles`]）。
    ///
    /// 粒子在本管线的**场景 pass 内部**画（不透明与半透明之后）：它是场景里的
    /// 东西 —— 要被几何遮住、要参与曝光 / 色调映射 / Bloom。UI 则相反，见
    /// `ui_renderer` 的模块头。
    particles: crate::particles::ParticleRenderer,
}

impl ScenePipeline {
    /// 创建渲染管线、绑定组和 uniform 缓冲。
    ///
    /// `format` 是颜色附件的纹理格式（编辑器离屏或交换链）。
    /// `sample_count` 是主 pass 的采样数（见 [`supported_sample_count`]）；
    /// 传 1 就是关闭 MSAA。**调用方必须按同一个值准备颜色/深度附件**：
    /// 采样数不匹配的话渲染通道直接校验失败。
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sample_count: u32) -> Self {
        let mut global_entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::with_capacity(12);
        global_entries.extend([
            // 0: 相机（顶点阶段要 view_proj，片元阶段要相机位置算半程向量）
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // 1: 光照
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // 2: 逐实例数据（模型矩阵 + 蒙皮槽位 + 材质）。storage 数组：
            //    一个实例一段，顶点着色器靠 `@builtin(instance_index)`
            //    直接索引（批量绘制时它从 first_instance 起递增）。
            //    逐实例而不是逐 draw/逐实体的理由见 `DrawUniform` 的说明。
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // 3: 阴影深度图（片元阶段比较采样）
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    // 深度纹理只能用 Depth 采样类型（不是 Float）。
                    // 维度必须是 D2Array：一层一个级联，着色器按 index 取层。
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            // 4: 深度比较采样器
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            // 5: 阴影 uniform（光源视图投影 + 偏置 / 开关）
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // 6: 蒙皮骨骼矩阵（顶点阶段只读存储）
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ]);
        // 7..11：天空（绑定号契约集中在 sky.rs，见那里的说明）。
        global_entries.extend(sky::global_layout_entries());
        let global_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_global_bgl"),
            entries: &global_entries,
        });

        let material_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_material_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // 阴影 pass 的布局：故意不含深度贴图本身（见结构体上 shadow_bgl 的注释）。
        let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_shadow_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        // 每层取一个矩阵，靠动态偏移选层（见 `SHADOW_PASS_STRIDE`）。
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 2: 蒙皮骨骼矩阵（与主 pass 同一条缓冲，阴影顶点也做蒙皮）
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_camera_ub"),
            contents: bytemuck::cast_slice(&[CameraUniform {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0; 4],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_lights_ub"),
            contents: bytemuck::bytes_of(&LightsUniform {
                ambient: [0.06, 0.06, 0.08, 1.0],
                lights: [GpuLight::ZERO; MAX_LIGHTS],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let draw_buffer_size = MAX_RENDER_DRAWS as u64 * DRAW_STRIDE as u64;
        let draw_buffer_init = vec![0u8; draw_buffer_size as usize];
        let draw_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_draw_sb"),
            contents: &draw_buffer_init,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // -- 阴影资源 --
        let shadow_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_shadow_ub"),
            contents: bytemuck::bytes_of(&ShadowUniform {
                light_view_proj: [[[f32::NAN; 4]; 4]; MAX_SHADOW_CASCADES],
                splits: [0.0; MAX_SHADOW_CASCADES],
                forward: [0.0; 4],
                // z = 0：默认阴影关闭。没有标记任何灯时片元着色器不采样这张图。
                params: [
                    1.0 / DEFAULT_SHADOW_RESOLUTION as f32,
                    SHADOW_DEPTH_BIAS,
                    0.0,
                    0.0,
                ],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // 阴影 pass 的每层矩阵：一条缓冲、`MAX_SHADOW_CASCADES` 段、动态偏移选段。
        let shadow_pass_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_shadow_pass_ub"),
            size: SHADOW_PASS_STRIDE * MAX_SHADOW_CASCADES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene_shadow_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // `Comparison` 采样器配 `textureSampleCompareLevel` 用：
            // 硬件替我们做"片元深度 vs 存图深度"的比较，省掉一次纹理读取 + 分支。
            compare: Some(wgpu::CompareFunction::Less),
            ..Default::default()
        });

        let shadow_resolution = DEFAULT_SHADOW_RESOLUTION;
        let (shadow_texture, shadow_view, shadow_layer_views) =
            Self::create_shadow_target(device, shadow_resolution);

        // 蒙皮骨骼缓冲：先给一小段（8 槽 × 256 骨 ≈ 128 KB），实体多了再翻倍。
        let bones_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_bones_sb"),
            size: MIN_BONE_SLOTS as u64 * BONES_PER_SLOT as u64 * 64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // 天空资源：必须在全局绑定组之前建 —— 组里持有它的 uniform 缓冲、
        // 立方体与 LUT 的视图（换内容不换对象，见 sky.rs 的生命周期说明）。
        let sky = SkyResources::new(device, &global_bgl, format, sample_count);

        let global_bind_group = Self::create_global_bind_group(
            device,
            &global_bgl,
            &camera_buffer,
            &lights_buffer,
            &draw_buffer,
            &shadow_view,
            &shadow_sampler,
            &shadow_buffer,
            &bones_buffer,
            &sky,
        );

        let shadow_pass_bind_group = Self::create_shadow_pass_bind_group(
            device,
            &shadow_bgl,
            &shadow_pass_buffer,
            &draw_buffer,
            &bones_buffer,
        );

        let pipeline = Self::build_pipeline(
            device,
            &global_bgl,
            &material_bgl,
            format,
            sample_count,
            false,
        );
        let blend_pipeline = Self::build_pipeline(
            device,
            &global_bgl,
            &material_bgl,
            format,
            sample_count,
            true,
        );
        let shadow_pipeline = Self::build_shadow_pipeline(device, &shadow_bgl);

        Self {
            pipeline,
            blend_pipeline,
            global_bgl,
            materials: MaterialLibrary::new(material_bgl),
            camera_buffer,
            lights_buffer,
            draw_buffer,
            global_bind_group,
            bones_buffer,
            bone_slots: MIN_BONE_SLOTS,
            bone_staging: Vec::new(),
            morph_weights: Vec::new(),
            morph_buffers: HashMap::new(),
            morph_buffers_used: HashSet::new(),
            shadow_bgl,
            shadow_pipeline,
            shadow_buffer,
            shadow_pass_buffer,
            shadow_pass_bind_group,
            shadow_sampler,
            shadow_texture,
            shadow_view,
            shadow_layer_views,
            shadow_resolution,
            sky,
            sample_count,
            draw_staging: Vec::new(),
            pending_draws: Vec::new(),
            opaque_batches: Vec::new(),
            blended_draws: Vec::new(),
            batch_index: HashMap::new(),
            batch_cursor: Vec::new(),
            shadow_batches: Vec::new(),
            shadow_pending: Vec::new(),
            shadow_batch_index: HashMap::new(),
            frustum_culling: true,
            stats: FrameStats::default(),
            particles: crate::particles::ParticleRenderer::new(device, format, sample_count),
        }
    }

    /// 主 pass 使用的采样数（1 = 未开 MSAA）。
    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    /// 上一帧的绘制统计（见 [`FrameStats`]）。
    pub fn frame_stats(&self) -> FrameStats {
        self.stats
    }

    /// 开关视锥剔除（默认开；关掉只用于 A/B 基准与像素等价测试）。
    ///
    /// 剔除是**保守**的：开与关的画面必须逐位一致。生产路径没有理由关它；
    /// 这个开关的存在是为了让"剔除到底省了多少"能被同机背靠背量出来，而不是
    /// 靠版本对比猜。
    pub fn set_frustum_culling(&mut self, enabled: bool) {
        self.frustum_culling = enabled;
    }

    /// 当前是否开了视锥剔除。
    pub fn frustum_culling(&self) -> bool {
        self.frustum_culling
    }

    /// 创建阴影深度图层（深度数组，一层一个级联）。
    ///
    /// 返回（纹理，整体 D2Array 视图，每层 D2 视图）。两套视图不是冗余：
    /// 采样必须用数组视图（着色器按 index 取层），渲染必须用单层视图
    /// （wgpu 不允许把数组视图直接当一层的渲染目标）。
    #[allow(clippy::type_complexity)]
    fn create_shadow_target(
        device: &wgpu::Device,
        resolution: u32,
    ) -> (wgpu::Texture, wgpu::TextureView, Vec<wgpu::TextureView>) {
        let size = wgpu::Extent3d {
            width: resolution.max(1),
            height: resolution.max(1),
            depth_or_array_layers: MAX_SHADOW_CASCADES as u32,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene_shadow_depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("scene_shadow_depth_array"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let layers = (0..MAX_SHADOW_CASCADES as u32)
            .map(|i| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("scene_shadow_depth_layer"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        (texture, view, layers)
    }

    #[allow(clippy::too_many_arguments)]
    fn create_global_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        camera_buffer: &wgpu::Buffer,
        lights_buffer: &wgpu::Buffer,
        draw_buffer: &wgpu::Buffer,
        shadow_view: &wgpu::TextureView,
        shadow_sampler: &wgpu::Sampler,
        shadow_buffer: &wgpu::Buffer,
        bones_buffer: &wgpu::Buffer,
        sky: &SkyResources,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_global_bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: draw_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: shadow_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: bones_buffer.as_entire_binding(),
                },
                // 7..11：天空。对象的身份不变（内容变），所以这里没有"重建
                // 绑定组"的需求 —— 这正是纹理固定分配换来的。
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: sky.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: sky.sh_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&sky.env_view),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(&sky.env_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(&sky.brdf_view),
                },
            ],
        })
    }

    fn create_shadow_pass_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        shadow_buffer: &wgpu::Buffer,
        draw_buffer: &wgpu::Buffer,
        bones_buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_shadow_pass_bg"),
            layout,
            entries: &[
                // 阴影 pass 的 uniform：必须**显式写段长**。
                // `as_entire_binding()` 会把绑定固定成"整条缓冲"，wgpu 于是只允许
                // 动态偏移 = 0（偏移后就越界），四层全部画到同一层上。
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: shadow_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: draw_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: bones_buffer.as_entire_binding(),
                },
            ],
        })
    }

    /// 若目标分辨率与当前不一致，重建阴影深度图并更新依赖它的全局绑定组。
    ///
    /// 只在灯里的 `shadow_resolution` 真的变了时才发生；正常一帧是零开销的
    /// 一次整数比较。重建必须连 `global_bind_group` 一起换 —— 它内部持有
    /// 旧的 `TextureView`。
    fn ensure_shadow_resolution(&mut self, device: &wgpu::Device, resolution: u32) {
        let resolution = resolution.max(1);
        if resolution == self.shadow_resolution {
            return;
        }
        let (texture, view, layers) = Self::create_shadow_target(device, resolution);
        self.shadow_texture = texture;
        self.shadow_view = view;
        self.shadow_layer_views = layers;
        self.shadow_resolution = resolution;
        self.global_bind_group = Self::create_global_bind_group(
            device,
            &self.global_bgl,
            &self.camera_buffer,
            &self.lights_buffer,
            &self.draw_buffer,
            &self.shadow_view,
            &self.shadow_sampler,
            &self.shadow_buffer,
            &self.bones_buffer,
            &self.sky,
        );
    }

    /// 骨骼缓冲至少要装下 `slots` 个槽位；不够就翻倍重建。
    ///
    /// 重建必须同时换 `global_bind_group` 和 `shadow_pass_bind_group` ——
    /// 它们内部持有旧 buffer 的引用。因此只能在**开 pass 之前**调用（本帧
    /// 就是收集完绘制项之后、`begin_render_pass` 之前），在 pass 中途换绑定
    /// 会被 wgpu 直接判非法。
    ///
    /// 稳态零开销：`slots <= bone_slots` 时就是一次整数比较。
    fn ensure_bones_capacity(&mut self, device: &wgpu::Device, slots: u32) {
        if slots <= self.bone_slots {
            return;
        }
        let mut capacity = self.bone_slots.max(MIN_BONE_SLOTS);
        while capacity < slots {
            capacity *= 2;
        }
        let bytes = capacity as u64 * BONES_PER_SLOT as u64 * 64; // mat4x4 = 64 字节
        self.bones_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_bones_sb"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.bone_slots = capacity;
        self.global_bind_group = Self::create_global_bind_group(
            device,
            &self.global_bgl,
            &self.camera_buffer,
            &self.lights_buffer,
            &self.draw_buffer,
            &self.shadow_view,
            &self.shadow_sampler,
            &self.shadow_buffer,
            &self.bones_buffer,
            &self.sky,
        );
        self.shadow_pass_bind_group = Self::create_shadow_pass_bind_group(
            device,
            &self.shadow_bgl,
            &self.shadow_buffer,
            &self.draw_buffer,
            &self.bones_buffer,
        );
    }

    fn build_pipeline(
        device: &wgpu::Device,
        global_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        sample_count: u32,
        blend: bool,
    ) -> wgpu::RenderPipeline {
        let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_vs"),
            source: wgpu::ShaderSource::Wgsl(SCENE_VERT_SRC.into()),
        });
        let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_fs"),
            // 天空公式（sky_radiance / sky_sh_eval）只有一份实现：公共片段在
            // 这里是**前缀**，绑定号由场景 FS 自己声明（见 sky.rs 的说明）。
            source: wgpu::ShaderSource::Wgsl(
                format!("{}{}", sky::SKY_COMMON_WGSL, SCENE_FRAG_SRC).into(),
            ),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_pl"),
            bind_group_layouts: &[global_bgl, material_bgl],
            push_constant_ranges: &[],
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(if blend {
                "scene_pipeline_blend"
            } else {
                "scene_pipeline"
            }),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs,
                entry_point: Some("vs_main"),
                buffers: &[GltfVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    // 不透明用 REPLACE；半透明用标准的 src-alpha / 1-src-alpha 混合
                    blend: Some(if blend {
                        wgpu::BlendState::ALPHA_BLENDING
                    } else {
                        wgpu::BlendState::REPLACE
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                // 半透明物体关掉背面剔除——否则看穿玻璃杯时会缺一半内壁
                cull_mode: if blend { None } else { Some(wgpu::Face::Back) },
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: !blend,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count.max(1),
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        })
    }

    /// 阴影 pass 的管线：**只有顶点阶段**（无片元），深度写入开、背面剔除。
    ///
    /// 无片元的管线只能配 `color_attachments: &[]`，正是"纯深度预渲染"要的形态。
    /// 剔除背面（而非正面）：正面剔除能缓解 peter-panning，但会让单面几何
    /// （内置 Plane 就只有一个面）从光的方向看过去整个消失、投不出影子。
    /// 对"开箱即用的图元"来说，看得见影子比少一点 peter-panning 重要。
    fn build_shadow_pipeline(
        device: &wgpu::Device,
        shadow_bgl: &wgpu::BindGroupLayout,
    ) -> wgpu::RenderPipeline {
        let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_shadow_vs"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_VERT_SRC.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_shadow_pl"),
            bind_group_layouts: &[shadow_bgl],
            push_constant_ranges: &[],
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene_shadow_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs,
                entry_point: Some("vs_shadow"),
                buffers: &[GltfVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                // 顺手用一下硬件的深度偏置：斜率相关的那一份交给它，
                // 着色器里只再补一个常数偏置（PCF 采样点的跨度靠那句补）。
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    }

    /// 把**已打包**的光源列表编成 GPU 可用的 uniform。
    ///
    /// `packed` 由调用方按"过滤掉强度为 0 的、截断到 [`MAX_LIGHTS`]"的顺序备好；
    /// `shadow_slot` 是其中那盏使用阴影贴图的方向光的下标
    /// （由 [`pick_shadow_slot`] 决定）。只有它会打上 `shadow_params.x = 1`。
    fn build_lights(
        ambient: [f32; 3],
        packed: &[&Light],
        shadow_slot: Option<usize>,
    ) -> LightsUniform {
        let mut uniform = LightsUniform {
            ambient: [ambient[0], ambient[1], ambient[2], 0.0],
            lights: [GpuLight::ZERO; MAX_LIGHTS],
        };

        let mut count = 0usize;
        for light in packed {
            let color = [
                light.color[0],
                light.color[1],
                light.color[2],
                light.intensity,
            ];
            uniform.lights[count] = match light.kind {
                LightKind::Directional => {
                    let d = light.normalized_direction();
                    GpuLight {
                        pos_type: [d[0], d[1], d[2], 0.0],
                        color_intensity: color,
                        shadow_params: [0.0; 4],
                    }
                }
                LightKind::Point => GpuLight {
                    pos_type: [light.position[0], light.position[1], light.position[2], 1.0],
                    color_intensity: color,
                    shadow_params: [
                        0.0,
                        if light.range.is_finite() {
                            light.range.max(0.0)
                        } else {
                            0.0
                        },
                        0.0,
                        0.0,
                    ],
                },
            };
            count += 1;
        }

        if let Some(slot) = shadow_slot {
            if slot < count {
                uniform.lights[slot].shadow_params[0] = 1.0;
            }
        }

        uniform.ambient[3] = count as f32;
        uniform
    }

    /// 绑定项目（共享材质资产从这里读 `.mat` / 贴图 / 导入设置）。
    ///
    /// 三个宿主（编辑器 / 独立游戏 / 无头截图）必须在开始渲染前调用一次；
    /// 不绑定也能渲染，只是实体上的 `material_asset` 会被忽略（退回模型材质）。
    pub fn set_project(&mut self, project: Option<ProjectHandle>) {
        self.materials.set_project(project);
    }

    /// 让所有已缓存的资产材质 / 贴图失效，下一帧重新读盘。
    ///
    /// 编辑器保存 `.mat`、改贴图导入设置、或资产刷新后调用。
    pub fn invalidate_materials(&mut self) {
        self.materials.invalidate();
    }

    /// 材质库失效代数（编辑器渲染指纹用它判断"要不要重画"）。
    pub fn material_revision(&self) -> u64 {
        self.materials.revision()
    }

    /// 执行一帧场景渲染。
    ///
    /// 写入 camera / lights / model uniform，按"先不透明、后半透明（远→近）"
    /// 的顺序发出绘制调用。调用者需提前用 `queue.submit()` 提交 `encoder`。
    ///
    /// # 关于 `msaa_color_view`
    ///
    /// `color_view` 是**最终要被人看的那张**（交换链视图 / 交给 egui 的解析目标），
    /// 它永远是 1 采样。开了 MSAA 时，渲染实际发生在 `msaa_color_view`（多采样），
    /// 由 `resolve_target` 解析到 `color_view` —— 这就是 MSAA 的标准接法：
    /// 光栅化按多采样做覆盖判断，最后一次性降采样，边缘于是有中间色而不是硬台阶。
    ///
    /// 采样数必须与管线一致，否则渲染通道直接校验失败。传 `None` 表示未开 MSAA。
    ///
    /// # 关于 `world_matrices`
    ///
    /// 着色器要的是**世界矩阵**，而实体自己存的是**局部变换**（层级在
    /// `xgame_core` 里维护）。层级解算不放在渲染器里，是为了让"世界矩阵怎么算"
    /// 只有一份实现（`Scene::world_matrices`），渲染器只消费结果。
    /// 调用方传入的 `world_matrices` 必须与 `entities` 同序同长；长度不足的
    /// 位置退回该实体的局部矩阵，不会 panic。
    ///
    /// # 关于 `jitter_ndc`（TAA 的亚像素抖动）
    ///
    /// 非零时把相机投影按它平移（`[(0,0)]` = 不抖动，逐位与旧画面一致）。
    /// **只动相机 uniform**：剔除视锥与级联阴影拟合仍然用未抖动的 `view_proj`
    /// —— 阴影的"整数纹素宽度 + 中心吸附"经不起每帧半个像素的来回拉扯，
    /// 抖进去就是影子边缘持续抖动（比锯齿难看得多）。
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        msaa_color_view: Option<&wgpu::TextureView>,
        depth_view: &wgpu::TextureView,
        entities: &[Entity],
        world_matrices: &[Mat4],
        model_cache: &HashMap<String, GltfMesh>,
        anim: &AnimationDriver,
        view_proj: &Mat4,
        jitter_ndc: [f32; 2],
        camera_pos: &Vec3,
        lighting: SceneLighting<'_>,
        sky: SkyConfig,
        particle_quads: &[xgame_particle::ParticleQuad],
    ) {
        // --- 光源打包 + 阴影准备（都在 CPU 侧，先算完再开 pass）---
        let packed: Vec<&Light> = lighting
            .lights
            .iter()
            .filter(|l| l.is_active())
            .take(MAX_LIGHTS)
            .collect();
        let shadow_slot = pick_shadow_slot(&packed);

        // 分辨率取自那盏灯（0 = 用默认值）。**必须先定分辨率**：级联拟合要用它
        // 算每层纹素的世界尺寸（进而决定吸附步长），顺序反了就得重拟合一遍。
        let shadow_resolution = match shadow_slot {
            Some(slot) => match packed[slot].shadow_resolution {
                0 => DEFAULT_SHADOW_RESOLUTION,
                r => r.max(1),
            },
            None => self.shadow_resolution,
        };
        if shadow_slot.is_some() {
            self.ensure_shadow_resolution(device, shadow_resolution);
        }

        // 级联阴影：按**当前相机的视锥**切段，每段拟合一个光源正交视锥。
        // 取不到包围盒（空场景）或视锥退化时 `None` —— 整条阴影链路关掉。
        let cascades = match shadow_slot {
            Some(slot) => scene_bounds(entities, world_matrices, model_cache).and_then(|bounds| {
                let light = packed[slot];
                cascade_shadow_matrices(
                    &view_proj.inverse(),
                    *camera_pos,
                    Vec3::from(light.normalized_direction()),
                    bounds,
                    self.shadow_resolution,
                    SHADOW_CASCADES,
                )
            }),
            None => None,
        };
        let shadow_enabled = cascades.is_some();

        // --- 每帧全局 uniform ---
        //
        // TAA 的抖动只叠在相机矩阵上：阴影拟合（上面的 `cascades`）与剔除视锥
        // 用的是**未抖动**的 `view_proj`，理由见函数文档。
        let render_view_proj = crate::taa::jitter_projection(*view_proj, jitter_ndc);
        queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[CameraUniform {
                view_proj: render_view_proj.to_cols_array_2d(),
                inv_view_proj: render_view_proj.inverse().to_cols_array_2d(),
                camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 1.0],
            }]),
        );
        queue.write_buffer(
            &self.lights_buffer,
            0,
            bytemuck::bytes_of(&Self::build_lights(lighting.ambient, &packed, shadow_slot)),
        );

        // 主 pass 的级联 uniform：没阴影时矩阵填 0（shader 靠 params.w = 0 整段跳过，
        // 不会读到它们），有阴影时按层填有效值，其余层留 0。
        let mut shadow_uniform = ShadowUniform {
            light_view_proj: [[[0.0; 4]; 4]; MAX_SHADOW_CASCADES],
            splits: [0.0; MAX_SHADOW_CASCADES],
            forward: [0.0; 4],
            params: [
                1.0 / self.shadow_resolution as f32,
                SHADOW_DEPTH_BIAS,
                if shadow_enabled { 1.0 } else { 0.0 },
                0.0,
            ],
        };
        if let Some(c) = &cascades {
            shadow_uniform.forward = [c.forward.x, c.forward.y, c.forward.z, 0.0];
            shadow_uniform.params[3] = c.count as f32;
            for i in 0..c.count {
                shadow_uniform.light_view_proj[i] = c.view_proj[i].to_cols_array_2d();
                shadow_uniform.splits[i] = c.splits[i];
                // 阴影 pass 每层段只放矩阵：动态偏移选层（见 `SHADOW_PASS_STRIDE`）。
                queue.write_buffer(
                    &self.shadow_pass_buffer,
                    i as u64 * SHADOW_PASS_STRIDE,
                    bytemuck::cast_slice(&c.view_proj[i].to_cols_array()),
                );
            }
        }
        queue.write_buffer(&self.shadow_buffer, 0, bytemuck::bytes_of(&shadow_uniform));

        // --- 天空 / 环境（内容 key 变了才重建）---
        //
        // 必须在 `begin_render_pass` 之前：预过滤立方体与 BRDF LUT 各自要开
        // 自己的 pass，一帧的 encoder 里 pass 不能嵌套。关着天空时只写 80
        // 字节 uniform，不碰任何资源（旧画面逐位一致的落脚点）。
        self.sky.prepare(
            queue,
            encoder,
            SkyUniform::build(&sky, sky::sun_from_lights(&packed)),
        );

        // --- 视锥剔除：先把两个视锥提出来 ---
        //
        // 相机视锥在 `view_proj` 退化时拿不到（`None`）—— 此时整段不剔除，
        // 宁可多画也不能因为一个坏矩阵把整场景判成不可见。
        let camera_frustum = if self.frustum_culling {
            Frustum::from_view_proj(view_proj)
        } else {
            None
        };
        // 阴影视锥只在真的开了阴影时才有意义；矩阵退化时保守地认为所有
        // 物体都投影（`is_none_or`）。
        //
        // 剔除用**最后一级**（覆盖最远、最宽）：它能见 = 至少某一层能见。
        // 用第 0 层会漏掉"近处看不见、影子却落在远处"的那批物体。
        let shadow_frustum = cascades.as_ref().and_then(|c| {
            c.view_proj
                .get(c.count.saturating_sub(1))
                .and_then(Frustum::from_view_proj)
        });

        // --- 收集绘制项：分组（不透明）+ 排序键（半透明），实例数据攒进 staging ---
        // 不透明与半透明分开收集：半透明要按到相机的距离从远到近排序，
        // 否则两个半透明物体前后叠在一起时，谁的 alpha 生效取决于绘制顺序；
        // 不透明按图元分组实例化（见模块头"实例化"一节）。
        //
        // 上一帧的残留先清掉。所有收集缓冲都是复用字段（稳态零分配）：
        // `clear()` 只把长度归零，容量留着下次接着用。
        self.draw_staging.clear();
        self.pending_draws.clear();
        self.opaque_batches.clear();
        self.blended_draws.clear();
        self.batch_index.clear();
        self.shadow_batches.clear();
        self.shadow_pending.clear();
        self.shadow_batch_index.clear();
        self.morph_buffers_used.clear();
        // 骨骼暂存同理：本帧每个动画实体占一个新槽，槽位随实体序单调递增。
        self.bone_staging.clear();
        let mut bone_slots_used = 0u32;
        // 本帧用掉的**实例槽位**数（不透明实例 + 半透明绘制项 + 阴影专用实例），
        // 上限 [`MAX_RENDER_DRAWS`]：超出静默丢弃（见该常量的说明）。
        let mut draw_slots_used = 0usize;
        // 剔除读数：被整实体跳过的数量 / 绘制项数（按图元计）。
        let mut culled_entities = 0usize;
        let mut culled_instances = 0usize;

        for (i, entity) in entities.iter().enumerate() {
            if i >= MAX_RENDER_ENTITIES || draw_slots_used >= MAX_RENDER_DRAWS {
                break;
            }
            if entity.model.is_empty() {
                continue;
            }
            let Some(mesh) = model_cache.get(&entity.model) else {
                continue;
            };

            // 世界矩阵由调用方解算好（层级在 core 里维护）；越界时退回局部矩阵，
            // 保证即使调用方少传了一段也不会画出诡异的东西。
            let world = world_matrices
                .get(i)
                .copied()
                .unwrap_or_else(|| entity.transform.local_matrix());

            // -- 视锥剔除（实体级，先于蒙皮 / 材质 / 实例数据）--
            //
            // 用**网格级**世界包围盒：网格内部各图元共享一个物体，逐图元求交
            // 省下的那点实例写不值得再乘一次矩阵。蒙皮网格给余量（见
            // [`cull_bounds`]），否则动画会把肢体甩到绑定盒外。
            //
            // 三种去向：
            //   相机内               → 主 pass（阴影 pass 复用同一批）
            //   相机外、影子里       → `shadow_batches`（只投影，不进主 pass）
            //   两边都外             → 整实体跳过
            let mut draw_in_main = true;
            if let Some(camera_frustum) = camera_frustum {
                let bounds = cull_bounds(mesh, &world, entity.animator.is_some());
                let in_camera = camera_frustum.intersects_bounds(&bounds);
                let in_shadow = shadow_enabled
                    && shadow_frustum
                        .as_ref()
                        .is_none_or(|f| f.intersects_bounds(&bounds));
                if !in_camera && !in_shadow {
                    culled_entities += 1;
                    culled_instances += mesh.primitives.len();
                    continue;
                }
                draw_in_main = in_camera;
            }

            // -- 蒙皮槽位 --
            //
            // 四个条件齐了才算：有时钟（实体带 animator 且被 tick 过）、组件还在
            // （防条目残留：万一某个宿主没 tick 就渲染，摘掉 animator 的实体不该
            // 继续蒙皮）、驱动解析出的剪辑在模型里、模型有蒙皮。任何一条不满足
            // → params.x = 0，着色器走原始顶点，与动画功能上线前逐位一致。
            //
            // 剪辑名一律问驱动（状态机模式下由状态表解析，Animator.clip 只是
            // 单剪辑模式的配置），别回头读组件。
            let mut params = [0u32; 4];
            // 剪辑里的形变权重轨道（口型 / 表情动画）。**不要求模型有蒙皮** ——
            // 静态网格同样可以带 morph，所以这段放在骨骼分支外面单独采。
            self.morph_weights.clear();
            let mut clip_drives_morph = false;
            if let (Some((clip_name, time, looping)), true) =
                (anim.playback(entity.id), entity.animator.is_some())
            {
                if let Some(clip) = mesh.clip(clip_name) {
                    let t = crate::animation::clip_time(time, clip.duration, looping);
                    clip_drives_morph =
                        crate::animation::sample_morph_weights(clip, t, &mut self.morph_weights);
                }
                let skin_idx = mesh.primitives.iter().find_map(|p| p.skin);
                if let (Some(clip), Some(skin_idx)) = (mesh.clip(clip_name), skin_idx) {
                    if let Some(skin) = mesh.skins.get(skin_idx) {
                        let t = crate::animation::clip_time(time, clip.duration, looping);
                        // 换剪辑/换状态交叉淡入中：旧剪辑按它自己的时钟与循环方式
                        // 采样，两份局部姿势按 alpha 混合。旧剪辑已经不在模型里
                        // （资产改名等）就退回硬切 —— 宁可跳一下，不能画错。
                        let fade = anim.fade_from(entity.id).and_then(|f| {
                            mesh.clip(f.clip).map(|c| {
                                let prev_t =
                                    crate::animation::clip_time(f.t, c.duration, f.looping);
                                (c, prev_t, f.alpha)
                            })
                        });
                        // 层栈 = 单层 additive 快捷方式（先求值）++ 配置的 layers。
                        // 剪辑不在模型里的层直接不进栈 —— 这层不生效，基础姿势不受
                        // 影响（与"旧淡出剪辑缺失退硬切"同一取舍：宁缺勿错）。
                        let mut stack: Vec<crate::animation::LayerInput> = Vec::new();
                        if let Some(a) = anim.additive(entity.id) {
                            if let Some(c) = mesh.clip(a.clip) {
                                let add_t = crate::animation::clip_time(a.t, c.duration, a.looping);
                                stack.push(crate::animation::LayerInput {
                                    clip: c,
                                    t: add_t,
                                    weight: a.weight,
                                    mode: xgame_core::AnimLayerMode::Additive,
                                    mask: &[],
                                });
                            }
                        }
                        for l in anim.layers(entity.id) {
                            if l.clip.is_empty() {
                                continue;
                            }
                            let Some(c) = mesh.clip(&l.clip) else {
                                continue;
                            };
                            let l_t = crate::animation::clip_time(l.t, c.duration, l.looping);
                            stack.push(crate::animation::LayerInput {
                                clip: c,
                                t: l_t,
                                weight: l.weight,
                                mode: l.mode,
                                mask: &l.mask,
                            });
                        }
                        let bones = crate::animation::compute_bones_composed(
                            &mesh.nodes,
                            skin,
                            (clip, t),
                            fade,
                            &stack,
                        );
                        // 超过槽位容量的骨架（>256 骨）解析时已告警；这里截断
                        // 保证 staging 的槽距不被撑歪——宁可末梢骨不对，不能整帧错位。
                        let slot = bone_slots_used;
                        append_bone_slot(&mut self.bone_staging, &bones);
                        bone_slots_used += 1;
                        // 存槽位 + 1：0 是"不蒙皮"的哨兵，着色器里直接 `> 0u` 判分支。
                        params[0] = slot + 1;
                    }
                }
            }

            // -- 形变目标（morph target）--
            //
            // 权重来源二选一：剪辑带权重轨道时**以动画为准**（Unity 同款语义：
            // 被动画驱动的属性不听手动覆盖），否则用实体的 `morph_weights`
            // （脚本 / Inspector 写的那份）。模型自带默认权重在
            // `MorphTargets::resolve_into` 里兜底。
            //
            // 顺序必须是"形变 → 蒙皮"：形变改的是**基础顶点**，蒙皮矩阵随后
            // 才把它们搬到骨骼空间（GPU 侧做）。反过来是两套坐标系对不上。
            //
            // 没有 morph 的模型整段跳过；有权重的模型里，`apply_source` 还会
            // 比对上次值，**权重没变就不重写顶点缓冲**（静止的表情零开销）。
            if mesh.primitives.iter().any(|p| p.morph.is_some()) {
                let source: &[f32] = if clip_drives_morph {
                    &self.morph_weights
                } else {
                    &entity.morph_weights
                };
                for prim in &mesh.primitives {
                    if prim.morph.is_none() {
                        continue;
                    }
                    let key = (entity.id, prim as *const GltfPrimitive as usize);
                    let output = self
                        .morph_buffers
                        .entry(key)
                        .or_insert_with(|| prim.create_morph_buffer(device).expect("已检查 morph"));
                    if let Some(morph) = &prim.morph {
                        morph.apply_source(queue, output, source);
                    }
                    self.morph_buffers_used.insert(key);
                }
            }

            // -- 攒本实体的每个图元 --
            //
            // 模型矩阵 / 蒙皮槽位是逐实体的，材质是逐图元的：每个图元一个
            // 实例槽位（`DrawUniform`），槽位在批次分组之后统一分配（见下）。
            //
            // 半透明排序用的是**世界坐标**——实体被父节点挪过位置之后，
            // 局部坐标早就不是它在屏幕上的深度了。
            let world_pos = Vec3::new(world.w_axis.x, world.w_axis.y, world.w_axis.z);
            let dist = (world_pos - *camera_pos).length_squared();

            // -- 共享材质资产（逐实体解析一次，逐图元复用 key）--
            //
            // `bind_asset` 在**收集阶段**读盘 / 建绑定组（pass 里只能读 self）；
            // 失败返回 None，这个实体安静地退回模型自带材质。
            let asset_key = entity
                .material_asset
                .as_deref()
                .and_then(|reference| self.materials.bind_asset(device, queue, reference));

            // 模型材质的绑定组同样在收集阶段预热；有资产时贴图被资产替换，
            // 这批模型绑定组至少整个模型仍可能被别的实体用到，建了不亏。
            for prim in &mesh.primitives {
                self.materials
                    .bind_model(device, prim.material_id, &prim.texture);
            }
            // 借出的 Arc 在本实体收集期间保持有效；`Arc` 是为了不与上面
            // 对 `self.materials` 的可变借用打架（库自己还持有同一份）。
            let asset = asset_key.and_then(|key| self.materials.asset(key)).cloned();

            for (pi, prim) in mesh.primitives.iter().enumerate() {
                if draw_slots_used >= MAX_RENDER_DRAWS {
                    break;
                }
                draw_slots_used += 1;

                // 有效材质 = 模型 → 材质资产 → 实体级覆盖（后者逐字段打补丁）。
                let effective = prim
                    .material
                    .with_asset(asset.as_deref())
                    .merged_with(entity.material.as_ref());
                let material_key = asset_key.unwrap_or(MaterialKey::Model(prim.material_id));
                let uniform = DrawUniform {
                    model: world.to_cols_array_2d(),
                    params,
                    material: MaterialUniform::from_params(&effective),
                };

                if !draw_in_main {
                    // 阴影专用：相机看不见，但影子会进画面。半透明不投影
                    //（它不写主深度，混进阴影图会投出一块错误的黑斑）。
                    if effective.blend {
                        continue;
                    }
                    // 不透明：按图元地址分组（与主 pass 同一套规则），但只写
                    // 阴影批次；不需要材质绑定组（深度 pass 不采样材质）。
                    let key = (
                        prim as *const GltfPrimitive as usize,
                        prim.morph.is_some().then_some(entity.id),
                    );
                    let batch = match self.shadow_batch_index.get(&key) {
                        Some(b) => *b,
                        None => {
                            let b = self.shadow_batches.len() as u32;
                            self.shadow_batches.push(DrawBatch {
                                first: 0,
                                count: 0,
                                entity: i as u32,
                                prim: pi as u32,
                                // 深度 pass 不采样材质；填模型 key 只是让字段有值。
                                material: MaterialKey::Model(prim.material_id),
                            });
                            self.shadow_batch_index.insert(key, b);
                            b
                        }
                    };
                    self.shadow_batches[batch as usize].count += 1;
                    self.shadow_pending.push(PendingDraw { batch, uniform });
                } else if effective.blend {
                    // 半透明：一项一个批次（不做合批，见模块头），距离排序键留好。
                    self.blended_draws.push(BlendedDraw {
                        dist,
                        batch: DrawBatch {
                            first: 0, // 槽位在排序之后统一分配
                            count: 1,
                            entity: i as u32,
                            prim: pi as u32,
                            material: material_key,
                        },
                        uniform,
                    });
                } else {
                    // 不透明：按（图元地址 + 材质 key）找批次，找不到就新开一批
                    // （顺序 = 首次遇到）。材质 key 是必需的：同一个图元挂不同
                    // `.mat` 的实体不能合成一批（会互相覆盖材质）。
                    let key = (
                        prim as *const GltfPrimitive as usize,
                        material_key,
                        prim.morph.is_some().then_some(entity.id),
                    );
                    let batch = match self.batch_index.get(&key) {
                        Some(b) => *b,
                        None => {
                            let b = self.opaque_batches.len() as u32;
                            self.opaque_batches.push(DrawBatch {
                                first: 0,
                                count: 0,
                                entity: i as u32,
                                prim: pi as u32,
                                material: material_key,
                            });
                            self.batch_index.insert(key, b);
                            b
                        }
                    };
                    let batch_ref = &mut self.opaque_batches[batch as usize];
                    batch_ref.count += 1;
                    self.pending_draws.push(PendingDraw { batch, uniform });
                }
            }
        }

        // --- 槽位分配 + 实例数据落 staging ---
        self.morph_buffers
            .retain(|key, _| self.morph_buffers_used.contains(key));
        //
        // 不透明批次占 [0, n)：一批一段**连续**槽位（`first` = 前缀和），
        // 实例下标 = first + 批内序号 —— 与 `draw_indexed` 的 first_instance
        // 正好对上。半透明接在后面（顺序 = 排序后的绘制顺序，互不重叠即可）；
        // 阴影专用批次再接到最后（只有阴影 pass 读得到，主 pass 不碰）。
        //
        // 远 → 近。`partial_cmp` 对 NaN 返回 None，退化成"保持原序"即可，
        // 不值得为一个坏变换把整帧干掉。
        self.blended_draws.sort_by(|a, b| {
            b.dist
                .partial_cmp(&a.dist)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut next_slot = 0u32;
        for batch in &mut self.opaque_batches {
            batch.first = next_slot;
            next_slot += batch.count;
        }
        for draw in &mut self.blended_draws {
            draw.batch.first = next_slot;
            next_slot += 1;
        }
        for batch in &mut self.shadow_batches {
            batch.first = next_slot;
            next_slot += batch.count;
        }

        let stride = DRAW_STRIDE;
        self.draw_staging.clear();
        self.draw_staging.resize(next_slot as usize * stride, 0);
        if next_slot > 0 {
            // 不透明：每批一个游标，实例按收集顺序填进本批的连续槽位。
            self.batch_cursor.clear();
            self.batch_cursor
                .extend(self.opaque_batches.iter().map(|b| b.first));
            for draw in &self.pending_draws {
                let cursor = &mut self.batch_cursor[draw.batch as usize];
                let offset = *cursor as usize * stride;
                *cursor += 1;
                self.draw_staging[offset..offset + stride]
                    .copy_from_slice(bytemuck::bytes_of(&draw.uniform));
            }
            // 半透明：排序后一路往后排。
            for draw in &self.blended_draws {
                let offset = draw.batch.first as usize * stride;
                self.draw_staging[offset..offset + stride]
                    .copy_from_slice(bytemuck::bytes_of(&draw.uniform));
            }
            // 阴影专用：与主 pass 同一套游标填法，接在最后一段槽位。
            self.batch_cursor.clear();
            self.batch_cursor
                .extend(self.shadow_batches.iter().map(|b| b.first));
            for draw in &self.shadow_pending {
                let cursor = &mut self.batch_cursor[draw.batch as usize];
                let offset = *cursor as usize * stride;
                *cursor += 1;
                self.draw_staging[offset..offset + stride]
                    .copy_from_slice(bytemuck::bytes_of(&draw.uniform));
            }
            // 一次写完整段实例数据（见 `draw_staging` 的说明）。
            queue.write_buffer(&self.draw_buffer, 0, &self.draw_staging);
        }

        // 骨骼：容量不够先翻倍重建（连带换两个绑定组，必须在开 pass 之前），
        // 再一次性把本帧所有动画实体的骨骼写进去。
        if bone_slots_used > 0 {
            self.ensure_bones_capacity(device, bone_slots_used);
            queue.write_buffer(
                &self.bones_buffer,
                0,
                bytemuck::cast_slice(&self.bone_staging),
            );
        }

        // 统计口径 = 主 pass 真正发出的 draw call（阴影复用不透明批次，不重复计；
        // 阴影专用批次单独计数）。剔除读数给基准与编辑器 HUD 用。
        self.stats = FrameStats {
            opaque_batches: self.opaque_batches.len(),
            opaque_instances: self.pending_draws.len(),
            blended_draws: self.blended_draws.len(),
            culled_entities,
            culled_instances,
            shadow_only_batches: self.shadow_batches.len(),
            shadow_only_instances: self.shadow_pending.len(),
        };

        // --- 阴影 pass：每层画一遍深度（必须在主 pass 之前）---
        //
        // 只画**不透明**图元。半透明物体（玻璃之类）本来也不该投出实心影子，
        // 而且它们不写主深度缓冲，混到阴影图里只会投出一块错误的黑斑。
        // 相机可见的不透明批次与"相机外、影子在画面里"的专用批次都在这画；
        // 实例数据全在同一个 storage 缓冲里，只是槽位分段不同。
        //
        // 每层一个 pass（而不是一张 4 象限图集）：图集更省一次绑定切换，但
        // 每层的 UV 映射得各自缩放、PCF 在象限边界上要夹紧，代码复杂度不值
        // 那点收益。层数 = `cascades.count`，空场景时整段跳过。
        if let Some(c) = &cascades {
            for cascade in 0..c.count {
                let Some(layer_view) = self.shadow_layer_views.get(cascade) else {
                    break;
                };
                let mut spass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shadow_pass"),
                    // 深度-only 管线：没有颜色附件。
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: layer_view,
                        depth_ops: Some(wgpu::Operations {
                            // 1.0 = 最远。没被任何东西遮住的区域保持"最远"，
                            // 比较时永远判为受光。
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                spass.set_pipeline(&self.shadow_pipeline);
                // 动态偏移选层：一段 = 一个矩阵（见 `SHADOW_PASS_STRIDE`）。
                spass.set_bind_group(
                    0,
                    &self.shadow_pass_bind_group,
                    &[(cascade as u64 * SHADOW_PASS_STRIDE) as u32],
                );
                for batch in self.opaque_batches.iter().chain(self.shadow_batches.iter()) {
                    let Some(prim) = batch_prim(entities, model_cache, batch) else {
                        continue;
                    };
                    spass.set_vertex_buffer(
                        0,
                        batch_vertex_buffer(entities, &self.morph_buffers, prim, batch),
                    );
                    spass.set_index_buffer(prim.index_buffer.slice(..), prim.index_format);
                    spass.draw_indexed(
                        0..prim.num_indices,
                        0,
                        batch.first..batch.first + batch.count,
                    );
                }
            }
        }

        // 粒子：实例数据与相机基在开 pass 之前就备好（`prepare` 里做了 alpha
        // 排序，那一步不该发生在 pass 里）。没有粒子时整段跳过。
        self.particles
            .prepare(device, queue, view_proj, camera_pos, particle_quads);

        // --- 渲染通道 ---
        // 开了 MSAA 就把渲染目标换成多采样那张，并把最终视图设为 resolve target。
        let (pass_view, resolve_target) = match msaa_color_view {
            Some(msaa) => (msaa, Some(color_view)),
            None => (color_view, None),
        };
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: pass_view,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.12,
                        g: 0.12,
                        b: 0.14,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        // 不透明：写深度。全局绑定组（含实例数据的 storage）整段绑一次，
        // 逐批只换材质绑定组与顶点/索引缓冲，然后一次实例化 draw。
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.global_bind_group, &[]);
        for batch in &self.opaque_batches {
            let Some(prim) = batch_prim(entities, model_cache, batch) else {
                continue;
            };
            // 绑定组在收集阶段就备好了（见那里的说明）；真缺了说明是逻辑错误，
            // 宁可当场炸也不能拿着上一批的材质去画（画面会静默错色）。
            let material = self
                .materials
                .bind_group(batch.material)
                .expect("材质绑定组缺失");
            rpass.set_bind_group(1, material, &[]);
            rpass.set_vertex_buffer(
                0,
                batch_vertex_buffer(entities, &self.morph_buffers, prim, batch),
            );
            rpass.set_index_buffer(prim.index_buffer.slice(..), prim.index_format);
            rpass.draw_indexed(
                0..prim.num_indices,
                0,
                batch.first..batch.first + batch.count,
            );
        }

        // 天空盒：不透明之后、半透明之前（见模块头的理由）。管线是 z = 1.0
        // + `LessEqual` + 不写深度 —— 被几何盖住的像素在这里失败、空像素作画；
        // 半透明随后仍混在天空之上。
        if sky.enabled {
            self.sky.draw(&mut rpass, &self.global_bind_group);
        }

        // 半透明：不写深度，远→近。逐项一次 draw call（项数与批次无关）。
        if !self.blended_draws.is_empty() {
            rpass.set_pipeline(&self.blend_pipeline);
            rpass.set_bind_group(0, &self.global_bind_group, &[]);
            for draw in &self.blended_draws {
                let Some(prim) = batch_prim(entities, model_cache, &draw.batch) else {
                    continue;
                };
                let material = self
                    .materials
                    .bind_group(draw.batch.material)
                    .expect("材质绑定组缺失");
                rpass.set_bind_group(1, material, &[]);
                rpass.set_vertex_buffer(
                    0,
                    batch_vertex_buffer(entities, &self.morph_buffers, prim, &draw.batch),
                );
                rpass.set_index_buffer(prim.index_buffer.slice(..), prim.index_format);
                rpass.draw_indexed(
                    0..prim.num_indices,
                    0,
                    draw.batch.first..draw.batch.first + 1,
                );
            }
        }

        // 粒子：最后画。深度测试让它们被前面的几何遮住，深度写关掉，
        // 于是粒子之间不互相遮挡（也不污染后续 pass 的深度）。
        self.particles.draw(&mut rpass);
    }

    /// 上一帧的粒子绘制统计（无粒子时为全 0）。
    pub fn particle_stats(&self) -> crate::particles::ParticleDrawStats {
        self.particles.stats()
    }
}

fn append_bone_slot(staging: &mut Vec<[f32; 16]>, bones: &[Mat4]) {
    let end = staging.len() + BONES_PER_SLOT;
    staging.extend(bones.iter().take(BONES_PER_SLOT).map(|m| m.to_cols_array()));
    // 主 pass 与阴影 pass 都按固定 256 骨寻址，空位不能借用下一角色的数据。
    staging.resize(end, Mat4::IDENTITY.to_cols_array());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_r03_two_small_skeletons_have_fixed_slots() {
        let first = vec![Mat4::from_translation(Vec3::X); 2];
        let second = vec![Mat4::from_translation(Vec3::Y); 3];
        let mut staging = Vec::new();
        append_bone_slot(&mut staging, &first);
        append_bone_slot(&mut staging, &second);
        assert_eq!(staging.len(), 2 * BONES_PER_SLOT);
        for (slot, bones) in [first, second].iter().enumerate() {
            for (joint, matrix) in bones.iter().enumerate() {
                let uploaded = Mat4::from_cols_array(&staging[slot * BONES_PER_SLOT + joint]);
                assert_eq!(
                    uploaded.transform_point3(Vec3::ONE),
                    matrix.transform_point3(Vec3::ONE)
                );
            }
            assert!(
                staging[slot * BONES_PER_SLOT + bones.len()..(slot + 1) * BONES_PER_SLOT]
                    .iter()
                    .all(|m| *m == Mat4::IDENTITY.to_cols_array())
            );
        }
    }

    #[test]
    fn bone_slots_bound_empty_and_oversized_skeletons() {
        let mut staging = Vec::new();
        append_bone_slot(&mut staging, &[]);
        append_bone_slot(&mut staging, &vec![Mat4::ZERO; BONES_PER_SLOT + 1]);
        append_bone_slot(&mut staging, &[Mat4::IDENTITY]);
        assert_eq!(staging.len(), 3 * BONES_PER_SLOT);
        assert!(staging[..BONES_PER_SLOT]
            .iter()
            .all(|m| *m == Mat4::IDENTITY.to_cols_array()));
        assert!(staging[BONES_PER_SLOT..2 * BONES_PER_SLOT]
            .iter()
            .all(|m| *m == Mat4::ZERO.to_cols_array()));
        assert_eq!(staging[2 * BONES_PER_SLOT], Mat4::IDENTITY.to_cols_array());
    }

    #[test]
    fn point_range_invalid_values_are_disabled_and_directional_ignores_range() {
        for range in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let light = Light::point("范围", [0.0; 3], [1.0; 3], 1.0, range);
            let uniform = ScenePipeline::build_lights([0.0; 3], &[&light], None);
            assert_eq!(uniform.lights[0].shadow_params[1], 0.0);
        }
        let light = Light::default();
        let uniform = ScenePipeline::build_lights([0.0; 3], &[&light], Some(0));
        assert_eq!(uniform.lights[0].shadow_params, [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn regression_r09_point_range_reaches_gpu() {
        let light = Light::point("范围", [0.0; 3], [1.0; 3], 1.0, 7.0);
        let uniform = ScenePipeline::build_lights([0.0; 3], &[&light], None);
        assert_eq!(uniform.lights[0].shadow_params[1], 7.0);
    }

    fn project(m: &Mat4, p: Vec3) -> Vec3 {
        let clip = *m * p.extend(1.0);
        clip.truncate() / clip.w
    }

    // -- 材质 uniform（metallic-roughness 原样过桥）--

    /// 材质 uniform 必须**原样**携带 metallic / roughness —— PBR BRDF 直接
    /// 消费这两个值。旧 Blinn-Phong 实现把它们翻译成"高光强度 + 光泽度"
    /// （metallic=1 → params[0]=0.6），这条测试在那套实现下必然红。
    #[test]
    fn material_uniform_carries_metallic_and_roughness_untouched() {
        let p = MaterialParams {
            base_color: [0.2, 0.4, 0.6, 1.0],
            metallic: 1.0,
            roughness: 0.2,
            blend: false,
        };
        let u = MaterialUniform::from_params(&p);
        assert_eq!(u.base_color, p.base_color);
        assert!(
            (u.params[0] - 1.0).abs() < 1e-6,
            "metallic 应当原样保留，收到 {:?}",
            u.params
        );
        assert!(
            (u.params[1] - 0.2).abs() < 1e-6,
            "roughness 应当原样保留，收到 {:?}",
            u.params
        );
    }

    /// 安全夹取：metallic 夹到 [0,1]；roughness 下限不给 0 —— GGX 的 D 项
    /// 在 α→0 时发散（除以零 → NaN 传染整帧），粗糙度 0 的材质必须变成
    /// "很光滑"而不是"算崩"。
    #[test]
    fn material_uniform_clamps_metallic_and_keeps_roughness_alive() {
        let p = MaterialParams {
            metallic: 2.5,
            roughness: 0.0,
            ..Default::default()
        };
        let u = MaterialUniform::from_params(&p);
        assert!((u.params[0] - 1.0).abs() < 1e-6, "metallic 上夹 1.0");
        assert!(
            u.params[1] > 0.0,
            "粗糙度必须有正下限（GGX 除零防护），收到 {:?}",
            u.params
        );

        let p = MaterialParams {
            metallic: -1.0,
            roughness: 9.0,
            ..Default::default()
        };
        let u = MaterialUniform::from_params(&p);
        assert!(u.params[0] >= 0.0, "metallic 下夹 0.0");
        assert!(u.params[1] <= 1.0, "roughness 上夹 1.0");
    }

    // -- Bounds --

    #[test]
    fn empty_bounds_is_empty_and_union_brings_it_to_life() {
        let empty = Bounds::EMPTY;
        assert!(empty.is_empty());

        let one = Bounds::from_points([[1.0, 2.0, 3.0]]);
        assert!(!one.is_empty());
        // 单点盒：min == max。
        assert_eq!(one.min, one.max);

        let merged = empty.union(&one);
        assert_eq!(merged, one, "空盒并任何东西应该得到那个东西本身");

        let two = Bounds::from_points([[-1.0, 0.0, 0.0], [2.0, 5.0, 1.0]]);
        let all = one.union(&two);
        assert_eq!(all.min, Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(all.max, Vec3::new(2.0, 5.0, 3.0));
    }

    #[test]
    fn bounds_center_and_radius() {
        let b = Bounds {
            min: Vec3::new(-1.0, -1.0, -1.0),
            max: Vec3::new(1.0, 1.0, 1.0),
        };
        assert_eq!(b.center(), Vec3::ZERO);
        assert!((b.radius() - 3f32.sqrt()).abs() < 1e-5);
    }

    #[test]
    fn transformed_bounds_wraps_rotated_corners() {
        // 单位盒绕 Y 轴转 45°，角点会伸到 sqrt(2) 之外 —— 求盒后必须能罩住。
        let unit = Bounds {
            min: Vec3::splat(-0.5),
            max: Vec3::splat(0.5),
        };
        let rot = Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let out = unit.transformed(&rot);
        // 转角后 x/z 半宽从 0.5 变成 sqrt(2)/2 ≈ 0.7071
        let expected = 0.5f32 * 2f32.sqrt();
        assert!(
            (out.half_extent().x - expected).abs() < 1e-4,
            "got {:?}",
            out.half_extent()
        );
        assert!((out.half_extent().z - expected).abs() < 1e-4);
        // y 不受绕 Y 旋转影响
        assert!((out.half_extent().y - 0.5).abs() < 1e-5);
        assert_eq!(out.center(), Vec3::ZERO);
    }

    #[test]
    fn transformed_empty_bounds_stays_empty() {
        let out = Bounds::EMPTY.transformed(&Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)));
        assert!(out.is_empty());
    }

    // -- 阴影拟合 --

    /// 一个够用的小相机：eye 在 +Z 退开，看向原点，标准透视。
    fn probe_camera(eye: Vec3, target: Vec3) -> Mat4 {
        let view = Mat4::look_at_rh(eye, target, Vec3::Y);
        let proj = Mat4::perspective_rh(60f32.to_radians(), 16.0 / 9.0, 0.1, 200.0);
        proj * view
    }

    fn probe_scene() -> Bounds {
        Bounds {
            min: Vec3::new(-10.0, -0.5, -10.0),
            max: Vec3::new(10.0, 6.0, 10.0),
        }
    }

    #[test]
    fn cascade_splits_are_monotonic_and_end_at_the_coverage() {
        let splits = cascade_splits(0.1, 60.0, 4);
        assert_eq!(splits.len(), 4);
        for w in splits.windows(2) {
            assert!(w[1] > w[0], "切分必须严格递增：{splits:?}");
        }
        assert!(
            (splits[3] - 60.0).abs() < 1e-4,
            "最后一级必须精确落在覆盖距离上：{splits:?}"
        );
    }

    /// 对数混合的含义：**近处切得细、远处切得粗**。纯均匀切分每段等长，
    /// 这条测试在那种实现下会红（第一段 15 而不是 3 左右）。
    #[test]
    fn cascade_splits_bias_toward_the_camera() {
        let splits = cascade_splits(0.1, 64.0, 4);
        let uniform = 64.0 / 4.0;
        assert!(
            splits[0] < uniform * 0.6,
            "近处一级应当明显短于均匀切分（{:.2} vs {uniform:.2}）",
            splits[0]
        );
        // 最后一级明显长于均匀切分 —— 远处的精度按透视本来就该少给。
        assert!(splits[3] - splits[2] > uniform * 1.2, "{splits:?}");
    }

    #[test]
    fn cascade_splits_handle_degenerate_ranges() {
        // count = 0：没有级联（调用方应当关闭阴影）。
        assert!(cascade_splits(0.1, 10.0, 0).is_empty());
        // near >= far（坏相机）：仍然给出单调且有限的切分，不能 panic / NaN。
        let splits = cascade_splits(5.0, 1.0, 3);
        assert_eq!(splits.len(), 3);
        assert!(splits.iter().all(|d| d.is_finite()), "{splits:?}");
        assert!(splits.windows(2).all(|w| w[1] > w[0]), "{splits:?}");
    }

    /// **每层的子视锥必须完整落在该层的光源裁剪空间里。** 这是阴影贴图能
    /// 工作的前提：落在外面的部分没有深度可比较，画面上就是"没有影子"。
    /// 级联的经典 bug（切分区间与拟合范围对不上）会在这里立刻红。
    #[test]
    fn shadow_cascades_cover_their_own_slab() {
        let view_proj = probe_camera(Vec3::new(0.0, 6.0, 24.0), Vec3::ZERO);
        let inv = view_proj.inverse();
        let cascades = cascade_shadow_matrices(
            &inv,
            Vec3::new(0.0, 6.0, 24.0),
            Vec3::new(0.4, -1.0, 0.3),
            probe_scene(),
            2048,
            SHADOW_CASCADES,
        )
        .expect("应当拟合出级联");

        assert_eq!(cascades.count, SHADOW_CASCADES);
        let [near_q, far_q] = frustum_corners(&inv).expect("角点");
        let near_center = (near_q[0] + near_q[1] + near_q[2] + near_q[3]) / 4.0;
        let far_center = (far_q[0] + far_q[1] + far_q[2] + far_q[3]) / 4.0;
        let forward = (far_center - near_center).normalize();
        let eye = Vec3::new(0.0, 6.0, 24.0);
        let near_d = (near_center - eye).dot(forward);
        let span = (far_center - eye).dot(forward) - near_d;

        for c in 0..cascades.count {
            let d0 = if c == 0 {
                near_d
            } else {
                cascades.splits[c - 1]
            };
            let d1 = cascades.splits[c];
            let (t0, t1) = ((d0 - near_d) / span, (d1 - near_d) / span);
            for k in 0..4 {
                for t in [t0, t1] {
                    let corner = near_q[k].lerp(far_q[k], t);
                    let ndc = project(&cascades.view_proj[c], corner);
                    assert!(
                        ndc.x.abs() <= 1.001 && ndc.y.abs() <= 1.001,
                        "第 {c} 层的子视锥角点跑去画面外：ndc = {ndc:?}"
                    );
                    assert!(
                        (-0.001..=1.001).contains(&ndc.z),
                        "第 {c} 层的子视锥角点深度越界：ndc.z = {}",
                        ndc.z
                    );
                }
            }
        }
    }

    /// CSM 的**全部意义**：同一张 2048² 深度图，近处的纹素必须比"整场景一个
    /// 视锥"细一个量级。这条测试就是那笔账 —— 旧实现（单级拟合）下第一级
    /// 纹素约 2.9 cm，四级的第 0 层不到 1 cm。
    #[test]
    fn shadow_cascades_sharpen_the_near_field() {
        let eye = Vec3::new(0.0, 6.0, 24.0);
        let bounds = probe_scene();
        let view_proj = probe_camera(eye, Vec3::ZERO);
        let cascades = cascade_shadow_matrices(
            &view_proj.inverse(),
            eye,
            Vec3::new(0.4, -1.0, 0.3),
            bounds,
            2048,
            SHADOW_CASCADES,
        )
        .expect("应当拟合出级联");

        // 单级拟合的纹素：直径 / 分辨率（与 `fit_directional_shadow` 的口径一致）。
        let single_texel = 2.0 * bounds.half_extent().length() * 1.02 / 2048.0;
        assert!(
            cascades.texel_world[0] < single_texel * 0.5,
            "第 0 层纹素 {:.4} m 应当明显细于单级 {:.4} m",
            cascades.texel_world[0],
            single_texel
        );
        // 远处的层反而更粗（覆盖更宽），但必须仍优于"整场景一个视锥"的量级
        // 或至少在同一数量级 —— 反了说明切分与拟合接错了。
        assert!(cascades.texel_world[0] < cascades.texel_world[3]);
        assert!(cascades.texel_world.iter().all(|t| *t > 0.0));
    }

    /// 同输入两次拟合必须逐位相同：无头截图"同机逐字节一致"的地基。
    #[test]
    fn shadow_cascades_are_deterministic() {
        let eye = Vec3::new(1.0, 7.0, 20.0);
        let inv = probe_camera(eye, Vec3::new(0.0, 0.0, 0.0)).inverse();
        let a =
            cascade_shadow_matrices(&inv, eye, Vec3::new(0.2, -1.0, 0.1), probe_scene(), 2048, 4)
                .expect("a");
        let b =
            cascade_shadow_matrices(&inv, eye, Vec3::new(0.2, -1.0, 0.1), probe_scene(), 2048, 4)
                .expect("b");
        assert_eq!(
            a.view_proj[0].to_cols_array(),
            b.view_proj[0].to_cols_array()
        );
        assert_eq!(a.splits, b.splits);
        assert_eq!(a.texel_world, b.texel_world);
    }

    /// 退化输入必须给 `None`（调用方据此关阴影），而不是 NaN 矩阵。
    #[test]
    fn shadow_cascades_reject_degenerate_input() {
        // 空场景 / 非法层数。
        let inv = probe_camera(Vec3::new(0.0, 5.0, 10.0), Vec3::ZERO).inverse();
        assert!(cascade_shadow_matrices(
            &inv,
            Vec3::new(0.0, 5.0, 10.0),
            Vec3::new(0.0, -1.0, 0.0),
            Bounds::EMPTY,
            2048,
            4
        )
        .is_none());
        assert!(cascade_shadow_matrices(
            &inv,
            Vec3::new(0.0, 5.0, 10.0),
            Vec3::new(0.0, -1.0, 0.0),
            probe_scene(),
            2048,
            0
        )
        .is_none());
        // 逆矩阵退化成零：角点全是原点，切不出来。
        assert!(cascade_shadow_matrices(
            &Mat4::ZERO,
            Vec3::ZERO,
            Vec3::new(0.0, -1.0, 0.0),
            probe_scene(),
            2048,
            4
        )
        .is_none());
    }

    /// 零方向（数据坏了）也要拟合出有限矩阵 —— 与单级拟合同一条容错约定。
    #[test]
    fn shadow_cascades_survive_a_zero_direction() {
        let eye = Vec3::new(0.0, 5.0, 10.0);
        let cascades = cascade_shadow_matrices(
            &probe_camera(eye, Vec3::ZERO).inverse(),
            eye,
            Vec3::ZERO,
            probe_scene(),
            2048,
            2,
        )
        .expect("零方向也要能拟合");
        assert!(cascades.view_proj[0]
            .to_cols_array()
            .iter()
            .all(|v| v.is_finite()));
    }

    /// 相机远平面比场景大得多时，覆盖距离必须按**场景**收紧（不是按远平面）：
    /// 40×40 的地面不该被切到 200 单位的远平面上，否则每层都糊。
    #[test]
    fn shadow_coverage_follows_the_scene_not_the_far_plane() {
        let eye = Vec3::new(0.0, 6.0, 24.0);
        let bounds = probe_scene();
        let cascades = cascade_shadow_matrices(
            &probe_camera(eye, Vec3::ZERO).inverse(),
            eye,
            Vec3::new(0.4, -1.0, 0.3),
            bounds,
            2048,
            SHADOW_CASCADES,
        )
        .expect("应当拟合出级联");
        let cover = cascades.splits[SHADOW_CASCADES - 1];
        // 场景最远角点在视线上的投影 ≈ 30 上下（eye 24 处看向原点，远角 10）。
        assert!(
            cover < 60.0,
            "覆盖距离应当贴着场景（{cover:.2}），而不是相机远平面 200"
        );
        // 同时必须罩住整个场景：最远角点不能被切掉。
        let mut scene_far = f32::NEG_INFINITY;
        for corner in bounds.corners() {
            scene_far = scene_far.max((corner - eye).dot(cascades.forward));
        }
        assert!(
            cover + 1e-3 >= scene_far.min(MAX_SHADOW_DISTANCE),
            "覆盖距离 {cover:.2} 没罩住场景（最远 {scene_far:.2}）"
        );
    }

    #[test]
    fn shadow_fit_of_empty_scene_is_none() {
        assert!(fit_directional_shadow(Vec3::new(0.0, -1.0, 0.0), Bounds::EMPTY).is_none());
    }

    /// uniform 的字节布局必须与 WGSL 的 `ShadowUniform` 逐字段对上。
    ///
    /// 4×mat4（256）+ 3×vec4（48）= 304。这不是"差不多"的事：偏移错一格，
    /// 片元就会用另一层（或另一盏灯）的矩阵去投影，表现是"影子在奇怪的位置"
    /// 而不是报错 —— 只能靠测试钉住。
    #[test]
    fn shadow_uniform_matches_wgsl_layout() {
        assert_eq!(std::mem::size_of::<ShadowUniform>(), 304);
        // uniform 的结构体尺寸必须是 16 的倍数（WGSL 的地址空间对齐要求）。
        assert_eq!(std::mem::size_of::<ShadowUniform>() % 16, 0);
        // 阴影 pass 的动态偏移步长必须能容纳一个矩阵，且是 256 的倍数。
        assert!((SHADOW_PASS_STRIDE as usize) >= 64);
        assert_eq!(SHADOW_PASS_STRIDE % 256, 0);
        // 主 pass 与阴影 pass 读的是同一批矩阵：层数上限必须一致。
        assert_eq!(SHADOW_CASCADES, MAX_SHADOW_CASCADES);
    }

    /// 场景包围盒里的任何一点，投影到光源裁剪空间后都必须落在
    /// NDC 的 [-1,1]×[-1,1] 与深度 [0,1] 里。**这是阴影贴图能工作的前提**：
    /// 一旦有点落在外面，那部分是"没有深度可比较"的，画面上就是没有阴影。
    #[test]
    fn shadow_fit_covers_the_whole_scene() {
        let bounds = Bounds {
            min: Vec3::new(-12.0, -1.0, -12.0),
            max: Vec3::new(12.0, 9.0, 12.0),
        };
        for dir in [
            Vec3::new(0.45, -1.0, 0.35),
            Vec3::new(0.0, -1.0, 0.0), // 正上方（会走 up=Z 那条分支）
            Vec3::new(1.0, -0.2, 0.0),
        ] {
            let m = fit_directional_shadow(dir, bounds).expect("should fit");
            for corner in bounds.corners() {
                let ndc = project(&m, corner);
                assert!(
                    ndc.x >= -1.0001 && ndc.x <= 1.0001,
                    "dir {dir:?} corner {corner:?} ndc.x = {}",
                    ndc.x
                );
                assert!(
                    ndc.y >= -1.0001 && ndc.y <= 1.0001,
                    "dir {dir:?} corner {corner:?} ndc.y = {}",
                    ndc.y
                );
                assert!(
                    ndc.z >= -0.0001 && ndc.z <= 1.0001,
                    "dir {dir:?} corner {corner:?} ndc.z = {}",
                    ndc.z
                );
            }
        }
    }

    /// 深度顺序必须与"离光源的远近"一致：更靠近光源的点（逆向光方向）深度更小。
    /// 反了的话比较结果整个反相 —— 该亮的暗、该暗的亮。
    #[test]
    fn shadow_fit_orders_depth_by_distance_to_light() {
        let bounds = Bounds {
            min: Vec3::new(-5.0, -5.0, -5.0),
            max: Vec3::new(5.0, 5.0, 5.0),
        };
        let dir = Vec3::new(0.0, -1.0, 0.0); // 光自上而下
        let m = fit_directional_shadow(dir, bounds).unwrap();

        // 上游（更靠近灯）= +y，下游 = -y。
        let near = project(&m, Vec3::new(0.0, 4.0, 0.0)).z;
        let far = project(&m, Vec3::new(0.0, -4.0, 0.0)).z;
        assert!(
            near < far,
            "靠近光源的点深度必须更小：near={near} far={far}"
        );
    }

    #[test]
    fn shadow_fit_handles_zero_direction() {
        let bounds = Bounds {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        // 零向量不该 panic，也不该产生 NaN。
        let m = fit_directional_shadow(Vec3::ZERO, bounds).expect("degenerate dir still fits");
        let ndc = project(&m, Vec3::ZERO);
        assert!(ndc.is_finite(), "零方向拟合出了 NaN：{ndc:?}");
    }

    /// 视锥半径必须**封顶**：场景里只要有一个物体飞远（自由落体掉出视野、被脚本
    /// 扔出去），整场景的包围盒就会被它拉大，深度范围随之拉长 —— 阴影贴图分辨率
    /// 固定，精度会当场崩掉。夹住之后，超出范围的东西落在视锥外（不投影），
    /// 而剩下的部分精度保得住。
    #[test]
    fn shadow_fit_bounds_the_frustum_for_runaway_scenes() {
        // 一个"点了火"的场景：物体飞到了 x = 10_000。
        let runaway = Bounds {
            min: Vec3::new(-1.0, -1.0, -1.0),
            max: Vec3::new(10_000.0, 1.0, 1.0),
        };
        let m = fit_directional_shadow(Vec3::new(0.0, -1.0, 0.0), runaway).unwrap();

        // 视锥被夹到 MAX_SHADOW_DISTANCE ⇒ "场景中心 + 3 倍上限"必然落在视锥外。
        let outside = runaway.center() + Vec3::new(MAX_SHADOW_DISTANCE * 3.0, 0.0, 0.0);
        let ndc = project(&m, outside);
        assert!(
            ndc.x.abs() > 1.0 || ndc.y.abs() > 1.0,
            "视锥没有被夹住（飞出的物体把半径撑大了）：ndc = {ndc:?}"
        );
    }

    // -- 阴影光源挑选 --

    #[test]
    fn pick_shadow_slot_chooses_first_shadow_casting_directional() {
        let point = Light::point("lamp", [0.0, 3.0, 0.0], [1.0; 3], 1.0, 10.0);
        let mut sun = Light::directional("sun", [0.5, -1.0, 0.5], [1.0; 3], 1.0);
        sun.casts_shadow = true;
        let mut sun2 = Light::directional("sun2", [0.0, -1.0, 0.0], [1.0; 3], 1.0);
        sun2.casts_shadow = true;

        let packed: Vec<&Light> = vec![&point, &sun, &sun2];
        // 第一节是点光 —— 不支持，跳过；选中的是第二盏（下标 1）。
        assert_eq!(pick_shadow_slot(&packed), Some(1));
    }

    #[test]
    fn pick_shadow_slot_ignores_lights_that_opt_out() {
        let mut off = Light::directional("off", [0.0, -1.0, 0.0], [1.0; 3], 1.0);
        off.casts_shadow = false;
        let packed: Vec<&Light> = vec![&off];
        assert_eq!(pick_shadow_slot(&packed), None);
    }

    /// 打包后的下标必须与 `build_lights` 写进去的位置一致 —— 否则会把
    /// 阴影标记贴到错误的那盏灯上（画面表现为"某个无关的灯在投影"）。
    #[test]
    fn build_lights_marks_the_shadow_slot() {
        let mut sun = Light::directional("sun", [0.0, -1.0, 0.0], [1.0; 3], 1.0);
        sun.casts_shadow = true;
        let lamp = Light::point("lamp", [0.0, 3.0, 0.0], [1.0; 3], 1.0, 10.0);

        let packed: Vec<&Light> = vec![&lamp, &sun];
        let slot = pick_shadow_slot(&packed);
        assert_eq!(slot, Some(1));

        let u = ScenePipeline::build_lights([0.0; 3], &packed, slot);
        assert_eq!(u.ambient[3], 2.0, "两盏灯都应被写入");
        assert_eq!(u.lights[1].shadow_params[0], 1.0, "方向光应被标为投射阴影");
        assert_eq!(u.lights[0].shadow_params[0], 0.0, "点光不该被标");

        // 没有任何灯投射阴影时，标记必须全是 0（片元着色器据此跳过采样）。
        let u_none = ScenePipeline::build_lights([0.0; 3], &packed, None);
        assert_eq!(u_none.lights[0].shadow_params[0], 0.0);
        assert_eq!(u_none.lights[1].shadow_params[0], 0.0);
    }

    // -- 视锥剔除 --

    /// 蒙皮网格必须拿到 50% 余量，静态网格必须**一点不多**。
    ///
    /// 这条测试守的是"动画肢体甩出绑定盒"的修复：如果哪天有人把
    /// `mesh.skins.is_empty()` 判反、或把余量乘到错误的分支上，这里立刻红。
    #[test]
    fn cull_bounds_pads_skinned_meshes_only() {
        let bounds = Bounds {
            min: Vec3::new(-1.0, -1.0, -1.0),
            max: Vec3::new(1.0, 1.0, 1.0),
        };
        let mesh = GltfMesh {
            primitives: Vec::new(),
            vertex_size: 0,
            bounds,
            nodes: Vec::new(),
            skins: vec![crate::animation::Skin {
                joints: vec![0],
                inverse_bind: vec![Mat4::IDENTITY],
            }],
            animations: Vec::new(),
        };
        let world = Mat4::IDENTITY;

        let padded = cull_bounds(&mesh, &world, true);
        assert_eq!(
            padded.half_extent(),
            Vec3::splat(1.5),
            "蒙皮余量是 50%（1.5× 半长），不是'把常数抄进断言'"
        );
        assert_eq!(padded.center(), Vec3::ZERO);

        // 没有动画器（或动画器还没 tick）时不该平白多画 3× 体积。
        assert_eq!(cull_bounds(&mesh, &world, false), bounds);
    }

    /// 非蒙皮网格即便有动画器（节点动画 / 内置图元的平移旋转）也不加余量：
    /// 静态几何的世界包围盒本来就是精确的。
    #[test]
    fn cull_bounds_leaves_static_meshes_exact() {
        let bounds = Bounds {
            min: Vec3::new(-2.0, 0.0, -0.5),
            max: Vec3::new(2.0, 1.0, 0.5),
        };
        let mesh = GltfMesh {
            primitives: Vec::new(),
            vertex_size: 0,
            bounds,
            nodes: Vec::new(),
            skins: Vec::new(),
            animations: Vec::new(),
        };
        let world = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(
            cull_bounds(&mesh, &world, true),
            bounds.transformed_affine(&world)
        );
    }
}
