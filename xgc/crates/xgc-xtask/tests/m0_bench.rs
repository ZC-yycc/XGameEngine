//! M0 全量 schema 导出。用 Release 运行忽略项：`cargo test -p xgc-xtask --release --test m0_bench -- --ignored --nocapture --test-threads=1`。

use std::hint::black_box;
use std::time::{Duration, Instant};

use xgc_world::builtin_registry;

#[test]
#[ignore = "M0 计时需使用 Release 并串行运行"]
fn schema_export() {
    const SAMPLES: usize = 9;
    let expected_types = builtin_registry().len();
    assert!(expected_types > 0);

    // 与 CLI 导出的内容路径相同；计时不含进程启动、stdout 和测试解析。
    let export = || serde_json::to_string_pretty(&builtin_registry().schema()).unwrap();
    for _ in 0..2 {
        drop(black_box(export()));
    }

    let mut elapsed = [Duration::ZERO; SAMPLES];
    let mut bytes = 0;
    for duration in &mut elapsed {
        let start = Instant::now();
        let text = export();
        *duration = start.elapsed();
        let document: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(document["engine"], "xgc");
        let types = document["types"].as_array().unwrap();
        assert_eq!(types.len(), expected_types);
        assert!(types.iter().any(|component| {
            component["name"] == "core.transform"
                && component["fields"]
                    .as_array()
                    .is_some_and(|fields| fields.iter().any(|field| field["name"] == "position"))
        }));
        bytes = text.len();
    }
    elapsed.sort_unstable();
    eprintln!(
        "M0 schema_export types={expected_types} bytes={bytes} median={:?} min={:?} max={:?} samples={elapsed:?}",
        elapsed[SAMPLES / 2],
        elapsed[0],
        elapsed[SAMPLES - 1],
    );
}
