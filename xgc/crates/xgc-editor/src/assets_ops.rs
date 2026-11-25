//! 项目资产清单：只读扫描清单内容目录，不解析模型 / 贴图，也不生成 `.meta`。
//! UI 缓存扫描结果，只在首次展开或用户刷新时重新访问文件系统。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use xgc_base::path::normalize_project_relative;

use super::EditorState;

pub(crate) const MAX_ASSETS: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    Model,
    Texture,
    Audio,
    Prefab,
    Script,
    Other,
}

impl AssetKind {
    fn from_path(path: &str) -> Self {
        let path = path.to_ascii_lowercase();
        if path.ends_with(".prefab.ron") {
            Self::Prefab
        } else if path.ends_with(".glb") || path.ends_with(".gltf") {
            Self::Model
        } else if path.ends_with(".png") || path.ends_with(".jpg") || path.ends_with(".jpeg") {
            Self::Texture
        } else if path.ends_with(".wav") || path.ends_with(".ogg") {
            Self::Audio
        } else if path.ends_with(".lua") {
            Self::Script
        } else {
            Self::Other
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Texture => "texture",
            Self::Audio => "audio",
            Self::Prefab => "prefab",
            Self::Script => "script",
            Self::Other => "file",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetEntry {
    /// 项目相对路径，正斜杠。
    pub path: String,
    pub kind: AssetKind,
}

impl EditorState {
    /// 扫描当前项目的内容目录；单场景 / 无项目时为空。结果去重、排序，最多 2000 条。
    pub fn assets(&self) -> Vec<AssetEntry> {
        let Some(project) = &self.project else {
            return Vec::new();
        };
        let content = &project.manifest().content;
        let dirs: BTreeSet<_> = [
            Some(content.models.as_str()),
            Some(content.textures.as_str()),
            Some(content.audio.as_str()),
            Some(content.prefabs.as_str()),
            Some(content.scripts.as_str()),
            content.shaders.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|dir| normalize_project_relative(dir).ok())
        .collect();
        let mut entries = Vec::new();
        let mut visited = BTreeSet::new();
        for dir in dirs {
            if entries.len() >= MAX_ASSETS {
                break;
            }
            // 内容根也可能指向链接，不能只在递归子目录时跳过。
            if plain_directory(project.root(), &dir) {
                walk(project.root(), Path::new(&dir), &mut visited, &mut entries);
            }
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries
    }
}

fn plain_directory(root: &Path, relative: &str) -> bool {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        if part.starts_with('.') {
            return false;
        }
        path.push(part);
        let Ok(meta) = path.symlink_metadata() else {
            return false;
        };
        if !meta.is_dir() || is_hidden_or_link(&meta) {
            return false;
        }
    }
    true
}

fn is_hidden_or_link(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // junction 会把扫描引到项目外；Windows 隐藏项也不一定有点号前缀。
        const HIDDEN_OR_REPARSE: u32 = 0x2 | 0x400;
        if meta.file_attributes() & HIDDEN_OR_REPARSE != 0 {
            return true;
        }
    }
    false
}

fn walk(root: &Path, dir: &Path, visited: &mut BTreeSet<PathBuf>, out: &mut Vec<AssetEntry>) {
    let absolute = root.join(dir);
    let Ok(identity) = absolute.canonicalize() else {
        return;
    };
    if !visited.insert(identity) {
        return;
    }
    let Ok(read) = std::fs::read_dir(absolute) else {
        return;
    };
    // 先排序再截断，否则达到上限时，每次看到的资产取决于文件系统枚举顺序。
    let mut children: Vec<_> = read.flatten().collect();
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        if out.len() >= MAX_ASSETS {
            return;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') || name.to_ascii_lowercase().ends_with(".meta") {
            continue;
        }
        // 枚举到的名字不能被规范化成另一个文件；Unix 允许的反斜杠也不能当分隔符。
        if normalize_project_relative(name).ok().as_deref() != Some(name) {
            continue;
        }
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if is_hidden_or_link(&meta) {
            continue;
        }
        let path = dir.join(name);
        if meta.is_dir() {
            walk(root, &path, visited, out);
        } else if meta.is_file() {
            let relative = path.to_string_lossy().replace('\\', "/");
            out.push(AssetEntry {
                kind: AssetKind::from_path(&relative),
                path: relative,
            });
        }
    }
}
