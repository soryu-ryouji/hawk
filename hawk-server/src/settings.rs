//! 启动参数：单用户方案下服务端配置只有端口、访问密钥、可选写入密钥与数据目录。

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "hawk-server", about = "hawk 远程访问服务端（单用户）")]
pub struct Settings {
    /// 监听端口（TLS 由前置反代终结，本进程监听明文 HTTP）
    #[arg(long, env = "HAWK_SERVER_PORT", default_value_t = 27370)]
    pub port: u16,

    /// 访问密钥：控制面与只读网页版的凭据（REST 走 Authorization: Bearer，WSS 走查询参数）
    #[arg(long, env = "HAWK_SERVER_ACCESS_KEY")]
    pub access_key: String,

    /// 广域网写入密钥（可选）：仅用于换发写作用域的浏览器令牌；不配置则广域网写关闭
    #[arg(long, env = "HAWK_SERVER_WRITE_KEY")]
    pub write_key: Option<String>,

    /// 数据目录（设备注册表 devices.json、网页版身份与令牌）
    #[arg(long, env = "HAWK_SERVER_DATA_DIR", default_value = "hawk-server-data")]
    pub data_dir: PathBuf,
}
