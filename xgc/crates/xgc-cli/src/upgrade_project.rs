//! 原生清单 v1 的一次性离线升级。日常项目打开只认当前格式。

use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;
use xgc_base::layer::CollisionLayers;
use xgc_base::render_settings::{RenderQuality, RenderSettings};
use xgc_migrate::ProjectCheckReport;
use xgc_reflect::Registry;
use xgc_store::{
    save_project, ProjectContent, ProjectManifest, ProjectSession, PROJECT_FORMAT_VERSION,
};

use crate::check_cmd::project::check_session;

const MANIFEST: &str = "project.xgc";
const PROJECT_LOCK: &str = ".xgc-project.lock";

#[derive(Deserialize)]
struct ManifestV1 {
    format_version: u32,
    name: String,
    entry_scene: String,
    scenes: Vec<String>,
    content: ProjectContent,
    #[serde(default)]
    search_paths: Vec<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    render: RenderSettings,
    #[serde(default)]
    quality: RenderQuality,
    #[serde(default)]
    layers: CollisionLayers,
}

impl ManifestV1 {
    fn into_candidate(self) -> Result<ProjectManifest, UpgradeError> {
        if self.format_version != 1 {
            return Err(UpgradeError::Invalid(format!(
                "原生项目离线升级只接受 v1，输入为 v{}",
                self.format_version
            )));
        }
        Ok(ProjectManifest {
            format_version: PROJECT_FORMAT_VERSION,
            // 体检计算实际水位时还没有项目上界，只报告原输入已有的问题。
            next_entity_id: u64::MAX,
            name: self.name,
            entry_scene: self.entry_scene,
            scenes: self.scenes,
            content: self.content,
            search_paths: self.search_paths,
            input: self.input,
            render: self.render,
            quality: self.quality,
            layers: self.layers,
        })
    }
}

#[derive(Debug)]
enum UpgradeError {
    Invalid(String),
    Failed(String),
    Problems(Vec<String>),
}

pub(crate) fn upgrade(root: &Path, output: &Path, dry_run: bool) -> ExitCode {
    let (report, next_entity_id) = match upgrade_project(root, output, dry_run) {
        Ok(result) => result,
        Err(UpgradeError::Invalid(err)) => {
            eprintln!("项目升级失败：{err}");
            return ExitCode::from(2);
        }
        Err(UpgradeError::Failed(err)) => {
            eprintln!("项目升级失败：{err}");
            return ExitCode::from(1);
        }
        Err(UpgradeError::Problems(problems)) => {
            eprintln!("项目体检未通过（{} 个问题），未开始升级：", problems.len());
            for problem in problems {
                eprintln!("  {problem}");
            }
            return ExitCode::from(2);
        }
    };
    let suffix = if dry_run {
        "（dry-run，未写盘）"
    } else {
        ""
    };
    println!(
        "migrate: project={} scenes={} prefabs={} next_entity_id={} -> {}{suffix}",
        report.name,
        report.scenes,
        report.prefabs,
        next_entity_id,
        output.display(),
    );
    ExitCode::SUCCESS
}

fn upgrade_project(
    root: &Path,
    output: &Path,
    dry_run: bool,
) -> Result<(ProjectCheckReport, u64), UpgradeError> {
    let root_meta = fs::symlink_metadata(root).map_err(|err| io_error(root, err))?;
    if !root_meta.is_dir() || is_indirection(&root_meta) {
        return Err(UpgradeError::Invalid("源项目必须是普通目录".into()));
    }
    let root = root.canonicalize().map_err(|err| io_error(root, err))?;
    let output = new_output(output)?;
    if output.starts_with(&root) || root.starts_with(&output) {
        return Err(UpgradeError::Invalid("源项目与输出目录不能重叠".into()));
    }

    let manifest_path = root.join(MANIFEST);
    if !source_entry(&root, &manifest_path)?.is_file() {
        return Err(UpgradeError::Invalid("project.xgc 必须是普通文件".into()));
    }
    let original = fs::read(&manifest_path).map_err(|err| io_error(&manifest_path, err))?;
    let old: ManifestV1 = ron::de::from_bytes(&original)
        .map_err(|err| UpgradeError::Failed(format!("project.xgc 解析失败：{err}")))?;
    let mut candidate = old.into_candidate()?;
    let registry = xgc_world::builtin_registry();
    let project = ProjectSession::from_manifest(&manifest_path, candidate.clone())
        .map_err(|err| UpgradeError::Invalid(err.to_string()))?;
    // 把链接拒绝在读取引用前，避免在源与目标之间改变链接的相对含义。
    let entries = project_entries(&root)?;
    let audit = check_session(&project, &registry).map_err(UpgradeError::Failed)?;
    require_clean(&audit.report)?;
    candidate.next_entity_id = audit.next_entity_id;
    if !dry_run {
        publish(&root, &output, &entries, &candidate, &original, &registry)?;
    }
    Ok((audit.report, candidate.next_entity_id))
}

fn require_clean(report: &ProjectCheckReport) -> Result<(), UpgradeError> {
    if report.has_problems() {
        return Err(UpgradeError::Problems(report.problems.clone()));
    }
    Ok(())
}

fn io_error(path: &Path, err: io::Error) -> UpgradeError {
    UpgradeError::Failed(format!("{}：{err}", path.display()))
}

fn output_exists(path: &Path) -> UpgradeError {
    UpgradeError::Invalid(format!("输出目录已存在：{}", path.display()))
}

fn require_missing(path: &Path) -> Result<(), UpgradeError> {
    match fs::symlink_metadata(path) {
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io_error(path, err)),
        Ok(_) => Err(output_exists(path)),
    }
}

