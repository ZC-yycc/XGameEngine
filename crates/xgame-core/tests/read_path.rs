//! 运行时**读**路径的解析门禁（脚本 / 音频共用）。
//!
//! 这一组钉的是"运行时也挡得住越界读"这件事。此前只有 `--check` 会点名
//! 越界引用，但那只是工具 —— **不跑检查的宿主仍然会去读**：脚本里写
//! `../../secrets.lua`、音频 clip 写绝对路径，`root.join()` 会照单全收
//! （`Path::join` 遇绝对路径直接替换根）。
//!
//! 语义真相源：`resolve_read_path`（`crates/xgame-core/src/project.rs`）。

use std::path::{Path, PathBuf};

use xgame_core::project::ReadPathError;
use xgame_core::{resolve_read_path, ContentConfig, PathDefect, Project};

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgame_readpath_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn write_file(root: &Path, relative: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("建父目录");
    }
    std::fs::write(path, b"x").expect("写文件");
}

/// 越界 / Windows 不认的写法一律 `Invalid`，**不落盘读**。
#[test]
fn escapes_and_illegal_spellings_are_invalid() {
    let root = temp_root("invalid");
    std::fs::create_dir_all(&root).unwrap();

    let cases: &[(&str, PathDefect)] = &[
        ("../outside.lua", PathDefect::ParentEscape),
        ("Scripts/../../outside.lua", PathDefect::ParentEscape),
        ("/etc/passwd", PathDefect::Absolute),
        ("C:/Windows/win.ini", PathDefect::Absolute),
        (
            "Scripts/a:1.lua",
            PathDefect::Colon {
                part: "a:1.lua".into(),
            },
        ),
        (
            "Scripts/CON.lua",
            PathDefect::ReservedName {
                part: "CON.lua".into(),
            },
        ),
    ];
    for (raw, want) in cases {
        match resolve_read_path(&root, raw, &[]) {
            Err(ReadPathError::Invalid(defect)) => {
                assert_eq!(&defect, want, "{raw} 的拒绝原因")
            }
            other => panic!("{raw} 必须被拒绝，实际: {other:?}"),
        }
    }
    std::fs::remove_dir_all(&root).unwrap();
}

/// 直接命中：返回 `根/<规范化路径>`，开头的 `./` 被吃掉。
#[test]
fn direct_hit_normalizes_prefix() {
    let root = temp_root("direct");
    write_file(&root, "Scripts/Player.lua");

    let path = resolve_read_path(&root, "./Scripts/Player.lua", &[]).expect("应当找到");
    assert_eq!(path, root.join("Scripts/Player.lua"));

    // 反斜杠写法等价（Windows 手写 / 复制来的路径很常见）。
    let path = resolve_read_path(&root, "Scripts\\Player.lua", &[]).expect("应当找到");
    assert_eq!(path, root.join("Scripts/Player.lua"));
    std::fs::remove_dir_all(&root).unwrap();
}

/// `search_paths` 是**有序兜底**：直接路径优先，其次按列表顺序第一个命中的胜出。
#[test]
fn search_paths_are_ordered_fallback() {
    let root = temp_root("search");
    write_file(&root, "Mods/A/shared.lua");
    write_file(&root, "Mods/B/shared.lua");
    let search = vec!["Mods/A".to_string(), "Mods/B".to_string()];

    let path = resolve_read_path(&root, "shared.lua", &search).expect("兜底命中");
    assert_eq!(path, root.join("Mods/A/shared.lua"), "先配的目录优先");

    // 直接路径命中时**不看** search_paths（否则同名的两份会随机漂移）。
    write_file(&root, "shared.lua");
    let path = resolve_read_path(&root, "shared.lua", &search).expect("直接命中");
    assert_eq!(path, root.join("shared.lua"));
    std::fs::remove_dir_all(&root).unwrap();
}

/// 不合规的 `search_paths` 条目**跳过**，绝不因此读到项目外。
#[test]
fn invalid_search_entry_cannot_widen_the_search() {
    let root = temp_root("bad_search");
    std::fs::create_dir_all(&root).unwrap();
    // 在项目**外**放一份同名文件：越界条目不能把搜索范围扩到它。
    let outside = root.parent().unwrap().join(format!(
        "xgame_readpath_outside_{}_{}.lua",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&outside, b"x").unwrap();

    let search = vec!["..".to_string(), "C:/Windows".to_string(), "".to_string()];
    match resolve_read_path(&root, "outside.lua", &search) {
        Err(ReadPathError::Missing { tried, .. }) => {
            assert_eq!(tried.len(), 1, "不合规条目不应进入候选列表: {tried:?}");
        }
        other => panic!("越界条目不得命中，实际: {other:?}"),
    }
    std::fs::remove_file(&outside).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

/// 找不到时报出**试过哪些**，否则作者拿着"文件不存在"无从下手。
#[test]
fn missing_lists_every_candidate() {
    let root = temp_root("missing");
    write_file(&root, "Mods/A/keep.lua");
    let search = vec!["Mods/A".to_string()];

    match resolve_read_path(&root, "Scripts/Nope.lua", &search) {
        Err(ReadPathError::Missing { relative, tried }) => {
            assert_eq!(relative, "Scripts/Nope.lua");
            assert_eq!(tried, vec!["Scripts/Nope.lua", "Mods/A/Scripts/Nope.lua"]);
        }
        other => panic!("应当报缺失，实际: {other:?}"),
    }
    std::fs::remove_dir_all(&root).unwrap();
}

/// `--check` 的目录体检要覆盖 `search_paths` 条目（运行时是跳过它们）。
#[test]
fn content_defects_include_search_paths() {
    let project = Project {
        content: ContentConfig {
            search_paths: vec!["Mods/Shared".into(), "../escape".into(), "".into()],
            ..ContentConfig::default()
        },
        ..Project::default()
    };
    let defects = project.content_path_defects();
    assert_eq!(defects.len(), 2, "两条坏条目各报一次: {defects:?}");
    assert!(
        defects[0].contains("content.search_paths[1]") && defects[0].contains("`..`"),
        "{defects:?}"
    );
    assert!(
        defects[1].contains("content.search_paths[2]") && defects[1].contains("empty path"),
        "{defects:?}"
    );
    // 报告必须是纯 ASCII（GBK 控制台与 CI 文本匹配都靠它）。
    assert!(defects.iter().all(|d| d.is_ascii()), "{defects:?}");
}
