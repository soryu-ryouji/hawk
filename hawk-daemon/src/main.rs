//! 进程入口（薄壳）：参数解析（settings）、--dump-openapi 特殊模式分流、日志初始化。
//! 组件组装与启动编排在 bootstrap；HTTP 层在 api/，领域核心在 core/。

mod api;
mod bootstrap;
mod core;
mod settings;

use crate::settings::Settings;

#[tokio::main]
async fn main() {
    let cli = settings::Cli::parse_args();
    // --dump-openapi：打印代码生成的 OpenAPI schema 到 stdout 后退出（openapi.json 的固化来源；
    // 契约测试校验二者同步，改 API 后用 `cargo run -- --dump-openapi > openapi.json` 更新）
    if cli.dump_openapi {
        print!("{}", api::build_openapi_json());
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    bootstrap::run(Settings::from_cli(cli)).await;
}
