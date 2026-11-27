use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::Path;

use xgc_base::EntityId;
use xgc_world::{
    Camera, GlobalTransform, LodGroup, LodLevel, SceneEnvironment, Script, Transform, World,
    WorldData,
};

const ZONE_SPACING: f32 = 16.0;
pub(super) const CAMERA_Z: f32 = 4.0;
const LOD_RADIUS: f32 = 7.0;
const CAMERA_SCRIPT: &str = "camera.lua";
const READY_FILE: &str = "fixture-ready";
const FIXTURE_VERSION: &str = "xgc-s5-rgba8-v1\n";
pub(super) const FULL_RGBA8_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(super) struct Profile {
    pub(super) zones: usize,
    texture_edge: usize,
    pub(super) steps: i32,
    steps_per_zone: i32,
}

pub(super) const SMOKE: Profile = Profile {
    zones: 3,
    texture_edge: 32,
    steps: 48,
    steps_per_zone: 8,
};
pub(super) const FULL: Profile = Profile {
    zones: 128,
    texture_edge: 2048,
    steps: 3_600,
    steps_per_zone: 14,
};

impl Profile {
    fn turn_step(self) -> i32 {
        (self.zones as i32 - 1) * self.steps_per_zone
    }

    pub(super) fn camera_x(self, frame: i32) -> f32 {
        let outbound = frame + 1 - self.steps_per_zone;
        let step = outbound.min((2 * self.turn_step() - outbound).max(0));
        step as f32 * ZONE_SPACING / self.steps_per_zone as f32
    }

    pub(super) fn outbound_center(self, frame: i32) -> Option<usize> {
        let outbound = frame + 1 - self.steps_per_zone;
        ((0..=self.turn_step()).contains(&outbound) && outbound % self.steps_per_zone == 0)
            .then_some((outbound / self.steps_per_zone) as usize)
    }

    pub(super) fn zone_at(self, x: f32) -> Option<usize> {
        let zone = (x / ZONE_SPACING).round() as i32;
        let dx = x - zone as f32 * ZONE_SPACING;
        ((0..self.zones as i32).contains(&zone)
            && dx * dx + CAMERA_Z * CAMERA_Z <= LOD_RADIUS.powi(2))
        .then_some(zone as usize)
    }

    fn camera_script(self) -> String {
        format!(
            r#"local frame = 0
function update(ctx)
    frame = frame + 1
    local outbound = frame - {steps_per_zone}
    local step = math.min(outbound, math.max(0, {return_step} - outbound))
    set_pos(ctx, step * {ZONE_SPACING} / {steps_per_zone}, 0, {CAMERA_Z})
end
"#,
            steps_per_zone = self.steps_per_zone,
            return_step = 2 * self.turn_step(),
        )
    }

    pub(super) fn rgba8_bytes(self) -> u64 {
        self.zones as u64 * self.texture_edge as u64 * self.texture_edge as u64 * 4
    }
}

fn model(zone: usize) -> String {
    format!("zone-{zone:03}.glb")
}

// PNG 用无压缩 DEFLATE 保存真实 RGBA8：盘上体量来自逐像素数据，导入后整张上传。
const CRC32: [u32; 256] = {
    let mut table = [0; 256];
    let mut index = 0;
    while index < table.len() {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xedb8_8320
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
};

fn png_chunk(output: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    let start = output.len();
    output.extend_from_slice(kind);
    output.extend_from_slice(bytes);
    let crc = output[start..].iter().fold(u32::MAX, |crc, &byte| {
        (crc >> 8) ^ CRC32[((crc ^ u32::from(byte)) & 255) as usize]
    });
    output.extend_from_slice(&(!crc).to_be_bytes());
}

const DEFLATE_BLOCK: usize = u16::MAX as usize;

fn png_size(edge: usize) -> usize {
    let scanlines = edge * (edge * 4 + 1);
    63 + scanlines + scanlines.div_ceil(DEFLATE_BLOCK) * 5
}

fn texture_png(edge: usize, zone: usize) -> Vec<u8> {
    let mut rows = Vec::with_capacity(edge * (edge * 4 + 1));
    let red = 50 + (zone % 4) as u8 * 52;
    let green = 50 + (zone / 4 % 4) as u8 * 52;
    let blue = 48 + (zone / 16) as u8 * 24;
    for y in 0..edge {
        rows.push(0); // PNG Filter None。
        for x in 0..edge {
            let shade = ((x ^ y ^ (x * y)) & 7) as u8;
            rows.extend_from_slice(&[red + shade, green + shade, blue + shade, 255]);
        }
    }
    let mut zlib = Vec::with_capacity(rows.len() + rows.len().div_ceil(DEFLATE_BLOCK) * 5 + 6);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let blocks = rows.chunks(DEFLATE_BLOCK);
    let block_count = blocks.len();
    for (index, block) in blocks.enumerate() {
        let len = block.len() as u16;
        zlib.push(u8::from(index + 1 == block_count));
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in &rows {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = Vec::with_capacity(png_size(edge));
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = [0; 13];
    let side = (edge as u32).to_be_bytes();
    ihdr[..4].copy_from_slice(&side);
    ihdr[4..8].copy_from_slice(&side);
    ihdr[8..10].copy_from_slice(&[8, 6]);
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &zlib);
    png_chunk(&mut png, b"IEND", &[]);
    assert_eq!(png.len(), png_size(edge));
    png
}

const QUAD_BYTES: usize = 92;

fn glb_prefix(edge: usize) -> (Vec<u8>, usize) {
    // 视距内正对相机时四边形覆盖探针像素，UV 取满整张贴图。
    let png_bytes = png_size(edge);
    let bin_bytes = QUAD_BYTES + png_bytes.next_multiple_of(4);
    let mut json = format!(
        r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
    "buffers":[{{"byteLength":{bin_bytes}}}],"bufferViews":[{{"buffer":0,"byteLength":48}},
    {{"buffer":0,"byteOffset":48,"byteLength":32}},{{"buffer":0,"byteOffset":80,"byteLength":12}},
    {{"buffer":0,"byteOffset":{QUAD_BYTES},"byteLength":{png_bytes}}}],
    "accessors":[{{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-5,-3,0],"max":[5,3,0]}},
    {{"bufferView":1,"componentType":5126,"count":4,"type":"VEC2"}},{{"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"}}],
    "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"TEXCOORD_0":1}},"indices":2,"material":0}}]}}],
    "materials":[{{"pbrMetallicRoughness":{{"metallicFactor":0,"baseColorTexture":{{"index":0}}}}}}],
    "textures":[{{"source":0}}],"images":[{{"bufferView":3,"mimeType":"image/png"}}]}}"#
    )
    .into_bytes();
    json.resize(json.len().next_multiple_of(4), b' ');
    let file_bytes = 28 + json.len() + bin_bytes;
    let mut prefix = Vec::with_capacity(28 + json.len() + QUAD_BYTES);
    prefix.extend_from_slice(b"glTF");
    prefix.extend_from_slice(&2_u32.to_le_bytes());
    prefix.extend_from_slice(&(file_bytes as u32).to_le_bytes());
    prefix.extend_from_slice(&(json.len() as u32).to_le_bytes());
    prefix.extend_from_slice(b"JSON");
    prefix.extend_from_slice(&json);
    prefix.extend_from_slice(&(bin_bytes as u32).to_le_bytes());
    prefix.extend_from_slice(b"BIN\0");
    for value in [
        -5.0_f32, -3.0, 0.0, 5.0, -3.0, 0.0, 5.0, 3.0, 0.0, -5.0, 3.0, 0.0,
    ] {
        prefix.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0_f32, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0] {
        prefix.extend_from_slice(&value.to_le_bytes());
    }
    for index in [0_u16, 1, 2, 0, 2, 3] {
        prefix.extend_from_slice(&index.to_le_bytes());
    }
    (prefix, file_bytes)
}

