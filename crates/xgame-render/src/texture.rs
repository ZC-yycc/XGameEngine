/// 创建纹理时的采样 / 编码选项（来自资产 `.meta` 的导入设置）。
///
/// 这是"导入参数真的生效"的落点：颜色贴图要 sRGB（否则会偏亮 / 偏暗），
/// 法线 / 粗糙度贴图必须线性；像素风资源要最近邻（否则 16×16 的贴图
/// 会被插值糊成一团）。默认值与 `ImportSettings::default()` 对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureOptions {
    /// 是否按 sRGB 采样（`true` = 颜色贴图；`false` = 数据贴图）。
    pub srgb: bool,
    /// 是否用最近邻过滤（像素风 / UI 图标）。
    pub nearest: bool,
}

impl Default for TextureOptions {
    fn default() -> Self {
        Self {
            srgb: true,
            nearest: false,
        }
    }
}

/// GPU 驻留纹理，包含其视图和采样器。
pub struct Texture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

impl Texture {
    /// 从原始 RGBA8 像素数据创建纹理（默认 sRGB + 线性过滤）。
    pub fn from_rgba8(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        data: &[u8],
        label: &str,
    ) -> Self {
        Self::from_rgba8_with(
            device,
            queue,
            width,
            height,
            data,
            label,
            TextureOptions::default(),
        )
    }

    /// 从原始 RGBA8 像素数据创建纹理，可指定 sRGB / 过滤方式。
    pub fn from_rgba8_with(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        data: &[u8],
        label: &str,
        options: TextureOptions,
    ) -> Self {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if options.srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let filter = if options.nearest {
            wgpu::FilterMode::Nearest
        } else {
            wgpu::FilterMode::Linear
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{} sampler", label)),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: filter,
            min_filter: filter,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            texture,
            view,
            sampler,
        }
    }

    /// 从图片文件加载纹理（材质资产的 `albedo` 走这里）。
    ///
    /// 图片解码直接解成 RGBA8、再按 `options` 建 GPU 纹理；失败返回字符串而
    /// 不是 panic —— 一个坏贴图不该让整个游戏起不来（调用方退回白纹理）。
    pub fn from_file(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: impl AsRef<std::path::Path>,
        options: TextureOptions,
        label: &str,
    ) -> Result<Self, String> {
        let path = path.as_ref();
        let image =
            image::open(path).map_err(|e| format!("无法读取纹理 '{}': {e}", path.display()))?;
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        Ok(Self::from_rgba8_with(
            device,
            queue,
            width,
            height,
            rgba.as_raw(),
            label,
            options,
        ))
    }

    /// 创建 1×1 白色回退纹理（当模型没有纹理时使用）。
    pub fn white(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self::from_rgba8(device, queue, 1, 1, &[255, 255, 255, 255], "fallback white")
    }
}
