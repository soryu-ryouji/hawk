use clap::Parser;
use hawk_server::settings::Settings;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let settings = Settings::parse();
    if settings.access_key.len() < 16 {
        eprintln!("access key 过短：至少 16 个字符");
        std::process::exit(2);
    }
    if let Some(write_key) = &settings.write_key {
        if write_key.len() < 16 {
            eprintln!("write key 过短：至少 16 个字符");
            std::process::exit(2);
        }
        if write_key == &settings.access_key {
            eprintln!("write key 不能与 access key 相同");
            std::process::exit(2);
        }
    }
    if let Err(e) = hawk_server::run(settings).await {
        eprintln!("hawk-server 运行失败: {e}");
        std::process::exit(1);
    }
}
