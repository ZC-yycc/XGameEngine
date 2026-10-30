//! 引擎配置类型。
//!
//! 提供窗口设置、引擎顶层选项，以及资源路径配置。
//! 所有配置类型均支持 `Serialize` / `Deserialize`，可通过 RON 文件持久化。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 窗口配置
// ---------------------------------------------------------------------------

/// 窗口创建设置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    /// 窗口标题。
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// 是否允许用户调整窗口大小。
    pub resizable: bool,
    /// 是否启用垂直同步。
    pub vsync: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "XGameEngine".into(),
            width: 1280,
            height: 720,
            resizable: true,
            vsync: true,
        }
    }
}

// ---------------------------------------------------------------------------
// 构建 / 发布设置
// ---------------------------------------------------------------------------

/// 发布设置（`project.ron` 的 `build` 节）。
///
/// # 为什么场景清单必须**有序**
///
/// 有了运行期 `load_scene` 之后，"下一关"还需要知道自己是第几关 —— 让作者在每张
/// 关卡脚本里硬编码下一关的路径，加一关就要改两处，删一关直接断链。清单顺序就是
/// **关卡索引**：脚本 `scene_index()` 取当前序号、`load_scene_at(i + 1)` 进下一关，
/// 关卡编排变成数据。
///
/// # 为什么空清单是合法的
///
/// 老项目（以及单场景 demo）没有这一节。空清单的语义是"未配置"：有效顺序退化成
/// 只有入口场景 —— 与没有这个功能时的行为完全一致，所以不用强制升级项目文件。
///
/// **入口场景不必是清单第一项**（比如入口是"启动器"场景，它自己不参与关卡轮转），
/// 但清单里找不到当前场景时脚本的 `scene_index()` 会返回 `nil`，这点在
/// `Docs/脚本API.md` 写清了。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    /// 发布版本号，纯展示（写进启动日志 / 构建报告），不参与任何逻辑判断。
    #[serde(default = "default_build_version")]
    pub version: String,
    /// 关卡顺序，**项目相对路径**。顺序即关卡索引。
    #[serde(default)]
    pub scenes: Vec<String>,
}

fn default_build_version() -> String {
    "0.1.0".into()
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            version: default_build_version(),
            scenes: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 引擎顶层配置
// ---------------------------------------------------------------------------

/// 引擎顶层配置。
///
/// `Default` 直接派生：每个字段的默认值就是它自己 `Default::default()` 的结果，
/// 手写一份只是把同样的东西再抄一遍，抄漏一个字段还会静默变成"和别处不一样"。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineConfig {
    pub window: WindowConfig,
    /// 目标帧率。0 = 不限制。
    pub target_fps: u32,
    #[serde(default)]
    pub content: ContentConfig,
}

// ---------------------------------------------------------------------------
// 内容/资源路径配置
// ---------------------------------------------------------------------------

/// 内容资源路径配置。
///
/// 控制引擎搜索资产文件的目录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentConfig {
    /// 场景文件目录。
    pub scenes_dir: String,
    /// 模型/网格资产目录。
    pub models_dir: String,
    /// 纹理资产目录。
    pub textures_dir: String,
    /// 音频资产目录。
    pub audio_dir: String,
    /// 着色器资产目录。
    #[serde(default = "default_shaders_dir")]
    pub shaders_dir: String,
    /// 预制体资产目录。
    ///
    /// 带 `serde(default)`：这个字段是后加的，老项目的 `project.ron` 里没有它，
    /// 反序列化时补默认值即可，不必强制升级清单文件。
    #[serde(default = "default_prefabs_dir")]
    pub prefabs_dir: String,
    /// 脚本目录（`.lua` 文件）。
    ///
    /// 同样带 serde 默认值：脚本是后加的能力，老清单里没有这一项。
    /// 运行时也**只**用默认值兜底，不会因为目录不存在就报错 ——
    /// 一个还没写脚本的项目完全合法。
    #[serde(default = "default_scripts_dir")]
    pub scripts_dir: String,
    /// 附加搜索路径列表（有序，项目相对目录）。
    ///
    /// **语义**（2026-10-29 定义并落地，见 [`crate::project::resolve_read_path`]）：
    /// 运行时按项目相对路径读文件（脚本 / 音频）时，先试 `根/<相对路径>`，
    /// 再依次试 `根/<条目>/<相对路径>`。条目本身也过
    /// [`crate::project::normalize_project_relative`]，不合规的条目在运行时被
    /// **跳过**、由 `--check` 报成问题 —— 它绝不会让读取探到项目外。
    ///
    /// 只影响**读**；写盘永远落在清单声明的位置。空列表 = 与引入该字段之前
    /// 行为逐位一致（老工程全是空列表）。
    ///
    /// 典型用途：把共享脚本放 `Mods/Shared/Scripts`，让 `Scripts/` 与它一起被搜；
    /// 或者团队把公共资产放在独立目录里复用。
    pub search_paths: Vec<String>,
}

fn default_shaders_dir() -> String {
    "Shaders".into()
}

fn default_prefabs_dir() -> String {
    "Prefabs".into()
}

fn default_scripts_dir() -> String {
    "Scripts".into()
}

impl Default for ContentConfig {
    fn default() -> Self {
        Self {
            scenes_dir: "Scenes".into(),
            models_dir: "Resources".into(),
            textures_dir: "Resources".into(),
            audio_dir: "Resources".into(),
            shaders_dir: default_shaders_dir(),
            prefabs_dir: default_prefabs_dir(),
            scripts_dir: default_scripts_dir(),
            search_paths: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 运行时参数
// ---------------------------------------------------------------------------

/// 玩法层可调参数（重力 / 固定步长），持久化在 `project.ron` 的 `runtime` 节。
///
/// 这两个值此前硬编码在 `xgame-runtime::WorldConfig` 的默认值里 —— 想调重力
/// 只能改引擎源码重编译。现在进清单、由编辑器「⚙ Project Settings」面板编辑；
/// `Project` 上的 `#[serde(default)]` 保证旧清单照常加载。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// 世界重力（m/s²，Y 向上、向下为负）。
    pub gravity: [f32; 3],
    /// 物理 / 固定步脚本的步长（秒）。
    pub fixed_dt: f32,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            gravity: [0.0, -9.81, 0.0],
            fixed_dt: 1.0 / 60.0,
        }
    }
}