pub(super) fn write_assets(root: &Path, profile: Profile) {
    fs::create_dir(root).expect("夹具目录必须尚不存在");
    let (prefix, file_bytes) = glb_prefix(profile.texture_edge);
    for zone in 0..profile.zones {
        let png = texture_png(profile.texture_edge, zone);
        let padding = file_bytes - prefix.len() - png.len();
        let mut out = BufWriter::new(File::create_new(root.join(model(zone))).unwrap());
        out.write_all(&prefix).unwrap();
        out.write_all(&png).unwrap();
        out.write_all(&[0; 3][..padding]).unwrap();
        out.flush().unwrap();
    }
    fs::write(root.join(CAMERA_SCRIPT), profile.camera_script()).unwrap();
    fs::write(root.join(READY_FILE), FIXTURE_VERSION).unwrap();
}

pub(super) fn verify_assets(root: &Path, profile: Profile) -> u64 {
    assert_eq!(
        fs::read_to_string(root.join(READY_FILE)).unwrap(),
        FIXTURE_VERSION
    );
    assert_eq!(
        fs::read_to_string(root.join(CAMERA_SCRIPT)).unwrap(),
        profile.camera_script()
    );
    let (prefix, file_bytes) = glb_prefix(profile.texture_edge);
    let mut header = vec![0; prefix.len() + 29];
    let side = (profile.texture_edge as u32).to_be_bytes();
    let mut disk_bytes = 0;
    for zone in 0..profile.zones {
        // 只核对 GLB 结构和 PNG 尺寸，不把整个模型读入文件系统缓存。
        let mut file = File::open(root.join(model(zone))).unwrap();
        let bytes = file.metadata().unwrap().len();
        assert_eq!(bytes, file_bytes as u64);
        file.read_exact(&mut header).unwrap();
        let (glb, png) = header.split_at(prefix.len());
        assert_eq!(glb, prefix.as_slice());
        assert_eq!(&png[..16], b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR");
        assert_eq!(png[16..20], side);
        assert_eq!(png[20..24], side);
        assert_eq!(png[24..29], [8, 6, 0, 0, 0]);
        disk_bytes += bytes;
    }
    disk_bytes
}

pub(super) fn scene(profile: Profile) -> (WorldData, EntityId) {
    let mut world = World::new();
    // 方向光的阴影包围盒会提前导入全部分区。均匀光照也让探针区分不同贴图。
    world.set_environment(SceneEnvironment {
        lights: Vec::new(),
        ambient: [1.0; 3],
    });
    let camera_transform = Transform {
        position: [-ZONE_SPACING, 0.0, CAMERA_Z],
        ..Transform::default()
    };
    let camera = world.spawn((
        camera_transform,
        GlobalTransform::from_local(&camera_transform),
        Camera::default(),
        Script {
            paths: vec![CAMERA_SCRIPT.into()],
        },
    ));
    let camera_id = world.entity(camera).unwrap();
    for zone in 0..profile.zones {
        let transform = Transform {
            position: [zone as f32 * ZONE_SPACING, 0.0, 0.0],
            ..Transform::default()
        };
        world.spawn((
            transform,
            GlobalTransform::from_local(&transform),
            LodGroup {
                levels: vec![LodLevel {
                    model: model(zone),
                    max_distance: LOD_RADIUS,
                }],
            },
        ));
    }
    (world.world_data(), camera_id)
}

pub(super) fn restore(data: &WorldData) -> World {
    let mut world = World::new();
    world.restore_world_data(data).unwrap();
    world
}
