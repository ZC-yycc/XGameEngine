//! 输出设备枚举：无默认设备也应返回可诊断结果，不 panic。

use xgc_audio::output_devices;

#[test]
fn enumerates_output_devices_without_panicking() {
    let devices = output_devices().expect("枚举不应失败");
    eprintln!("输出设备：{devices:?}");
}