fn new_output(output: &Path) -> Result<PathBuf, UpgradeError> {
    require_missing(output)?;
    // 输出和中间目录都可能不存在。先解析最近的现有父目录，再拼回尚未创建的名字。
    let mut path = output;
    let mut missing = Vec::new();
    loop {
        match fs::metadata(path) {
            Ok(meta) if meta.is_dir() => break,
            Ok(_) => {
                return Err(UpgradeError::Invalid(format!(
                    "输出路径的上级不是目录：{}",
                    path.display()
                )));
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                let name = path.file_name().ok_or_else(|| {
                    UpgradeError::Invalid(format!("输出路径不合法：{}", output.display()))
                })?;
                missing.push(name);
                path = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
            }
            Err(err) => return Err(io_error(path, err)),
        }
    }
    let mut absolute = path.canonicalize().map_err(|err| io_error(path, err))?;
    absolute.extend(missing.into_iter().rev());
    require_missing(&absolute)?;
    Ok(absolute)
}

fn is_indirection(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Windows junction 与部分 reparse point 不会被 is_symlink 识别。
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.is_symlink()
}

fn source_entry(root: &Path, path: &Path) -> Result<Metadata, UpgradeError> {
    let meta = fs::symlink_metadata(path).map_err(|err| io_error(path, err))?;
    if is_indirection(&meta) || (!meta.is_file() && !meta.is_dir()) {
        return Err(UpgradeError::Invalid(format!(
            "源项目包含链接或特殊文件：{}",
            path.display()
        )));
    }
    let canonical = path.canonicalize().map_err(|err| io_error(path, err))?;
    if !canonical.starts_with(root) {
        return Err(UpgradeError::Invalid(format!(
            "源路径越出项目根目录：{}",
            path.display()
        )));
    }
    Ok(meta)
}

struct CopyEntry {
    relative: PathBuf,
    directory: bool,
}

fn is_generated_root_file(name: &OsStr) -> bool {
    [MANIFEST, PROJECT_LOCK].into_iter().any(|reserved| {
        let reserved = OsStr::new(reserved);
        if cfg!(windows) {
            name.eq_ignore_ascii_case(reserved)
        } else {
            name == reserved
        }
    })
}

fn project_entries(root: &Path) -> Result<Vec<CopyEntry>, UpgradeError> {
    let mut dirs = vec![PathBuf::new()];
    let mut entries = Vec::new();
    while let Some(dir) = dirs.pop() {
        let path = root.join(&dir);
        let listing = fs::read_dir(&path).map_err(|err| io_error(&path, err))?;
        for entry in listing {
            let entry = entry.map_err(|err| io_error(&path, err))?;
            let name = entry.file_name();
            if dir.as_os_str().is_empty() && is_generated_root_file(&name) {
                continue;
            }
            let relative = dir.join(name);
            let directory = source_entry(root, &entry.path())?.is_dir();
            if directory {
                dirs.push(relative.clone());
            }
            entries.push(CopyEntry {
                relative,
                directory,
            });
        }
    }
    entries.sort_unstable_by(|a, b| a.relative.cmp(&b.relative));
    Ok(entries)
}

fn publish(
    root: &Path,
    output: &Path,
    entries: &[CopyEntry],
    candidate: &ProjectManifest,
    original: &[u8],
    registry: &Registry,
) -> Result<(), UpgradeError> {
    let parent = output
        .parent()
        .ok_or_else(|| UpgradeError::Invalid("输出必须是新的项目目录".into()))?;
    fs::create_dir_all(parent).map_err(|err| io_error(parent, err))?;
    require_missing(output)?;
    let name = output.file_name().unwrap_or_else(|| OsStr::new("project"));
    // 同一输出的升级进程独占同名暂存目录，取得目录之前没有清理它的权限。
    let mut staging_name = std::ffi::OsString::from(".");
    staging_name.push(name);
    staging_name.push(".xgc-upgrade-staging");
    let staging = parent.join(staging_name);
    fs::create_dir(&staging).map_err(|err| io_error(&staging, err))?;
    let result =
        prepare_staging(root, &staging, entries, candidate, original, registry).and_then(|()| {
            require_missing(output)?;
            fs::rename(&staging, output).map_err(|err| io_error(output, err))
        });
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn prepare_staging(
    root: &Path,
    staging: &Path,
    entries: &[CopyEntry],
    candidate: &ProjectManifest,
    original: &[u8],
    registry: &Registry,
) -> Result<(), UpgradeError> {
    for entry in entries {
        let source = root.join(&entry.relative);
        let target = staging.join(&entry.relative);
        if source_entry(root, &source)?.is_dir() != entry.directory {
            return Err(UpgradeError::Invalid(format!(
                "源路径已变更：{}",
                source.display()
            )));
        }
        if entry.directory {
            fs::create_dir(&target).map_err(|err| io_error(&target, err))?;
        } else {
            fs::copy(&source, &target).map_err(|err| io_error(&source, err))?;
        }
    }
    let manifest_path = staging.join(MANIFEST);
    save_project(&manifest_path, candidate)
        .map_err(|err| UpgradeError::Failed(format!("{}：{err}", manifest_path.display())))?;
    // 核对将要发布的内容，避免源文件在体检与复制之间变更而留下过低的 ID 水位。
    let project = ProjectSession::open(&manifest_path)
        .map_err(|err| UpgradeError::Failed(err.to_string()))?;
    let audit = check_session(&project, registry).map_err(UpgradeError::Failed)?;
    require_clean(&audit.report)?;
    let source_manifest = root.join(MANIFEST);
    source_entry(root, &source_manifest)?;
    if fs::read(&source_manifest).map_err(|err| io_error(&source_manifest, err))? != original {
        return Err(UpgradeError::Invalid("源项目清单在升级期间发生变更".into()));
    }
    Ok(())
}
