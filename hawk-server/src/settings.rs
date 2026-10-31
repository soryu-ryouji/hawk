//! 启动参数与租户管理子命令：serve 为默认动作（无子命令即启动服务）；
//! `tenant add/list/revoke` 管理 tenant_key（程序生成，明文只打印一次）。

use clap::{FromArgMatches, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "hawk-server",
    about = "hawk 远程访问服务端（TenantAdmin/Device 控制面与密文中继）"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// 监听端口（TLS 由前置反代终结，本进程监听明文 HTTP）
    #[arg(long, env = "HAWK_SERVER_PORT", default_value_t = 9100, global = true)]
    pub port: u16,

    /// 管理面监听端口（仅环回 127.0.0.1：租户创建/查询/吊销，与公网主端口分离）
    #[arg(long, env = "HAWK_SERVER_ADMIN_PORT", default_value_t = 9110)]
    pub admin_port: u16,

    /// 内嵌中继监听端口（iroh-relay；中继 = 打洞协调 + 数据兜底）
    #[arg(
        long,
        env = "HAWK_SERVER_RELAY_PORT",
        default_value_t = 9102,
        global = true
    )]
    pub relay_port: u16,

    /// 内嵌中继的公网 URL（http(s)://host:9102 或反代域名）；未配置 = 本部署不提供中继
    ///（纯直连形态：无中继即无打洞协调，只有直连地址可达的会话能建立）
    #[arg(long, env = "HAWK_SERVER_RELAY_URL", global = true)]
    pub relay_url: Option<String>,

    /// 数据目录（设备/租户注册表、pepper、relay token 与 server.toml）；
    /// 缺省 ~/.config/hawk-server（与 hawk-app 的 ~/.config/hawk 同族的全平台约定）
    #[arg(long, env = "HAWK_SERVER_DATA_DIR", global = true)]
    pub data_dir: Option<PathBuf>,
}

impl Cli {
    /// 数据目录：显式参数/env 优先，缺省 ~/.config/hawk-server
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone().unwrap_or_else(default_data_dir)
    }
}

/// 默认数据目录（全平台统一 ~/.config/hawk-server；取不到 home 目录时退回当前目录，
/// 不让「找不到 home」这种环境问题阻断启动）
fn default_data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    match home {
        Some(home) => home.join(".config").join("hawk-server"),
        None => PathBuf::from("hawk-server-data"),
    }
}

/// 配置文件（数据目录 server.toml）：首启自动写模板；
/// 优先级：CLI 参数 > 环境变量 > 配置文件 > 默认值（部署者免每次手调环境变量）
const DEFAULT_SERVER_TOML: &str = r#"# hawk-server 配置（与 CLI/环境变量同义；CLI > env > 本文件 > 默认值）
# port = 9100            # 主端口（明文 HTTP，TLS 由前置反代终结）
# admin_port = 9110      # 管理面（仅环回，不得暴露）
# relay_port = 9102      # 内嵌中继监听端口
# relay_url = "https://hawk.example.com:9102"  # 中继公网入口；不配 = 不提供中继（纯直连）
"#;

/// server.toml 的读取面（全部可选；缺字段回落默认值）
#[derive(serde::Deserialize, Default)]
struct ServerToml {
    port: Option<u16>,
    admin_port: Option<u16>,
    relay_port: Option<u16>,
    relay_url: Option<String>,
}

impl ServerToml {
    fn load(file: &std::path::Path) -> ServerToml {
        match std::fs::read_to_string(file) {
            Ok(text) => toml_edit::de::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("server.toml 解析失败，按默认值继续: {e}");
                ServerToml::default()
            }),
            Err(_) => {
                // 首启写模板（写失败无碍运行）
                let _ = std::fs::write(file, DEFAULT_SERVER_TOML);
                ServerToml::default()
            }
        }
    }
}

impl Cli {
    /// 启动入口：clap 解析后与数据目录的 server.toml 合并（未显式提供的字段取文件值）
    pub fn load() -> Cli {
        let matches = <Self as clap::CommandFactory>::command().get_matches();
        let mut cli = Self::from_arg_matches(&matches).expect("clap 参数解析");
        let cfg = ServerToml::load(&cli.data_dir().join("server.toml"));
        cli.merge_file(&matches, &cfg);
        cli
    }

    /// 文件合并：仅填充「值来源为默认值」的字段（CLI/环境变量显式提供的不动）
    fn merge_file(&mut self, matches: &clap::ArgMatches, cfg: &ServerToml) {
        use clap::parser::ValueSource;
        // 未显式提供 = 默认值来源或无值来源（relay_url 这类 Option 参数无默认值为后者）
        let defaulted = |name: &str| {
            !matches!(
                matches.value_source(name),
                Some(ValueSource::CommandLine) | Some(ValueSource::EnvVariable)
            )
        };
        if defaulted("port") {
            if let Some(v) = cfg.port {
                self.port = v;
            }
        }
        if defaulted("admin_port") {
            if let Some(v) = cfg.admin_port {
                self.admin_port = v;
            }
        }
        if defaulted("relay_port") {
            if let Some(v) = cfg.relay_port {
                self.relay_port = v;
            }
        }
        if defaulted("relay_url") {
            if let Some(v) = cfg.relay_url.clone().filter(|u| !u.trim().is_empty()) {
                self.relay_url = Some(v);
            }
        }
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// 租户（tenant_key）管理：创建/列出/吊销
    Tenant {
        #[command(subcommand)]
        action: TenantAction,
    },
}

#[derive(Subcommand)]
pub enum TenantAction {
    /// 创建租户并生成 tenant_key（明文只打印这一次，服务端只存 HMAC 哈希）
    Add {
        /// 租户标签（仅运维识别用，如 "home"）
        #[arg(long)]
        label: String,
    },
    /// 列出租户（id/标签/创建时间；不含密钥本体）
    List,
    /// 吊销租户：TenantAdmin/Device Session 下一请求失效（设备记录保留）
    Revoke {
        /// 租户 id（tenant list 查看）
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::parser::ValueSource;

    #[test]
    fn file_template_loads_and_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("server.toml");
        // 首启写模板（全注释 → 全默认）
        let cfg = ServerToml::load(&file);
        assert!(cfg.port.is_none());
        assert!(file.exists());
        // 写入覆盖值后读取生效
        std::fs::write(
            &file,
            "port = 9200\nrelay_url = \"https://r.example.com:9102\"\n",
        )
        .unwrap();
        let cfg = ServerToml::load(&file);
        assert_eq!(cfg.port, Some(9200));
        assert_eq!(cfg.relay_url.as_deref(), Some("https://r.example.com:9102"));
    }

    #[test]
    fn cli_and_env_beat_file() {
        // CLI 显式参数不被文件覆盖
        let matches = <Cli as clap::CommandFactory>::command()
            .try_get_matches_from(["hawk-server", "--port", "9200"])
            .unwrap();
        let mut cli = Cli::from_arg_matches(&matches).unwrap();
        let cfg = ServerToml {
            port: Some(9300),
            admin_port: Some(9310),
            relay_port: Some(9302),
            relay_url: Some("https://file.example.com".to_string()),
        };
        cli.merge_file(&matches, &cfg);
        assert_eq!(cli.port, 9200, "CLI 显式值不被文件覆盖");
        assert_eq!(cli.admin_port, 9310, "默认值字段取文件值");
        assert_eq!(cli.relay_port, 9302);
        assert_eq!(cli.relay_url.as_deref(), Some("https://file.example.com"));
        // value_source 验证
        assert_eq!(matches.value_source("port"), Some(ValueSource::CommandLine));
    }
}
