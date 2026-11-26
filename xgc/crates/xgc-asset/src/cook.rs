//! 资产烹饪：把运行时需要的源资产复制到输出目录，并写确定性 manifest。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use xgc_base::path::normalize_project_relative;

use crate::{catalog_from_metas, find_files_in, is_meta, AssetCatalog};

const COOK_VERSION: u32 = 2;
const MANIFEST: &str = "cook.json";
const EXTENSIONS: &[&str] = &[
    "glb", "gltf", "bin", "wav", "lua", "png", "ppm", "ron", "meta", "ttf", "mat",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CookedFile {
    pub path: String,
    pub size: u64,
    pub hash: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookRuntime {
    pub search_paths: Vec<String>,
    pub input: Option<String>,
}

impl CookRuntime {
    /// 单世界没有项目清单，沿用根目录内的可选输入映射。
    pub fn standalone(root: &Path) -> Self {
        Self {
            search_paths: Vec::new(),
            input: root.join("input.ron").is_file().then(|| "input.ron".into()),
        }
    }

    fn normalized(mut self) -> Result<Self, String> {
        let normalize = |field: &str, path: &str| {
            normalize_project_relative(path)
                .map_err(|err| format!("cook.json 的 {field} 路径 {path} 不合法：{err}"))
        };
        for path in &mut self.search_paths {
            *path = normalize("search_paths", path)?;
        }
        if let Some(path) = &mut self.input {
            *path = normalize("input", path)?;
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CookManifest {
    pub version: u32,
    pub files: Vec<CookedFile>,
    #[serde(default)]
    pub runtime: CookRuntime,
    pub signature: String,
}

/// 复制资产并返回按路径排序的 manifest；输出目录里上一次 manifest 的文件先清理。
pub fn cook_project(root: &Path, out: &Path) -> Result<CookManifest, String> {
    cook_project_filtered(root, out, None)
}

/// `keep = None` 复制全部支持的源；`Some` 时从指定项目相对路径展开依赖。
pub fn cook_project_filtered(
    root: &Path,
    out: &Path,
    keep: Option<&BTreeSet<String>>,
) -> Result<CookManifest, String> {
    cook_project_with_runtime(root, out, CookRuntime::standalone(root), keep)
}

pub fn cook_project_with_runtime(
    root: &Path,
    out: &Path,
    runtime: CookRuntime,
    keep: Option<&BTreeSet<String>>,
) -> Result<CookManifest, String> {
    cook(root, out, runtime, |_, sources| {
        dependency_closure(sources, keep, |_| BTreeSet::new())
    })
}

/// 两个回调返回世界 / 预制体的项目相对引用，源目录与 GUID 目录共用同一文件表。
pub fn cook_project_with_dependencies(
    root: &Path,
    out: &Path,
    runtime: CookRuntime,
    world_dependencies: impl FnOnce(&Path, &AssetCatalog) -> BTreeSet<String>,
    mut prefab_dependencies: impl FnMut(&Path, &AssetCatalog, &Path) -> BTreeSet<String>,
) -> Result<CookManifest, String> {
    cook(root, out, runtime, |source_root, sources| {
        let mut metas: Vec<_> = sources
            .values()
            .map(PathBuf::as_path)
            .filter(|path| is_meta(path))
            .collect();
        // 重复 GUID 沿用资产目录的路径首占规则。
        metas.sort_unstable();
        let catalog = catalog_from_metas(metas);
        let keep = world_dependencies(source_root, &catalog);
        dependency_closure(sources, Some(&keep), |path| {
            prefab_dependencies(source_root, &catalog, path)
        })
    })
}

fn cook(
    root: &Path,
    out: &Path,
    runtime: CookRuntime,
    select: impl FnOnce(&Path, &BTreeMap<String, PathBuf>) -> BTreeSet<String>,
) -> Result<CookManifest, String> {
    let runtime = runtime.normalized()?;
    if !root.is_dir() {
        return Err(format!("项目根不是目录：{}", root.display()));
    }
    fs::create_dir_all(out).map_err(|err| format!("创建输出目录失败：{err}"))?;
    let source_root = root
        .canonicalize()
        .map_err(|err| format!("定位项目根失败：{err}"))?;
    let output_root = out
        .canonicalize()
        .map_err(|err| format!("定位输出目录失败：{err}"))?;
    if source_root == output_root {
        return Err("输出目录不能是项目根目录".into());
    }
    let sources = collect_sources(&source_root, &output_root)?;
    let mut files = select(&source_root, &sources);
    if let Some(input) = &runtime.input {
        if !sources.contains_key(input) {
            return Err(format!("缺少项目输入映射：{input}"));
        }
        files.insert(input.clone());
        let meta = format!("{input}.meta");
        if sources.contains_key(&meta) {
            files.insert(meta);
        }
    }
    let previous = read_manifest(out)
        .map(|manifest| {
            manifest
                .files
                .into_iter()
                .filter_map(|file| {
                    let path = normalize_project_relative(&file.path).ok()?;
                    Some((path, file.hash))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut manifest = CookManifest {
        version: COOK_VERSION,
        files: Vec::with_capacity(files.len()),
        runtime,
        signature: String::new(),
    };
    for (relative, source) in sources.into_iter().filter(|(path, _)| files.contains(path)) {
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
        if !files.contains(stale) {
            let _ = fs::remove_file(out.join(stale));
        }
    }
    write_manifest(out, &mut manifest)?;
    Ok(manifest)
}

fn collect_sources(root: &Path, out: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let paths = find_files_in(
        root,
        |directory| {
            directory != out
                && !matches!(
                    directory.file_name().and_then(|name| name.to_str()),
                    Some(".git" | "target" | ".autosave" | "dist")
                )
        },
        |_| true,
    )
    .map_err(|err| format!("读取 {} 失败：{err}", root.display()))?;
    Ok(paths
        .into_iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(root).ok()?.to_str()?;
            let key = normalize_project_relative(relative).ok()?;
            // 打包生成的顶层文件不能被外链重新解释为源资产。
            if matches!(key.as_str(), MANIFEST | "world.json" | "world.xgcb") {
                return None;
            }
            Some((key, path))
        })
        .collect())
}

fn is_source_asset(path: &str) -> bool {
    path.ends_with(".prefab.json")
        || Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

fn dependency_closure(
    sources: &BTreeMap<String, PathBuf>,
    keep: Option<&BTreeSet<String>>,
    mut prefab_dependencies: impl FnMut(&Path) -> BTreeSet<String>,
) -> BTreeSet<String> {
    let mut pending: Vec<String> = sources
        .keys()
        .filter(|path| is_source_asset(path) && keep.is_none_or(|keep| keep.contains(*path)))
        .cloned()
        .collect();
    let mut files = BTreeSet::new();
    while let Some(path) = pending.pop() {
        let Some(source) = sources.get(&path) else {
            // 源工程已有的悬空引用由 check 处理。
            continue;
        };
        if !files.insert(path.clone()) {
            continue;
        }
        let meta = format!("{path}.meta");
        if sources.contains_key(&meta) {
            files.insert(meta);
        }
        if path.ends_with(".prefab.json") {
            pending.extend(
                prefab_dependencies(source)
                    .into_iter()
                    .filter(|path| is_source_asset(path)),
            );
        } else if source
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gltf"))
        {
            if let Ok(text) = fs::read_to_string(source) {
                pending.extend(gltf_dependencies(&path, &text));
            }
        }
    }
    files
}

#[derive(Deserialize)]
struct GltfDependencies {
    #[serde(default)]
    buffers: Vec<GltfResource>,
    #[serde(default)]
    images: Vec<GltfResource>,
}

#[derive(Deserialize)]
struct GltfResource {
    uri: Option<String>,
}

fn gltf_dependencies(model: &str, text: &str) -> Vec<String> {
    let Ok(document) = serde_json::from_str::<GltfDependencies>(text) else {
        return Vec::new();
    };
    document
        .buffers
        .into_iter()
        .chain(document.images)
        .filter_map(|resource| external_path(model, resource.uri.as_deref()?))
        .collect()
}

fn external_path(model: &str, uri: &str) -> Option<String> {
    let uri = uri.strip_prefix("file://").unwrap_or(uri);
    if uri.starts_with(['/', '\\']) || uri.contains(':') {
        return None;
    }
    let mut parts: Vec<_> = model.split('/').collect();
    parts.pop();
    // 外链相对模型目录；允许同项目的兄弟目录，不能越过项目根。
    for part in uri.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn read_manifest(out: &Path) -> Option<CookManifest> {
    let text = fs::read_to_string(out.join(MANIFEST)).ok()?;
    serde_json::from_str(&text).ok()
}

fn signature(manifest: &CookManifest) -> String {
    // 搜索根换序会改变读取结果，也必须进入确定性签名。
    let data = serde_json::to_vec(&(manifest.version, &manifest.runtime, &manifest.files))
        .expect("清单只包含可序列化的整数与字符串");
    fnv_hex(&data)
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

pub fn write_manifest(out: &Path, manifest: &mut CookManifest) -> Result<(), String> {
    manifest.signature = signature(manifest);
    let text = serde_json::to_string_pretty(manifest)
        .map_err(|err| format!("manifest 序列化失败：{err}"))?;
    fs::write(out.join(MANIFEST), text).map_err(|err| format!("写入 manifest 失败：{err}"))
}

/// 无清单的单世界返回 `None`。存在清单时，先校验设置和文件，再交给宿主。
pub fn verify_manifest(out: &Path) -> Result<Option<CookManifest>, Vec<String>> {
    let text = match fs::read_to_string(out.join(MANIFEST)) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(vec![format!("读取 cook.json 失败：{err}")]),
    };
    let manifest: CookManifest =
        serde_json::from_str(&text).map_err(|err| vec![format!("cook.json 解析失败：{err}")])?;
    if manifest.version != COOK_VERSION {
        return Err(vec![format!(
            "cook.json 版本 {} 不受支持，当前版本为 {COOK_VERSION}，请重新打包",
            manifest.version
        )]);
    }
    if manifest.signature != signature(&manifest) {
        return Err(vec!["manifest 签名不匹配".to_string()]);
    }
    let normalized = manifest
        .runtime
        .clone()
        .normalized()
        .map_err(|err| vec![err])?;
    if normalized != manifest.runtime {
        return Err(vec!["cook.json 的运行配置路径未规范化".to_string()]);
    }
    let mut differences = Vec::new();
    if let Some(input) = &manifest.runtime.input {
        if !manifest.files.iter().any(|file| &file.path == input) {
            differences.push(format!("manifest 未登记项目输入映射：{input}"));
        }
    }
    for file in &manifest.files {
        if normalize_project_relative(&file.path).as_deref() != Ok(file.path.as_str()) {
            differences.push(format!("manifest 文件路径不合法：{}", file.path));
            continue;
        }
        match fs::read(out.join(&file.path)) {
            Ok(bytes) if bytes.len() as u64 == file.size && fnv_hex(&bytes) == file.hash => {}
            Ok(_) => differences.push(format!("文件内容变化：{}", file.path)),
            Err(_) => differences.push(format!("缺少文件：{}", file.path)),
        }
    }
    if differences.is_empty() {
        Ok(Some(manifest))
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
