# Hardened BliKVM Web 界面

该 React/TypeScript 界面正在从 `UPSTREAM.md` 中固定的 Hardened NanoKVM
基线迁移到 BliKVM。仅当 Rust 后端明确公开对应 capability 时才显示控制项；
硬件探测结果本身不会启用任何操作。

构建方法和安全约束请参阅 [`README.md`](README.md)。在平台适配器、恢复方案和
真机测试通过审核前，请勿把此开发构建部署到设备上。