impl RuntimeConfig {
    /// 容错读取：清单是**手改的文本**，非法值不能直接交给物理引擎。
    ///
    /// - 重力含 NaN / 无穷 / 超大值（>1e6，一帧能把物体弹出银河系）→ 退回默认；
    /// - `fixed_dt <= 0` 会让固定步长累加器永远凑不满一步（时间静止），非有限
    ///   同理 → 退回默认；
    /// - 合法但离谱的步长夹进 [1/240s, 1/10s]：比 240Hz 更快没有意义，
    ///   比 10Hz 更慢肉眼可见地卡。
    pub fn sanitized(self) -> Self {
        let sane = |v: [f32; 3]| v.iter().all(|x| x.is_finite() && x.abs() <= 1e6);
        let gravity = if sane(self.gravity) {
            self.gravity
        } else {
            Self::default().gravity
        };
        let fixed_dt = if self.fixed_dt.is_finite() && self.fixed_dt > 0.0 {
            self.fixed_dt.clamp(1.0 / 240.0, 1.0 / 10.0)
        } else {
            Self::default().fixed_dt
        };
        Self { gravity, fixed_dt }
    }
}

// ---------------------------------------------------------------------------
// 画面渲染参数
// ---------------------------------------------------------------------------

/// 色调映射算子。
///
/// 枚举而不是 bool（"开 / 关"）：色调映射是一整类算子的位置 —— Reinhard /
/// Filmic / Neutral 随时可以加进来，而 bool 到时候还要改磁盘格式。当前只有
/// 两个变体，`Aces` 是默认。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToneMapMode {
    /// 电影级 ACES 拟合曲线（RRT + ODT 的有理拟合）：高光滚降不裁剪，
    /// 颜色先转到 ACEScg（AP1）再回来，过曝处保留色相。默认。
    #[default]
    Aces,
    /// 不做色调映射：只有曝光与钳制 —— 高光直接裁到纯白。
    ///
    /// 保留它是刻意的：它是 ACES 上线前的旧画面（对照基线），也是
    /// "我要 flat 画面"的逃生舱；`scene_signature` 与报告都认这个开关。
    Off,
}

/// Bloom（泛光）参数：亮部溢出的柔光。
///
/// 三个字段就是"经典三件套"：**提取**（threshold / knee）控制"多亮才算亮"，
/// **强度**（intensity）控制光晕比例。`intensity = 0` 即关闭（不设独立开关：
/// 两个字段表达同一个状态迟早互相矛盾），关闭时渲染器**整段跳过** ——
/// 不建纹理、不跑 pass，是真正的零成本。
///
/// 为什么默认关：Bloom 是**风格**参数（写实档也常关），不是"正确性"参数 ——
/// 与色调映射（默认 ACES，因为它回答"高光怎么落到显示区间"这个必须回答的
/// 问题）不同。要发光的项目把 intensity 调到 0.4~0.8 即可。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BloomConfig {
    /// 光晕强度（叠加比例，0 = 关闭）。
    pub intensity: f32,
    /// 亮部提取阈值（曝光后的线性亮度；1.0 ≈ 显示白点）。
    pub threshold: f32,
    /// 软膝半宽：阈值附近多宽的一段"淡入"，避免亮斑边缘出现硬边。
    pub knee: f32,
}

impl Default for BloomConfig {
    fn default() -> Self {
        Self {
            intensity: 0.0,
            threshold: 1.0,
            knee: 0.6,
        }
    }
}

impl BloomConfig {
    /// 光晕强度的上限：再亮只是整幅糊成一片白，没有可用的画面。
    pub const MAX_INTENSITY: f32 = 10.0;
    /// 阈值上限：场景亮度标定是"白面 ≈ 1.0"，100 已远超任何真实内容。
    pub const MAX_THRESHOLD: f32 = 100.0;
    /// 软膝半宽上限（线性亮度单位）。
    pub const MAX_KNEE: f32 = 4.0;

    /// 与默认值完全一致时不落盘（`skip_serializing_if` 用）：默认清单里不出现
    /// 这一节，与"没有调过画面的项目"保持逐字节一致的 diff 噪声水平。
    /// 注意判据是**整体相等**而不是"intensity == 0"：关了光晕但调过阈值的
    /// 配置照样落盘 —— 用户调过的参数不悄悄丢。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 容错读取（同 [`RenderConfig::sanitized`] 的契约）：
    /// NaN / 无穷 / 负数落回默认值，合法但离谱的量级夹进安全范围。
    pub fn sanitized(self) -> Self {
        let intensity = if self.intensity.is_finite() {
            self.intensity.clamp(0.0, Self::MAX_INTENSITY)
        } else {
            Self::default().intensity
        };
        let threshold = if self.threshold.is_finite() {
            self.threshold.clamp(0.0, Self::MAX_THRESHOLD)
        } else {
            Self::default().threshold
        };
        let knee = if self.knee.is_finite() {
            self.knee.clamp(0.0, Self::MAX_KNEE)
        } else {
            Self::default().knee
        };
        Self {
            intensity,
            threshold,
            knee,
        }
    }
}

/// FXAA 抗锯齿参数（后处理级别的边缘平滑）。
///
/// **默认关**：引擎的场景 pass 已经走 MSAA（默认取适配器支持的最高档，常见
/// 4×），几何边缘本身就有硬件抗锯齿；FXAA 是给"MSAA 覆盖不到的走样"（细长
/// 几何、着色器高频细节、低采样档位）补的一层。默认开着会改变所有旧场景的
/// 画面，与 Bloom / 天空同一条纪律：风格 / 画质增强默认不动画面。
///
/// 目前只有开关，没有暴露阈值与强度：FXAA 3.11 的内部参数（subpix 0.75、
/// edge threshold 0.166 / 0.0833）是经过大量内容验证过的一组值，先不把
/// 调参负担转给用户。确需可调时再加字段，默认值保持不变。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct FxaaConfig {
    /// 是否启用 FXAA。
    pub enabled: bool,
}

