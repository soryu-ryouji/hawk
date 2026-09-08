//! 进程入口（薄壳）：参数解析（settings）、--dump-openapi 特殊模式分流、日志初始化。
//! 组件组装与启动编排在 bootstrap；HTTP 层在 api/，领域核心在 core/（库入口见 lib.rs）。

use hawk_daemon::api;
use hawk_daemon::bootstrap;
use hawk_daemon::settings::{Cli, Settings};

#[tokio::main]
async fn main() {
    let cli = Cli::parse_args();
    // --dump-openapi：打印代码生成的 OpenAPI schema 到 stdout 后退出（openapi.json 的固化来源；
    // 契约测试校验二者同步，改 API 后用 `cargo run -- --dump-openapi > openapi.json` 更新）
    if cli.dump_openapi {
        print!("{}", api::build_openapi_json());
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // panic 也进日志（默认 hook 仍保留：stderr 尾部是 Electron 侧错误屏的数据源）
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));

    bootstrap::run(Settings::from_cli(cli)).await;
}
