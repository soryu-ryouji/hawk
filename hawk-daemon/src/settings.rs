//! 启动设置：`local` 使用本地素材库，`remote` 仅查看远程素材库，`openapi` 导出契约。
//! 桌面版由 Electron 通过命令行 / 环境变量传入（见 docs/architecture.md）。
//! CLI 定义集中在 Cli（clap derive，单一权威：未知参数/非法值直接报错退出），
//! Settings 只承接业务校验与加工——解析层/配置层分离。
//! token 与对账间隔只走环境变量、不进命令行命名空间：token 避免出现在进程列表（ps 可见），
//! 对账间隔是内部调参（ui-check 缩短用），Electron 的传参约定不变。

use clap::{Args, Parser, Subcommand};

pub const DEFAULT_PORT: u16 = 27371;

/// 命令行接口（各项支持同名 HAWK_* 环境变量回退，命令行优先于环境变量）
#[derive(Parser)]
#[command(name = "hawk-daemon", version, about = "hawk 素材库后端服务")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// clap 解析入口（clap 依赖集中在 settings 模块，main 不经手）
    pub fn parse_args() -> Cli {
        <Cli as Parser>::parse()
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// 使用本地素材库
    Local(LocalArgs),
    /// 无本地库，仅查看远程素材库
    Remote(RemoteArgs),
    /// 打印 OpenAPI schema 到 stdout 后退出
    Openapi,
}

#[derive(Args)]
pub struct LocalArgs {
    /// 素材库根目录
    #[arg(value_name = "PATH", env = "HAWK_LIBRARY")]
    pub library: String,

    #[command(flatten)]
    pub daemon: DaemonArgs,

    /// 应用级远程访问配置（remote.toml，含身份目录）；缺省为 <library>/.hawk/remote.toml
    #[arg(long, env = "HAWK_REMOTE_CONFIG", value_name = "PATH")]
    pub remote_config: Option<String>,
}

#[derive(Args)]
pub struct RemoteArgs {
    #[command(flatten)]
    pub daemon: DaemonArgs,

    /// 应用级远程访问配置（remote.toml，含身份目录）
    #[arg(long, env = "HAWK_REMOTE_CONFIG", value_name = "PATH")]
    pub remote_config: String,
}

#[derive(Args)]
pub struct DaemonArgs {
    /// 本地监听端口：严格绑定，被占用即用固定文案报错退出（不自动换端口）。
    /// 0 = 由系统分配空闲端口（仅供测试/脚本，桌面端总是传显式端口）
    #[arg(long, env = "HAWK_PORT", default_value_t = DEFAULT_PORT)]
    pub port: u16,

    /// 局域网 web 查看托管的前端静态文件目录；不存在则不托管
    #[arg(long, env = "HAWK_WEB_DIST", value_name = "DIR")]
    pub web_dist: Option<String>,

    /// 全局缓存父目录；缺省用系统缓存目录
    #[arg(long, env = "HAWK_CACHE_PARENT", value_name = "DIR")]
    pub cache_parent: Option<String>,
}

#[derive(Clone)]
pub struct Settings {
    /// 本地素材库根目录；None = 远程查看模式（无本地库）
    pub library_root: Option<String>,
    pub port: u16,
    pub token: String,
    /// 派生数据维护间隔（秒），0 关闭：周期补齐调色板、宽高与感知哈希
    pub maintenance_interval_seconds: u64,
    /// 全局缓存父目录（桌面端设置面板配置，主进程经 --cache-parent 传入）；None 用系统缓存目录
    pub cache_parent: Option<String>,
    /// 应用级远程访问配置路径（Electron 传 ~/.config/hawk/remote.toml）；None 用库内缺省
    pub remote_config: Option<String>,
    /// 局域网 web 查看托管的前端静态文件目录（Electron 传入 web/dist）；不存在则不托管
    pub web_dist: Option<String>,
}

impl Settings {
    pub fn from_local(args: LocalArgs) -> Settings {
        let library = args.library.trim().to_string();
        if library.is_empty() || !std::path::Path::new(&library).is_dir() {
            eprintln!("素材库目录不存在: {library}");
            std::process::exit(2);
        }
        Settings::from_parts(Some(library), args.daemon, args.remote_config)
    }

    pub fn from_remote(args: RemoteArgs) -> Settings {
        Settings::from_parts(None, args.daemon, Some(args.remote_config))
    }

    fn from_parts(
        library_root: Option<String>,
        daemon: DaemonArgs,
        remote_config: Option<String>,
    ) -> Settings {
        let remote_config = remote_config
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty());
        if library_root.is_none() && remote_config.is_none() {
            eprintln!("remote 模式必须提供有效的 --remote-config");
            std::process::exit(2);
        }

        Settings {
            library_root,
            port: daemon.port,
            token: access_token(),
            maintenance_interval_seconds: std::env::var("HAWK_MAINTENANCE_INTERVAL")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(60),
            cache_parent: daemon.cache_parent,
            web_dist: daemon.web_dist,
            remote_config,
        }
    }
}

/// token 只存在于进程环境中；未传入时（开发场景）生成随机 token 并打印到 stdout
fn access_token() -> String {
    std::env::var("HAWK_TOKEN").ok().unwrap_or_else(|| {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("生成随机 token 失败");
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        println!("{hex}");
        hex
    })
}