impl FxaaConfig {
    /// 与默认值完全一致时不落盘（`skip_serializing_if` 用）：没开过的项目
    /// `project.ron` 里不出现这一节。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// TAA（时域抗锯齿）参数。
///
/// **默认关**：与 Bloom / 天空 / FXAA 同一条纪律 —— 画质增强默认不动画面。
/// 关着时渲染器**整段跳过**（不建历史纹理、不记 pass、相机投影不加抖动），
/// 画面与 TAA 上线前逐位一致。
///
/// 开启后每帧把相机投影按亚像素（Halton(2,3) 序列）抖动，再把当前帧与
/// **重投影后的历史帧**混合 —— 静态画面因此收敛到"每个像素平均采样了 N 个子
/// 像素位置"的效果，边缘比任何单帧算法都更干净，且不需要更高的分辨率。
///
/// 代价写清楚（这些不是实现缺陷，是时域方法本身的取舍）：
/// - **运动物体会轻微拖影**：v1 没有逐物体的运动向量，重投影只跟相机走，
///   靠 3×3 邻域钳制把"这一帧附近根本没有的颜色"挡在外面（钳制越紧越不拖影、
///   但越容易把有用的历史也丢掉）。
/// - **快速甩镜头会有一段模糊**：历史对不上时不累计，画面在几帧内由单帧接管。
/// - **与 FXAA 互斥**：两者叠加只是把边缘糊两遍，开着 TAA 时 FXAA 自动让位
///   （见 `xgame_render::postfx` 的 `fxaa_on`）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TaaConfig {
    /// 是否启用 TAA。
    pub enabled: bool,
    /// 历史帧权重（0 = 完全不用历史，1 = 永不接受新像素 —— 画面会冻住）。
    ///
    /// 越大越稳、越干净，残影也越久；0.9 是"静止时 10 帧收敛、移动时几帧内
    /// 丢掉陈旧历史"的常用折中。
    pub history_weight: f32,
}

impl Default for TaaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            history_weight: 0.9,
        }
    }
}

impl TaaConfig {
    /// 历史权重的安全上限。
    ///
    /// 不给到 1.0：权重 1 = 新颜色一个都不进画面，等于把画面冻在第一帧，
    /// 而且**没有任何下游能报错**（表现为"引擎卡住了"，实际是配置写错）。
    pub const MAX_HISTORY_WEIGHT: f32 = 0.98;

    /// 与默认值完全一致时不落盘（`skip_serializing_if` 用）：没开过的项目
    /// `project.ron` 里不出现这一节。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 容错读取：历史权重非法（NaN / 无穷 / 越界）退回默认值。
    pub fn sanitized(self) -> Self {
        let weight = if self.history_weight.is_finite() {
            self.history_weight.clamp(0.0, Self::MAX_HISTORY_WEIGHT)
        } else {
            Self::default().history_weight
        };
        Self {
            enabled: self.enabled,
            history_weight: weight,
        }
    }
}

/// SSAO（屏幕空间环境光遮蔽）参数。
///
/// **默认关**：与 Bloom / 天空 / FXAA / TAA 同一条纪律 —— 画质增强默认不动
/// 画面。关着时渲染器**整段跳过**（不建 AO 纹理、不跑 pass），画面与 SSAO
/// 上线前逐位一致。
///
/// 半分辨率深度 SSAO：只吃深度缓冲（不需要 G-buffer 法线），法线由深度
/// 梯度重建；AO 图在半分辨率上算完再模糊、上采样乘回场景色。噪声是
/// **确定性的**（像素坐标整数哈希驱动 kernel 旋转，没有随机相位）——
/// 同机同输入逐字节一致，`--screenshot` 的可复现契约在 SSAO 开着时也成立。
///
/// 代价与取舍写清楚：
/// - 法线来自深度梯度，**平滑曲面上会有法线噪声**（半分辨率 + 模糊压掉了
///   大部分）；有 G-buffer 之前这是无解的，不是调参能修的。
/// - 遮蔽是**屏幕空间**的：物体移出屏幕的遮挡贡献当场消失（边缘处 AO 会
///   变浅）。业界同款限制。
/// - AO 直接乘在最终颜色上（含直接光）：比"只乘环境光"保守，但在没有
///   光照分解的前向管线下这是唯一正确的接法。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SsaoConfig {
    /// 是否启用 SSAO。
    pub enabled: bool,
    /// 遮蔽强度（0 = 无遮蔽效果；2.0 是"看得出但不脏"的常用上限）。
    pub strength: f32,
    /// 采样半径（世界单位）。太大 = 远处物体也遮蔽近处；太小 = 看不出。
    pub radius: f32,
    /// 深度比较偏置（世界单位）：挡掉"表面遮蔽自己"的 acne。
    pub bias: f32,
}

impl Default for SsaoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: 1.0,
            radius: 0.5,
            bias: 0.025,
        }
    }
}

impl SsaoConfig {
    /// 遮蔽强度上限：再大只是把接触阴影压成死黑，画面信息全丢。
    pub const MAX_STRENGTH: f32 = 4.0;
    /// 采样半径上限（世界单位）：10 米的遮蔽半径已经不是"接触阴影"了。
    pub const MAX_RADIUS: f32 = 10.0;
    /// 偏置上限：偏置比半径还大时遮蔽整体失效，但那也是合法的"关"法。
    pub const MAX_BIAS: f32 = 1.0;

    /// 与默认值完全一致时不落盘（`skip_serializing_if` 用）：没开过的项目
    /// `project.ron` 里不出现这一节。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 容错读取：NaN / 无穷落回默认值，合法但离谱的量级夹进安全范围。
    pub fn sanitized(self) -> Self {
        let strength = if self.strength.is_finite() {
            self.strength.clamp(0.0, Self::MAX_STRENGTH)
        } else {
            Self::default().strength
        };
        let radius = if self.radius.is_finite() {
            self.radius.clamp(0.0, Self::MAX_RADIUS)
        } else {
            Self::default().radius
        };
        let bias = if self.bias.is_finite() {
            self.bias.clamp(0.0, Self::MAX_BIAS)
        } else {
            Self::default().bias
        };
        Self {
            enabled: self.enabled,
            strength,
            radius,
            bias,
        }
    }
}

