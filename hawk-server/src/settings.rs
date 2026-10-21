//! 启动参数与租户管理子命令：serve 为默认动作（无子命令即启动服务）；
//! `tenant add/list/revoke` 管理 service-key（程序生成，明文只打印一次）。

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "hawk-server",
    about = "hawk 远程访问服务端（多租户：service-key 准入 + hawk-app key 映射）"
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

    /// 数据目录（设备/租户/key 注册表、pepper、网页版身份与令牌）；
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

#[derive(Subcommand)]
pub enum Command {
    /// 租户（service-key）管理：创建/列出/吊销
    Tenant {
        #[command(subcommand)]
        action: TenantAction,
    },
}

#[derive(Subcommand)]
pub enum TenantAction {
    /// 创建租户并生成 service-key（明文只打印这一次，服务端只存 HMAC 哈希）
    Add {
        /// 租户标签（仅运维识别用，如 "home"）
        #[arg(long)]
        label: String,
    },
    /// 列出租户（id/标签/创建时间；不含密钥本体）
    List,
    /// 吊销租户：service-key 立即失效，池内 hawk-app key 一并失效（设备记录保留）
    Revoke {
        /// 租户 id（tenant list 查看）
        id: String,
    },
}
