# eframe 0.30 vendored

离线缓存缺 `windows-sys 0.60.2`（arboard 依赖），而 eframe 硬把 `egui-winit/clipboard`
打开。这里只改 eframe 的 Cargo.toml：去掉 clipboard 特性，保留 links；编辑器不需要
剪贴板。网络恢复后应删除本目录与 patch。