/// 程序化天空盒与环境光照（IBL）参数。
///
/// 天空是一个**解析函数**：天顶 / 地平线 / 地面三段颜色渐变 + 太阳盘 + 太阳
/// 光晕，直接对着方向求值。不做 Preetham/Hosek 那类物理大气散射 —— 前者是
/// "长什么样"的参数化，后者是"为什么长这样"的模型；先把前者做扎实（同样的
/// 出口结构以后能原样换成物理模型）。
///
/// 天空同时是**两件事的输入**，这是它比"一张背景图"重要的地方：
/// ① 背景像素画什么；② 环境照明（IBL）—— 漫反射用球谐（L2，CPU 侧投影）、
/// 镜面用 GGX 重要性采样的预过滤立方体贴图（GPU 侧一次生成）。两者从同一
/// 个解析函数现取 ⇒ "背景是蓝天、物体却按另一套环境光照明"这种分叉从根上
/// 不存在。
///
/// **默认关（`enabled = false`）**：与 [`BloomConfig`] 同一条纪律 —— 天空是
/// 风格参数，默认改变画面不可接受。关着时渲染器**整段跳过**（不建环境贴图、
/// 不采样三张环境纹理），画面与天空上线前逐位一致；`ambient` 照旧是环境光
/// 的入口。开着时环境光 = `ambient` + IBL 两者**相加**：想只要 IBL 就把
/// `ambient` 清零，想保留旧观感就留着它。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SkyConfig {
    /// 是否启用天空盒与环境照明。
    pub enabled: bool,
    /// 整体强度乘子（0 = 天空全黑 —— 那时 IBL 也全黑，等价于只有 ambient）。
    pub intensity: f32,
    /// 天顶色（线性 RGB）。
    pub zenith: [f32; 3],
    /// 地平线色。天空段与地面段在视线水平处**都**取它 —— 渐变因此在
    /// `dir.y == 0` 处连续，不需要额外处理"缝合线"。
    pub horizon: [f32; 3],
    /// 地面色（镜头朝下看时的颜色：体感上是"脚下的大地"）。
    pub ground: [f32; 3],
    /// 太阳盘半径（度）。真实太阳约 0.53°，默认调到能看清的量级。
    pub sun_size: f32,
    /// 太阳盘与光晕的强度乘子。太阳的位置与颜色来自场景里**第一盏
    /// active 的方向光**（颜色取光色）；场景没有方向光时太阳整段不画
    /// （这个乘子也不生效——没有灯就没有太阳）。
    pub sun_intensity: f32,
}

impl Default for SkyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 1.0,
            zenith: [0.12, 0.28, 0.58],
            horizon: [0.58, 0.68, 0.82],
            ground: [0.16, 0.15, 0.14],
            sun_size: 1.5,
            sun_intensity: 1.0,
        }
    }
}

impl SkyConfig {
    /// 整体强度上限：再亮整幅背景糊成纯白，没有可用画面。
    pub const MAX_INTENSITY: f32 = 10.0;
    /// 太阳强度上限（同上）。
    pub const MAX_SUN_INTENSITY: f32 = 10.0;
    /// 太阳盘半径范围（度）：下限保证看得见，上限避免"半个天空都是太阳"。
    pub const MIN_SUN_SIZE: f32 = 0.1;
    pub const MAX_SUN_SIZE: f32 = 20.0;
    /// 颜色分量上限：线性值允许超过 1（HDR 天空），但不能失控到 NaN 邻域
    /// 的不可用亮度。
    pub const MAX_COLOR: f32 = 50.0;

    /// 与默认值完全一致时不落盘（`skip_serializing_if` 用）：没开过天空的
    /// 项目 diff 里不出现这一节。判据是**整体相等**而不是 `!enabled` ——
    /// 关着但调过颜色的配置照样落盘（用户调过的参数不悄悄丢）。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 容错读取（与 [`RenderConfig::sanitized`] 同一契约）：
    /// NaN / 无穷进不了渲染器，合法但离谱的量级夹进安全范围。
    pub fn sanitized(self) -> Self {
        let color = |c: [f32; 3], fallback: [f32; 3]| -> [f32; 3] {
            if c.iter().all(|x| x.is_finite()) {
                c.map(|x| x.clamp(0.0, Self::MAX_COLOR))
            } else {
                fallback
            }
        };
        let d = Self::default();
        let intensity = if self.intensity.is_finite() {
            self.intensity.clamp(0.0, Self::MAX_INTENSITY)
        } else {
            d.intensity
        };
        let sun_size = if self.sun_size.is_finite() {
            self.sun_size.clamp(Self::MIN_SUN_SIZE, Self::MAX_SUN_SIZE)
        } else {
            d.sun_size
        };
        let sun_intensity = if self.sun_intensity.is_finite() {
            self.sun_intensity.clamp(0.0, Self::MAX_SUN_INTENSITY)
        } else {
            d.sun_intensity
        };
        Self {
            enabled: self.enabled,
            intensity,
            zenith: color(self.zenith, d.zenith),
            horizon: color(self.horizon, d.horizon),
            ground: color(self.ground, d.ground),
            sun_size,
            sun_intensity,
        }
    }
}

/// 阴影的**画质**参数（软阴影 / 级间混合），持久化在 `project.ron` 的
/// `render.shadow` 节。
///
/// **默认关**（两个 0.0）：与 Bloom / 天空 / FXAA / TAA 同一条纪律 —— 画质增强
/// 默认不动画面。关着时着色器走今天那条 3×3 PCF + 硬级间切换的路径，逐位一致。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ShadowConfig {
    /// 软阴影（PCSS）的灯光尺寸。
    ///
    /// - `0.0`（默认）= 硬阴影：3×3 PCF，与引入 PCSS 之前逐位一致；
    /// - `> 0` = 灯光的相对尺寸（0.5 是"一盏有体积的灯"的常用值）：先做
    ///   blocker search 估出半影宽度，再按宽度做 PCF。值越大影子边缘越糊、
    ///   离遮挡物越远的影子越散。
    ///
    /// 坏值（负数 / NaN）按 0 处理（[`Self::sanitized`]），不夹取上界 ——
    /// 上界没有物理意义，太大的值只是"糊成一片"，不该由引擎替作者决定。
    pub softness: f32,
    /// 级间混合带宽（占**该级远端切分距离**的比例）。
    ///
    /// - `0.0`（默认）= 不混合：两级在切分处硬接，近处细、远处粗看得出一条缝；
    /// - `> 0` = 在切分前这么多比例的深度带里把两级混起来，缝被抹平。
    ///   `0.05` 是肉眼刚好看不见缝、额外采样又只落在薄薄一条带上的取值。
    ///
    /// 混合带里要采样两级，代价只落在这条带上（其余像素照旧一级一次）。
    pub cascade_blend: f32,
}

