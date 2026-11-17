//! 相机矩阵：从 `render.camera` + `GlobalTransform` 生成 view-projection。
//! 列主序，WebGPU 的 NDC z ∈ [0,1]；没有 active 相机时调用方回退单位矩阵。

use xgc_base::EntityId;
use xgc_world::{Camera, GlobalTransform, World};

pub type Mat4 = [f32; 16];

pub const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// 局部 `GlobalTransform` → 模型矩阵（缩放 → 旋转 → 平移）。
pub fn model_matrix(transform: &GlobalTransform) -> Mat4 {
    let x = quat_rotate(transform.rotation, [transform.scale[0], 0.0, 0.0]);
    let y = quat_rotate(transform.rotation, [0.0, transform.scale[1], 0.0]);
    let z = quat_rotate(transform.rotation, [0.0, 0.0, transform.scale[2]]);
    [
        x[0],
        x[1],
        x[2],
        0.0, //
        y[0],
        y[1],
        y[2],
        0.0, //
        z[0],
        z[1],
        z[2],
        0.0, //
        transform.position[0],
        transform.position[1],
        transform.position[2],
        1.0,
    ]
}

/// 第一台 `active` 相机（EntityId 最小），返回 proj * view。
pub fn camera_view_projection(world: &World, width: u32, height: u32) -> Option<Mat4> {
    camera_view_projection_with_eye(world, width, height).map(|(matrix, _)| matrix)
}

/// 同上，另外给出相机世界坐标：片元着色需要它把面法线朝向相机一侧。
pub fn camera_view_projection_with_eye(
    world: &World,
    width: u32,
    height: u32,
) -> Option<(Mat4, [f32; 3])> {
    let mut selected: Option<(EntityId, Mat4, [f32; 3])> = None;
    world.for_each::<Camera>(|key, camera| {
        if !camera.active {
            return;
        }
        let Some(entity) = world.entity(key) else {
            return;
        };
        if selected
            .as_ref()
            .is_some_and(|(existing, _, _)| *existing <= entity)
        {
            return;
        }
        let Some(transform) = world.get::<GlobalTransform>(key) else {
            return;
        };
        let forward = quat_rotate(transform.rotation, [0.0, 0.0, -1.0]);
        let eye = transform.position;
        let target = [
            eye[0] + forward[0],
            eye[1] + forward[1],
            eye[2] + forward[2],
        ];
        let view = look_at_rh(eye, target, [0.0, 1.0, 0.0]);
        let aspect = f32::from(width.max(1) as u16) / f32::from(height.max(1) as u16);
        let projection = perspective_rh_zo(
            camera.fov_y_deg.to_radians(),
            aspect,
            camera.near.max(0.0001),
            camera.far,
        );
        selected = Some((entity, mul(projection, view), eye));
    });
    selected.map(|(_, matrix, eye)| (matrix, eye))
}

pub fn perspective_rh_zo(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    [
        f / aspect.max(0.0001),
        0.0,
        0.0,
        0.0,
        0.0,
        f,
        0.0,
        0.0,
        0.0,
        0.0,
        far / (near - far),
        -1.0,
        0.0,
        0.0,
        (far * near) / (near - far),
        0.0,
    ]
}

pub fn look_at_rh(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let forward = normalize(sub(target, eye));
    let mut right = normalize(cross(forward, up));
    if length(right) < 0.0001 {
        right = normalize(cross(forward, [0.0, 0.0, 1.0]));
    }
    let up = cross(right, forward);
    [
        right[0],
        up[0],
        -forward[0],
        0.0,
        right[1],
        up[1],
        -forward[1],
        0.0,
        right[2],
        up[2],
        -forward[2],
        0.0,
        -dot(right, eye),
        -dot(up, eye),
        dot(forward, eye),
        1.0,
    ]
}

pub fn mul(a: Mat4, b: Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[column * 4 + k];
            }
            out[column * 4 + row] = sum;
        }
    }
    out
}

pub fn transform_point(matrix: Mat4, point: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for row in 0..4 {
        out[row] = (0..4)
            .map(|column| matrix[column * 4 + row] * point[column])
            .sum();
    }
    out
}

fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [qx, qy, qz, qw] = q;
    let uv = [
        qy * v[2] - qz * v[1],
        qz * v[0] - qx * v[2],
        qx * v[1] - qy * v[0],
    ];
    let uuv = [
        qy * uv[2] - qz * uv[1],
        qz * uv[0] - qx * uv[2],
        qx * uv[1] - qy * uv[0],
    ];
    [
        v[0] + 2.0 * (qw * uv[0] + uuv[0]),
        v[1] + 2.0 * (qw * uv[1] + uuv[1]),
        v[2] + 2.0 * (qw * uv[2] + uuv[2]),
    ]
}

pub(crate) fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn length(v: [f32; 3]) -> f32 {
    dot(v, v).sqrt()
}

/// 零向量返回零向量（调用方各自决定退化兜底），不做 NaN。
pub(crate) fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = length(v);
    if len <= f32::EPSILON {
        [0.0, 0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}
