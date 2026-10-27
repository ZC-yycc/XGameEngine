use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

// 固定算法而非 DefaultHasher；只作构建差异门禁，不作签名或安全认证。
fn feed(hash: &mut [u64; 2], bytes: &[u8]) {
    for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
        hash[0] = (hash[0] ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        hash[1] = (hash[1] ^ u64::from(*byte)).wrapping_mul(0x100000001e7);
    }
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", path.display());
    for entry in fs::read_dir(path)
        .expect("读取引擎源码目录")
        .map(Result::unwrap)
    {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else {
            files.push(path);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.parent().unwrap().parent().unwrap();
    let rustc = env::var_os("RUSTC").expect("Cargo 提供 RUSTC");
    let output = Command::new(rustc)
        .arg("-vV")
        .output()
        .expect("读取 rustc 版本");
    assert!(output.status.success(), "rustc -vV 失败");
    let mut hash = [0xcbf29ce484222325, 0x84222325cbf29ce4];
    feed(&mut hash, b"xgame-build-v1");
    feed(&mut hash, &output.stdout);
    // Cargo 仅提供当前 xgame 单元的配置，不能覆盖下游插件全部 flags / feature 合并。
    let mut keys: Vec<String> = [
        "TARGET",
        "HOST",
        "PROFILE",
        "OPT_LEVEL",
        "DEBUG",
        "CARGO_CFG_PANIC",
        "CARGO_CFG_TARGET_FEATURE",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    keys.extend(
        env::vars()
            .map(|(key, _)| key)
            .filter(|key| key.starts_with("CARGO_FEATURE_")),
    );
    keys.sort();
    keys.dedup();
    // 第二份指纹：只回答"这是不是同一份引擎源码"。插件门禁要的是同一**构建单元**
    // （Rust ABI），而打包时判断播放器新不新鲜要的是同一份**源码** —— debug 编辑器
    // 配 release 播放器是开发期的正常组合。所以这里把 profile / 优化等级 / 调试信息
    // 三样排除，其余输入与构建指纹逐字一致。
    let mut source = [0xcbf29ce484222325, 0x84222325cbf29ce4];
    feed(&mut source, b"xgame-src-v1");
    feed(&mut source, &output.stdout);
    const PROFILE_ONLY_KEYS: &[&str] = &["PROFILE", "OPT_LEVEL", "DEBUG"];
    for key in keys {
        println!("cargo:rerun-if-env-changed={key}");
        let value = env::var(&key).unwrap_or_default();
        feed(&mut hash, key.as_bytes());
        feed(&mut hash, value.as_bytes());
        if !PROFILE_ONLY_KEYS.contains(&key.as_str()) {
            feed(&mut source, key.as_bytes());
            feed(&mut source, value.as_bytes());
        }
    }
    let mut files = vec![
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
        manifest.join("build.rs"),
    ];
    for entry in fs::read_dir(root.join("crates"))
        .unwrap()
        .map(Result::unwrap)
    {
        let dir = entry.path();
        if dir.join("src/lib.rs").is_file() {
            files.push(dir.join("Cargo.toml"));
            collect(&dir.join("src"), &mut files);
            if dir.join("build.rs").is_file() {
                files.push(dir.join("build.rs"));
            }
        }
    }
    files.sort();
    files.dedup();
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
        // 文件标识只取仓库相对路径；最终描述符只含版本前缀与十六进制摘要。
        let relative = file
            .strip_prefix(root)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/");
        let contents = fs::read(&file).expect("读取指纹输入");
        feed(&mut hash, relative.as_bytes());
        feed(&mut hash, &contents);
        feed(&mut source, relative.as_bytes());
        feed(&mut source, &contents);
    }
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let fingerprint = format!("xgame-build-v1:{:016x}{:016x}", hash[0], hash[1]);
    fs::write(out_dir.join("plugin-fingerprint.txt"), fingerprint).expect("写入构建指纹");
    let source_fingerprint = format!("xgame-src-v1:{:016x}{:016x}", source[0], source[1]);
    fs::write(out_dir.join("source-fingerprint.txt"), source_fingerprint).expect("写入源码指纹");
}
