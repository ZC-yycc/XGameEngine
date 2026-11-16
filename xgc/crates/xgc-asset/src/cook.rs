//! 资产烹饪：把运行时需要的源资产复制到输出目录，并写确定性 manifest。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const COOK_VERSION: u32 = 1;
const MANIFEST: &str = "cook.json";
const EXTENSIONS: &[&str] = &[
    "glb", "gltf", "bin", "wav", "lua", "png", "ppm", "ron", "meta", "ttf",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CookedFile {
    pub path: String,
    pub size: u64,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CookManifest {
    pub version: u32,
    pub files: Vec<CookedFile>,
    #[serde(default)]
    pub signature: String,
}

/// 复制资产并返回按路径排序的 manifest；输出目录里上一次 manifest 的文件先清理。
pub fn cook_project(root: &Path, out: &Path) -> Result<CookManifest, String> {
    cook_project_filtered(root, out, None)
}

/// `keep = None` 复制全部支持的源；`Some` 时只复制引用资产及其 `.meta` / `.bin`。
pub fn cook_project_filtered(
    root: &Path,
    out: &Path,
    keep: Option<&BTreeSet<String>>,
) -> Result<CookManifest, String> {
    if !root.is_dir() {
        return Err(format!("项目根不是目录：{}", root.display()));
    }
    fs::create_dir_all(out).map_err(|err| format!("创建输出目录失败：{err}"))?;
    let previous = read_manifest(out)
        .map(|manifest| {
            manifest
                .files
                .into_iter()
                .map(|file| (file.path, file.hash))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut files = Vec::new();
    collect(root, root, out, &mut files)?;
    if let Some(keep) = keep {
        let mut expanded = keep.clone();
        for path in keep {
            if path.ends_with(".gltf") {
                expanded.insert(path.trim_end_matches(".gltf").to_string() + ".bin");
            }
        }
        let metas: Vec<String> = expanded.iter().map(|path| format!("{path}.meta")).collect();
        expanded.extend(metas);
        files.retain(|(_, relative)| {
            relative == "input.ron"
                || expanded.contains(relative)
                || (relative.ends_with(".meta")
                    && expanded.contains(relative.trim_end_matches(".meta")))
        });
        let _ = &expanded;
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    let mut manifest = CookManifest {
        version: COOK_VERSION,
        files: Vec::with_capacity(files.len()),
        signature: String::new(),
    };
    for (source, relative) in files {
        let destination = out.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("创建目录失败：{err}"))?;
        }
        let bytes =
            fs::read(&source).map_err(|err| format!("读取 {} 失败：{err}", source.display()))?;
        let hash = fnv_hex(&bytes);
        let reusable = previous.get(&relative) == Some(&hash) && destination.is_file();
        if !reusable {
            fs::write(&destination, &bytes)
                .map_err(|err| format!("写入 {} 失败：{err}", destination.display()))?;
        }
        manifest.files.push(CookedFile {
            path: relative,
            size: bytes.len() as u64,
            hash,
        });
    }
    for stale in previous.keys() {
        if !manifest.files.iter().any(|file| file.path == *stale) {
            let _ = fs::remove_file(out.join(stale));
        }
    }
    write_manifest(out, &manifest)?;
    Ok(manifest)
}

fn collect(
    root: &Path,
    dir: &Path,
    out: &Path,
    files: &mut Vec<(PathBuf, String)>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("读取 {} 失败：{err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if path == out {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if matches!(name.as_str(), ".git" | "target" | ".autosave" | "dist") {
                continue;
            }
            collect(root, &path, out, files)?;
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !EXTENSIONS.contains(&extension.as_str()) && name != "input.ron" {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|err| err.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        files.push((path, relative));
    }
    Ok(())
}

fn read_manifest(out: &Path) -> Option<CookManifest> {
    let text = fs::read_to_string(out.join(MANIFEST)).ok()?;
    serde_json::from_str(&text).ok()
}

fn signature(files: &[CookedFile]) -> String {
    let mut state = 0xcbf2_9ce4_8422_2325u64;
    for file in files {
        for byte in file.path.as_bytes().iter().chain(file.hash.as_bytes()) {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{state:016x}")
}

pub fn file_entry(out: &Path, relative: &str) -> Result<CookedFile, String> {
    let path = out.join(relative);
    let bytes = fs::read(&path).map_err(|err| format!("读取 {} 失败：{err}", path.display()))?;
    Ok(CookedFile {
        path: relative.replace('\\', "/"),
        size: bytes.len() as u64,
        hash: fnv_hex(&bytes),
    })
}

pub fn write_manifest(out: &Path, manifest: &CookManifest) -> Result<(), String> {
    let mut manifest = manifest.clone();
    manifest.signature = signature(&manifest.files);
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|err| format!("manifest 序列化失败：{err}"))?;
    fs::write(out.join(MANIFEST), text).map_err(|err| format!("写入 manifest 失败：{err}"))
}

/// 校验 manifest 中每个文件存在且大小 / 哈希一致；没有 manifest 时视为无约束。
pub fn verify_manifest(out: &Path) -> Result<(), Vec<String>> {
    let path = out.join(MANIFEST);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let manifest: CookManifest = match serde_json::from_str(&text) {
        Ok(manifest) => manifest,
        Err(err) => return Err(vec![format!("cook.json 解析失败：{err}")]),
    };
    let mut differences = Vec::new();
    if !manifest.signature.is_empty() && manifest.signature != signature(&manifest.files) {
        differences.push("manifest 签名不匹配".to_string());
    }
    for file in manifest.files {
        let path = out.join(&file.path);
        match fs::read(&path) {
            Ok(bytes) if bytes.len() as u64 == file.size && fnv_hex(&bytes) == file.hash => {}
            Ok(_) => differences.push(format!("文件内容变化：{}", file.path)),
            Err(_) => differences.push(format!("缺少文件：{}", file.path)),
        }
    }
    if differences.is_empty() {
        Ok(())
    } else {
        Err(differences)
    }
}

fn fnv_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
