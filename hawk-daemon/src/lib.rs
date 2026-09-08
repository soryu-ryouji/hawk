//! hawk-daemon 库入口：模块树在此声明，`main.rs` 只是薄壳。
//! 拆出 lib 的目的：`tests/` 集成测试与外部工具（bench/fuzz/契约校验）可依赖库，
//! 不必通过 HTTP 绕行，也不必把测试塞进各模块的 `#[cfg(test)]`。
//! 依赖方向仍是 `api/` → `core/`（见 docs/backend/server-code-structure.md）。

pub mod api;
pub mod bootstrap;
pub mod core;
pub mod settings;
