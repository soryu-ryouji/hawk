//! 启动设置：素材库路径、监听端口、访问 token。
//! 桌面版由 Electron 通过命令行 / 环境变量传入（见 docs/architecture.md）。

pub const DEFAULT_PORT: u16 = 27371;

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
    pub fn from_args() -> Settings {
        let mut library = std::env::var("HAWK_LIBRARY").ok();
        let mut port = std::env::var("HAWK_PORT").ok().and_then(|v| v.parse::<u16>().ok());
        let mut web_dist = std::env::var("HAWK_WEB_DIST").ok();
        let mut cache_parent = std::env::var("HAWK_CACHE_PARENT").ok();
        let token = std::env::var("HAWK_TOKEN").ok();

        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--library" if i + 1 < args.len() => {
                    library = Some(args[i + 1].clone());
                    i += 1;
                }
                "--port" if i + 1 < args.len() => {
                    port = args[i + 1].parse::<u16>().ok();
                    i += 1;
                }
                "--web-dist" if i + 1 < args.len() => {
                    web_dist = Some(args[i + 1].clone());
                    i += 1;
                }
                "--cache-parent" if i + 1 < args.len() => {
                    cache_parent = Some(args[i + 1].clone());
                    i += 1;
                }
                _ => {}
            }
            i += 1;
        }

        let library = match library.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            Some(l) => l,
            None => {
                eprintln!("用法: hawk-daemon --library <素材库路径> [--port <端口>]");
                eprintln!("环境变量: HAWK_LIBRARY / HAWK_PORT / HAWK_TOKEN / HAWK_RESCAN_INTERVAL(对账扫描间隔秒,0 关闭,默认 60)");
                std::process::exit(2);
            }
        };

        if !std::path::Path::new(&library).is_dir() {
            eprintln!("素材库目录不存在: {library}");
            std::process::exit(2);
        }

        // token 只存在于进程环境中；未传入时（开发场景）生成随机 token 并打印到 stdout
        let token = token.unwrap_or_else(|| {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).expect("生成随机 token 失败");
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            println!("{hex}");
            hex
        });

        Settings {
            library_root: library,
            port: port.unwrap_or(DEFAULT_PORT),
            token,
            rescan_interval_seconds: std::env::var("HAWK_RESCAN_INTERVAL")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60),
            cache_parent,
            web_dist,
        }
    }
}
