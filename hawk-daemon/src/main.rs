//! 进程入口（薄壳）：只负责解析参数并分发到 OpenAPI 导出或 daemon 启动流程。
//! 组件组装与启动编排在 bootstrap；HTTP 层在 api/，领域核心在 core/（库入口见 lib.rs）。

use hawk_daemon::{
    api::openapi,
    bootstrap,
    settings::{Cli, Command},
};

#[tokio::main]
async fn main() {
    match Cli::parse_args().command {
        Command::Local(args) => bootstrap::run_local(args).await,
        Command::Remote(args) => bootstrap::run_remote(args).await,
        Command::Openapi => openapi::dump(),
    }
}
