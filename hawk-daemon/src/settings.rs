//! 启动设置：素材库路径、监听端口、访问 token。
//! 桌面版由 Electron 通过命令行 / 环境变量传入（见 docs/architecture.md）。
//! CLI 定义集中在 Cli（clap derive，单一权威：未知参数/非法值直接报错退出），
//! Settings 只承接业务校验与加工——解析层/配置层分离。
//! token 与对账间隔只走环境变量、不进命令行命名空间：token 避免出现在进程列表（ps 可见），
//! 对账间隔是内部调参（ui-check 缩短用），Electron 的传参约定不变。

use clap::Parser;

pub const DEFAULT_PORT: u16 = 27371;

/// 命令行接口（每项支持同名 HAWK_* 环境变量回退，命令行优先于环境变量）
#[derive(Parser)]
#[command(name = "hawk-daemon", version, about = "hawk 素材库后端服务")]
pub struct Cli {
    /// 素材库根目录（--dump-openapi 模式豁免必填校验）
    #[arg(long, env = "HAWK_LIBRARY", required_unless_present = "dump_openapi")]
    pub library: Option<String>,

    /// 本地监听端口（被占用时回退动态分配）
    #[arg(long, env = "HAWK_PORT", default_value_t = DEFAULT_PORT)]
    pub port: u16,

    /// 局域网 web 查看托管的前端静态文件目录；不存在则不托管
    #[arg(long, env = "HAWK_WEB_DIST")]
    pub web_dist: Option<String>,

    /// 全局缓存父目录；缺省用系统缓存目录
    #[arg(long, env = "HAWK_CACHE_PARENT")]
    pub cache_parent: Option<String>,

    /// 打印 OpenAPI schema 到 stdout 后退出（openapi.json 的固化来源）
    #[arg(long)]
    pub dump_openapi: bool,
}

impl Cli {
    /// clap 解析入口（clap 依赖集中在 settings 模块，main 不经手）
    pub fn parse_args() -> Cli {
        <Cli as Parser>::parse()
    }
}

#[derive(Clone)]
pub struct Settings {
    pub library_root: String,
    pub port: u16,
    pub token: String,
    /// 周期对账扫描间隔（秒），0 关闭。文件监听可能静默丢事件，周期扫描保证最终一致
    pub rescan_interval_seconds: u64,
    /// 全局缓存父目录（桌面端设置面板配置，主进程经 --cache-parent 传入）；None 用系统缓存目录
    pub cache_parent: Option<String>,
    /// 局域网 web 查看托管的前端静态文件目录（Electron 传入 web/dist）；不存在则不托管
    pub web_dist: Option<String>,
}

impl Settings {
    /// Cli 已完成词法解析（library 非空、端口合法由 clap 保证），此处承接业务校验与 env-only 参数
    pub fn from_cli(cli: Cli) -> Settings {
        let library = cli.library.unwrap_or_default().trim().to_string();
        if library.is_empty() {
            eprintln!("素材库路径为空（--library 或 HAWK_LIBRARY）");
            std::process::exit(2);
        }
        if !std::path::Path::new(&library).is_dir() {
            eprintln!("素材库目录不存在: {library}");
            std::process::exit(2);
        }

        // token 只存在于进程环境中；未传入时（开发场景）生成随机 token 并打印到 stdout
        let token = std::env::var("HAWK_TOKEN").ok().unwrap_or_else(|| {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).expect("生成随机 token 失败");
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            println!("{hex}");
            hex
        });

        Settings {
            library_root: library,
            port: cli.port,
            token,
            rescan_interval_seconds: std::env::var("HAWK_RESCAN_INTERVAL")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60),
            cache_parent: cli.cache_parent,
            web_dist: cli.web_dist,
        }
    }
}