impl ShadowConfig {
    /// 与默认值完全一致时不落盘（`skip_serializing_if` 用）：没开过的项目
    /// `project.ron` 里不出现这一节。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 容错读取：清单是**手改的文本**，负数 / NaN / 无穷不能直接交给着色器
    /// （负半径会让 PCF 的采样点绕到反方向，NaN 会让整片影子消失）。
    /// 非有限值与负值一律当 0（= 关）。
    pub fn sanitized(self) -> Self {
        let clean = |v: f32| if v.is_finite() && v > 0.0 { v } else { 0.0 };
        Self {
            softness: clean(self.softness),
            cascade_blend: clean(self.cascade_blend),
        }
    }
}

/// 画面渲染参数（色调映射 / 曝光 / 泛光 / 天空），持久化在 `project.ron` 的
/// `render` 节。
///
/// 与 [`RuntimeConfig`] 同一套路：此前曝光是硬编码常量、色调映射根本不存在；
/// 现在它们是**数据** —— 编辑器 ⚙ Project Settings 可调、智能体改一行文本即可、
/// 编辑器 Play / 独立游戏 / 无头截图三端走同一份配置（画面一致性的单入口）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RenderConfig {
    /// 曝光倍数（线性光乘子，1.0 = 不增不减）。
    pub exposure: f32,
    /// 色调映射算子，见 [`ToneMapMode`]。
    pub tone_map: ToneMapMode,
    /// 泛光参数，见 [`BloomConfig`]。
    ///
    /// 后加的字段：`serde(default)` 让旧清单照常加载；关着时不落盘
    /// （老项目 diff 不出新节的噪声）。
    #[serde(default, skip_serializing_if = "BloomConfig::is_default")]
    pub bloom: BloomConfig,
    /// 天空盒 / 环境照明参数，见 [`SkyConfig`]。同样后加 + 默认关。
    #[serde(default, skip_serializing_if = "SkyConfig::is_default")]
    pub sky: SkyConfig,
    /// FXAA 抗锯齿参数，见 [`FxaaConfig`]。同样后加 + 默认关。
    #[serde(default, skip_serializing_if = "FxaaConfig::is_default")]
    pub fxaa: FxaaConfig,
    /// TAA（时域抗锯齿）参数，见 [`TaaConfig`]。同样后加 + 默认关。
    #[serde(default, skip_serializing_if = "TaaConfig::is_default")]
    pub taa: TaaConfig,
    /// SSAO（屏幕空间环境光遮蔽）参数，见 [`SsaoConfig`]。同样后加 + 默认关。
    #[serde(default, skip_serializing_if = "SsaoConfig::is_default")]
    pub ssao: SsaoConfig,
    /// 阴影画质参数（软阴影 / 级间混合），见 [`ShadowConfig`]。同样后加 + 默认关。
    #[serde(default, skip_serializing_if = "ShadowConfig::is_default")]
    pub shadow: ShadowConfig,
    /// LOD 距离偏置：乘在每级 `max_distance` 上。
    ///
    /// - `1.0`（默认）= 按作者写的阈值，逐位与引入 LOD 之前一致；
    /// - `> 1` 把切换点推远（更好的画质 / 更强的机器）；
    /// - `0 < bias < 1` 拉近（弱机降负载）；
    /// - `<= 0` **关掉 LOD**：永远第 0 级、也不做绘制距离剔除（排查"远处
    ///   突然变样"时的开关）。
    ///
    /// 坏值（`NaN` / `±∞`）在渲染侧按 1 处理，不在这里夹取 —— 夹取会让
    /// "作者写了什么"与"实际跑什么"分叉，而这个字段本身就是调试用的。
    #[serde(default = "default_lod_bias", skip_serializing_if = "is_default_lod_bias")]
    pub lod_bias: f32,
}

fn default_lod_bias() -> f32 {
    1.0
}

fn is_default_lod_bias(bias: &f32) -> bool {
    *bias == 1.0
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            tone_map: ToneMapMode::Aces,
            bloom: BloomConfig::default(),
            sky: SkyConfig::default(),
            fxaa: FxaaConfig::default(),
            taa: TaaConfig::default(),
            ssao: SsaoConfig::default(),
            shadow: ShadowConfig::default(),
            lod_bias: default_lod_bias(),
        }
    }
}

impl RenderConfig {
    /// 曝光的安全范围。
    ///
    /// 下界不是 0：曝光 0 = 全黑画面（不是"关掉曝光"，只有 `tone_map` 才有
    /// 关的概念）；上界 20 已足够"夜里看太阳"，再大只是整幅纯白。
    pub const MIN_EXPOSURE: f32 = 0.05;
    pub const MAX_EXPOSURE: f32 = 20.0;

    /// 容错读取：清单是**手改的文本**，非法值不能直接交给渲染器。
    ///
    /// - 曝光 NaN / 无穷 / 非正 → 退回默认 1.0；
    /// - 合法但离谱的量级夹进 [`Self::MIN_EXPOSURE`] / [`Self::MAX_EXPOSURE`]；
    /// - Bloom 三件套走 [`BloomConfig::sanitized`]；
    /// - 天空走 [`SkyConfig::sanitized`]。
    pub fn sanitized(self) -> Self {
        let exposure = if self.exposure.is_finite() && self.exposure > 0.0 {
            self.exposure.clamp(Self::MIN_EXPOSURE, Self::MAX_EXPOSURE)
        } else {
            Self::default().exposure
        };
        Self {
            exposure,
            tone_map: self.tone_map,
            bloom: self.bloom.sanitized(),
            sky: self.sky.sanitized(),
            // 布尔开关没有"非法值"可夹；保留字段是为了让 sanitized 的返回
            // 结构完整（漏写会编译失败，这是刻意的）。
            fxaa: self.fxaa,
            taa: self.taa.sanitized(),
            ssao: self.ssao.sanitized(),
            shadow: self.shadow.sanitized(),
            // 偏置刻意不夹：见字段说明（坏值由 `lod::LodGroup::resolve` 兜底）。
            lod_bias: self.lod_bias,
        }
    }

