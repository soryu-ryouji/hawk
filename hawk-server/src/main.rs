use hawk_server::settings::{Cli, Command};

#[tokio::main]
async fn main() {
    let cli = Cli::load();
    // 租户管理子命令：同步执行后直接退出（不初始化 tracing，输出保持干净）
    if let Some(Command::Tenant { action }) = &cli.command {
        if let Err(e) = hawk_server::run_tenant_command(action, &cli.data_dir()) {
            eprintln!("租户管理失败: {e}");
            std::process::exit(2);
        }
        return;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    if let Err(e) = hawk_server::run(&cli).await {
        eprintln!("hawk-server 运行失败: {e}");
        std::process::exit(1);
    }
}
