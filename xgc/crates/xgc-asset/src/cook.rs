//! 资产烹饪：把运行时需要的源资产复制到输出目录，并写确定性 manifest。

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
}

/// 复制资产并返回按路径排序的 manifest；输出目录里上一次 manifest 的文件先清理。
pub fn cook_project(root: &Path, out: &Path) -> Result<CookManifest, String> {
    if !root.is_dir() {
        return Err(format!("项目根不是目录：{}", root.display()));
    }
    fs::create_dir_all(out).map_err(|err| format!("创建输出目录失败：{err}"))?;
    clean_previous(out)?;
    let mut files = Vec::new();
    collect(root, root, out, &mut files)?;
    files.sort_by(|a, b| a.1.cmp(&b.1));
    let mut manifest = CookManifest {
        version: COOK_VERSION,
        files: Vec::with_capacity(files.len()),
    };
    for (source, relative) in files {
        let destination = out.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("创建目录失败：{err}"))?;
        }
        let bytes =
            fs::read(&source).map_err(|err| format!("读取 {} 失败：{err}", source.display()))?;
        fs::write(&destination, &bytes)
            .map_err(|err| format!("写入 {} 失败：{err}", destination.display()))?;
        manifest.files.push(CookedFile {
            path: relative,
            size: bytes.len() as u64,
            hash: fnv_hex(&bytes),
        });
    }
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|err| format!("manifest 序列化失败：{err}"))?;
    fs::write(out.join(MANIFEST), text).map_err(|err| format!("写入 manifest 失败：{err}"))?;
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

fn clean_previous(out: &Path) -> Result<(), String> {
    let path = out.join(MANIFEST);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let Ok(manifest) = serde_json::from_str::<CookManifest>(&text) else {
        return Ok(());
    };
    for file in manifest.files {
        let target = out.join(&file.path);
        if target.starts_with(out) {
            let _ = fs::remove_file(target);
        }
    }
    Ok(())
}

fn fnv_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