    /// 本帧**实际**生效的 FXAA 配置。
    ///
    /// TAA 开着时 FXAA 让位：两者都在抹边缘，叠起来只是把画面糊两遍，还会
    /// 把 TAA 收敛出来的细节重新抹掉。这条规则只有一份实现（这里是唯一的
    /// 判定点），三宿主不必各自判断，也不会出现"编辑器糊、游戏不糊"。
    pub fn effective_fxaa(&self) -> FxaaConfig {
        FxaaConfig {
            enabled: self.fxaa.enabled && !self.taa.enabled,
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_window_config() {
        let cfg = WindowConfig::default();
        assert_eq!(cfg.title, "XGameEngine");
        assert!(cfg.resizable);
    }

    #[test]
    fn serde_roundtrip() {
        let cfg = EngineConfig::default();
        let ron_str = ron::to_string(&cfg).expect("serialize");
        let restored: EngineConfig = ron::from_str(&ron_str).expect("deserialize");
        assert_eq!(restored.target_fps, 0);
        assert_eq!(restored.window.width, 1280);
    }

    #[test]
    fn runtime_config_defaults_match_legacy_hardcoded_values() {
        // 这两个值就是当年 WorldConfig::default() 里硬编码的那一对 ——
        // 改默认值会让所有旧项目的物理行为悄悄变样。
        let rt = RuntimeConfig::default();
        assert_eq!(rt.gravity, [0.0, -9.81, 0.0]);
        assert_eq!(rt.fixed_dt, 1.0 / 60.0);
    }

    #[test]
    fn runtime_config_sanitized_rejects_garbage() {
        // 固定步长 0 = 累加器永远凑不满一步 = 时间静止，必须退回默认。
        let zero = RuntimeConfig {
            fixed_dt: 0.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(zero.fixed_dt, 1.0 / 60.0);

        let nan = RuntimeConfig {
            fixed_dt: f32::NAN,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(nan.fixed_dt, 1.0 / 60.0);

        let wild = RuntimeConfig {
            gravity: [f32::NAN, -1e30, 0.0],
            ..Default::default()
        }
        .sanitized();
        assert_eq!(wild.gravity, RuntimeConfig::default().gravity);

        // 合法但过慢的步长要夹住（10Hz 物理肉眼可见地卡）。
        let slow = RuntimeConfig {
            fixed_dt: 0.5,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(slow.fixed_dt, 1.0 / 10.0);
    }

    /// 渲染参数默认值 = ACES + 曝光 1.0。
    ///
    /// 这条钉死两件事：① 新场景默认就是色调映射后的画面（ACES 不是
    /// 可选项而是基线）；② 曝光 1.0 是"不增不减"，历史场景的亮度标定
    /// （`radiance = intensity × π`）不被悄悄改档。
    #[test]
    fn render_config_defaults_to_aces_and_unit_exposure() {
        let r = RenderConfig::default();
        assert_eq!(r.tone_map, ToneMapMode::Aces);
        assert_eq!(r.exposure, 1.0);
    }

    /// 曝光 0 / 负数 / NaN / 无穷必须退回默认（全黑画面不合法），
    /// 合法但离谱的量级（0.001 / 1e6）夹进安全范围。
    #[test]
    fn render_config_sanitized_rejects_garbage() {
        let zero = RenderConfig {
            exposure: 0.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(zero.exposure, 1.0, "曝光 0 = 全黑，必须退回默认");

        let negative = RenderConfig {
            exposure: -3.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(negative.exposure, 1.0);

        let nan = RenderConfig {
            exposure: f32::NAN,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(nan.exposure, 1.0);

        let inf = RenderConfig {
            exposure: f32::INFINITY,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(inf.exposure, 1.0);

        let tiny = RenderConfig {
            exposure: 0.0001,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(tiny.exposure, RenderConfig::MIN_EXPOSURE);

        let huge = RenderConfig {
            exposure: 1e9,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(huge.exposure, RenderConfig::MAX_EXPOSURE);

        // 合法值原样保留，色调映射开关不受曝光夹取影响。
        let ok = RenderConfig {
            exposure: 2.5,
            tone_map: ToneMapMode::Off,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(ok.exposure, 2.5);
        assert_eq!(ok.tone_map, ToneMapMode::Off);
    }

    /// Bloom 默认必须是**关**（intensity 0）：它是风格参数，默认改变画面
    /// 是不可接受的静默行为；要发光由用户显式调 intensity。
    #[test]
    fn bloom_defaults_to_off() {
        let b = BloomConfig::default();
        assert_eq!(b.intensity, 0.0, "Bloom 默认必须关闭");
        assert!(b.is_default());

        // 默认配置序列化后不出现 bloom 节（没调过画面的项目 diff 干净）。
        let text = ron::to_string(&RenderConfig::default()).expect("serialize");
        assert!(!text.contains("bloom"), "未调过的 Bloom 不该落盘: {text}");

        // 关了光晕但调过阈值 → 照样落盘（用户调过的参数不悄悄丢）。
        let tuned = RenderConfig {
            bloom: BloomConfig {
                threshold: 2.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let text = ron::to_string(&tuned).expect("serialize");
        assert!(text.contains("bloom"), "调过的参数必须落盘: {text}");
        let back: RenderConfig = ron::from_str(&text).expect("deserialize");
        assert_eq!(back, tuned);
    }

    /// FXAA 默认必须**关**：MSAA 已经处理了几何边缘，默认再叠一层后处理
    /// 会改变所有旧场景的画面（与 Bloom / 天空同一条纪律）。
    #[test]
    fn fxaa_defaults_to_off_and_roundtrips() {
        assert!(!FxaaConfig::default().enabled, "FXAA 默认必须关闭");
        assert!(FxaaConfig::default().is_default());

        // 默认配置序列化后不出现 fxaa 节（没开过的项目 diff 干净）。
        let text = ron::to_string(&RenderConfig::default()).expect("serialize");
        assert!(!text.contains("fxaa"), "未开过的 FXAA 不该落盘: {text}");

        // 显式打开 → 落盘 → 读回，开关不能在半路丢。
        let on = RenderConfig {
            fxaa: FxaaConfig { enabled: true },
            ..Default::default()
        };
        let text = ron::to_string(&on).expect("serialize");
        assert!(text.contains("fxaa"), "开着的 FXAA 必须落盘: {text}");
        let back: RenderConfig = ron::from_str(&text).expect("deserialize");
        assert_eq!(back, on);
        assert!(back.fxaa.enabled);
    }

    /// TAA 默认必须**关**，且默认配置不落盘。
    #[test]
    fn taa_defaults_to_off_and_roundtrips() {
        assert!(!TaaConfig::default().enabled, "TAA 默认必须关闭");
        assert!(TaaConfig::default().is_default());

        let text = ron::to_string(&RenderConfig::default()).expect("serialize");
        assert!(!text.contains("taa"), "未开过的 TAA 不该落盘: {text}");

        let on = RenderConfig {
            taa: TaaConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let text = ron::to_string(&on).expect("serialize");
        assert!(text.contains("taa"), "开着的 TAA 必须落盘: {text}");
        let back: RenderConfig = ron::from_str(&text).expect("deserialize");
        assert_eq!(back, on);
        assert!(back.taa.enabled);
        assert_eq!(back.taa.history_weight, TaaConfig::default().history_weight);
    }

    /// 历史权重：NaN / 无穷落回默认；1.0 夹到上限（权重 1 = 画面冻在第一帧）。
    #[test]
    fn taa_history_weight_is_clamped() {
        let nan = TaaConfig {
            history_weight: f32::NAN,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(nan.history_weight, TaaConfig::default().history_weight);

        let inf = TaaConfig {
            history_weight: f32::INFINITY,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(inf.history_weight, TaaConfig::default().history_weight);

        let frozen = TaaConfig {
            enabled: true,
            history_weight: 1.0,
        }
        .sanitized();
        assert_eq!(
            frozen.history_weight,
            TaaConfig::MAX_HISTORY_WEIGHT,
            "权重 1 会把画面冻住，必须夹住"
        );
        assert!(frozen.enabled, "夹权重不该顺手改开关");

        let negative = TaaConfig {
            history_weight: -0.5,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(negative.history_weight, 0.0, "负权重 = 只用当前帧");
    }

    /// SSAO 默认必须**关**，且默认配置不落盘（与 Bloom / TAA 同一条纪律）。
    #[test]
    fn ssao_defaults_to_off_and_roundtrips() {
        assert!(!SsaoConfig::default().enabled, "SSAO 默认必须关闭");
        assert!(SsaoConfig::default().is_default());

        let text = ron::to_string(&RenderConfig::default()).expect("serialize");
        assert!(!text.contains("ssao"), "未开过的 SSAO 不该落盘: {text}");

        let on = RenderConfig {
            ssao: SsaoConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let text = ron::to_string(&on).expect("serialize");
        assert!(text.contains("ssao"), "开着的 SSAO 必须落盘: {text}");
        let back: RenderConfig = ron::from_str(&text).expect("deserialize");
        assert_eq!(back, on);
        assert!(back.ssao.enabled);
        assert_eq!(back.ssao.strength, SsaoConfig::default().strength);
    }

    /// SSAO 容错：NaN / 无穷落回默认；超限夹到上限；负数按 0（= 无遮蔽效果）。
    #[test]
    fn ssao_config_sanitized_rejects_garbage() {
        let nan = SsaoConfig {
            strength: f32::NAN,
            radius: f32::INFINITY,
            bias: f32::NAN,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(nan.strength, SsaoConfig::default().strength);
        assert_eq!(nan.radius, SsaoConfig::default().radius);
        assert_eq!(nan.bias, SsaoConfig::default().bias);

        let clamped = SsaoConfig {
            enabled: true,
            strength: 100.0,
            radius: 50.0,
            bias: 9.0,
        }
        .sanitized();
        assert_eq!(clamped.strength, SsaoConfig::MAX_STRENGTH);
        assert_eq!(clamped.radius, SsaoConfig::MAX_RADIUS);
        assert_eq!(clamped.bias, SsaoConfig::MAX_BIAS);
        assert!(clamped.enabled, "夹取不该顺手改开关");

        let negative = SsaoConfig {
            strength: -1.0,
            radius: -0.5,
            bias: -0.1,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(negative.strength, 0.0);
        assert_eq!(negative.radius, 0.0);
        assert_eq!(negative.bias, 0.0);
    }

    /// 软阴影 / 级间混合默认必须**关**，且默认配置不落盘（与 Bloom / TAA / SSAO
    /// 同一条纪律）：没开过的项目 `project.ron` 里不出现 `render.shadow` 节，
    /// 老项目读进来就是全 0 = 走引入软阴影之前的 3×3 PCF 老路径。
    #[test]
    fn shadow_defaults_to_off_and_roundtrips() {
        assert_eq!(
            ShadowConfig::default(),
            ShadowConfig {
                softness: 0.0,
                cascade_blend: 0.0
            },
            "两个参数默认都必须是 0（= 关）"
        );
        assert!(ShadowConfig::default().is_default());

        let text = ron::to_string(&RenderConfig::default()).expect("serialize");
        assert!(!text.contains("shadow"), "未开过的软阴影不该落盘: {text}");

        let on = RenderConfig {
            shadow: ShadowConfig {
                softness: 0.5,
                cascade_blend: 0.05,
            },
            ..Default::default()
        };
        let text = ron::to_string(&on).expect("serialize");
        assert!(text.contains("shadow"), "开着的软阴影必须落盘: {text}");
        let back: RenderConfig = ron::from_str(&text).expect("deserialize");
        assert_eq!(back, on);

        // 老项目（清单里没有 shadow 节）读进来 = 默认全关：新字段不许改老画面。
        let legacy: RenderConfig =
            ron::from_str("(exposure: 1.0, tone_map: Aces)").expect("老 render 配置也要能读");
        assert_eq!(legacy.shadow, ShadowConfig::default());
    }

    /// 软阴影容错：清单是手改的文本，负数 / NaN / 无穷不能直接进着色器
    /// （负尺寸会把采样点绕到反方向，NaN 会让整片影子消失），一律当 0（= 关）。
    #[test]
    fn shadow_config_sanitized_rejects_garbage() {
        let nan = ShadowConfig {
            softness: f32::NAN,
            cascade_blend: f32::INFINITY,
        }
        .sanitized();
        assert_eq!(nan.softness, 0.0);
        assert_eq!(nan.cascade_blend, 0.0);

        let negative = ShadowConfig {
            softness: -0.5,
            cascade_blend: -0.1,
        }
        .sanitized();
        assert_eq!(negative.softness, 0.0);
        assert_eq!(negative.cascade_blend, 0.0);

        let ok = ShadowConfig {
            softness: 0.5,
            cascade_blend: 0.05,
        }
        .sanitized();
        assert_eq!(ok.softness, 0.5);
        assert_eq!(ok.cascade_blend, 0.05);
    }

    /// TAA 开着时 FXAA 必须自动让位（两者叠加只是糊两遍）。
    #[test]
    fn taa_supersedes_fxaa() {
        let both = RenderConfig {
            fxaa: FxaaConfig { enabled: true },
            taa: TaaConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        }
        .sanitized();
        assert!(both.fxaa.enabled, "用户填的开关原样保留（不偷偷改写配置）");
        assert!(!both.effective_fxaa().enabled, "但实际生效的 FXAA 必须关掉");

        // TAA 关着时 FXAA 照旧按用户填的来。
        let fxaa_only = RenderConfig {
            fxaa: FxaaConfig { enabled: true },
            ..Default::default()
        }
        .sanitized();
        assert!(fxaa_only.effective_fxaa().enabled);
    }

    /// Bloom 容错：NaN / 负数 / 超限。intensity 负数 = 关（不是"反向光晕"）。
    #[test]
    fn bloom_config_sanitized_rejects_garbage() {
        let nan = BloomConfig {
            intensity: f32::NAN,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(nan.intensity, 0.0, "NaN 强度必须落回默认（关）");

        let negative = BloomConfig {
            intensity: -2.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(negative.intensity, 0.0, "负强度 = 关，不是反向光晕");

        let huge = BloomConfig {
            intensity: 1e6,
            threshold: -5.0,
            knee: f32::INFINITY,
        }
        .sanitized();
        assert_eq!(huge.intensity, BloomConfig::MAX_INTENSITY);
        assert_eq!(huge.threshold, 0.0, "负阈值夹到 0（全亮部都提取）");
        assert_eq!(huge.knee, BloomConfig::default().knee, "无穷膝宽落回默认");

        let ok = BloomConfig {
            intensity: 0.6,
            threshold: 1.2,
            knee: 0.5,
        }
        .sanitized();
        assert_eq!(ok.intensity, 0.6);
        assert_eq!(ok.threshold, 1.2);
        assert_eq!(ok.knee, 0.5);
    }

    /// 天空默认必须是**关**：它是风格参数，默认改变旧项目画面不可接受
    /// （与 Bloom 同一条纪律）。默认配置序列化后不出现 sky 节。
    #[test]
    fn sky_defaults_to_off() {
        let s = SkyConfig::default();
        assert!(!s.enabled, "天空默认必须关闭");
        assert!(s.is_default());

        let text = ron::to_string(&RenderConfig::default()).expect("serialize");
        assert!(!text.contains("sky"), "未调过的天空不该落盘: {text}");

        // 关着但调过颜色 → 照样落盘（用户调过的参数不悄悄丢），且往返一致。
        let tuned = RenderConfig {
            sky: SkyConfig {
                zenith: [0.1, 0.2, 0.9],
                ..Default::default()
            },
            ..Default::default()
        };
        let text = ron::to_string(&tuned).expect("serialize");
        assert!(text.contains("sky"), "调过的参数必须落盘: {text}");
        let back: RenderConfig = ron::from_str(&text).expect("deserialize");
        assert_eq!(back, tuned);
    }

    /// 天空容错：NaN / 负强度 / 离谱太阳尺寸与颜色全部落到安全值。
    #[test]
    fn sky_config_sanitized_rejects_garbage() {
        let nan = SkyConfig {
            intensity: f32::NAN,
            sun_size: f32::NAN,
            sun_intensity: f32::INFINITY,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(nan.intensity, SkyConfig::default().intensity);
        assert_eq!(nan.sun_size, SkyConfig::default().sun_size);
        assert_eq!(
            nan.sun_intensity,
            SkyConfig::default().sun_intensity,
            "无穷太阳强度必须落回默认"
        );

        let negative = SkyConfig {
            intensity: -3.0,
            sun_intensity: -1.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(negative.intensity, 0.0, "负强度 = 关，不是反向天空");
        assert_eq!(negative.sun_intensity, 0.0);

        // 太阳尺寸过小 / 过大都夹住；颜色分量为负或超限都夹住。
        let wild = SkyConfig {
            sun_size: 0.001,
            zenith: [-1.0, 200.0, f32::NAN],
            ground: [f32::INFINITY, 0.5, 0.5],
            ..Default::default()
        }
        .sanitized();
        assert_eq!(wild.sun_size, SkyConfig::MIN_SUN_SIZE);
        assert_eq!(
            wild.zenith,
            SkyConfig::default().zenith,
            "含 NaN 的颜色整组退回默认"
        );
        assert_eq!(
            wild.ground,
            SkyConfig::default().ground,
            "含无穷的颜色整组退回默认"
        );

        // 有限但越界的颜色**只夹取、不整组退回**：一个分量写坏不该把同行的
        // 另外两个也丢回默认（用户调过的参数不悄悄丢）。
        let clamped = SkyConfig {
            zenith: [-1.0, 200.0, 0.5],
            ..Default::default()
        }
        .sanitized();
        assert_eq!(clamped.zenith, [0.0, SkyConfig::MAX_COLOR, 0.5]);

        // 合法输入逐字段原样保留（HDR 颜色允许超过 1）。
        let ok = SkyConfig {
            enabled: true,
            intensity: 1.4,
            zenith: [2.0, 1.0, 0.5],
            horizon: [0.7, 0.8, 0.9],
            ground: [0.2, 0.2, 0.2],
            sun_size: 2.5,
            sun_intensity: 3.0,
        }
        .sanitized();
        let expected = SkyConfig {
            enabled: true,
            intensity: 1.4,
            zenith: [2.0, 1.0, 0.5],
            horizon: [0.7, 0.8, 0.9],
            ground: [0.2, 0.2, 0.2],
            sun_size: 2.5,
            sun_intensity: 3.0,
        };
        assert_eq!(ok, expected, "合法输入必须逐字段原样保留");
    }
}
