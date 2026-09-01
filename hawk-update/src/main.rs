//! hawk Windows 更新辅助程序：等旧进程退出 → 解压更新包 → 覆盖应用目录 → 拉起新实例。
//! 由 hawk-app 主进程复制到更新临时目录后调用（temp 副本运行，避免应用目录内同名文件
//! 在覆盖时被自身占用）：
//!   hawk-update.exe --pid <旧主进程PID> --zip <更新包> --app <应用目录>
//!
//! 全过程写 <更新包目录>\install.log，失败以非零码退出。此前用 PowerShell 脚本做同一件事，
//! 执行策略、PSModulePath 污染、引号拼接编码等环境差异不可穷举，静默失败导致「点安装
//! 没反应」且现场无迹可循——确定性行为与可诊断性是本程序存在的意义。

use std::fs::{self, File};
use std::io::BufReader;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use zip::ZipArchive;

/// 等待旧进程退出的上限：超时则继续安装（单文件覆盖有重试兜底）
const WAIT_OLD_PROCESS_TIMEOUT: Duration = Duration::from_secs(30);
/// 单文件覆盖的重试窗口：吸收杀软扫描、hawk-daemon 收尾等短暂占用
const COPY_RETRY_WINDOW: Duration = Duration::from_secs(6);

struct Args {
    pid: u32,
    zip: PathBuf,
    app: PathBuf,
}

/// 过程日志：同步落盘到 <更新包目录>\install.log（stdout 无人在看）
struct Log(Option<File>);

impl Log {
    fn open(path: &Path) -> Log {
        Log(File::create(path).ok())
    }

    fn line(&mut self, msg: &str) {
        println!("{msg}");
        if let Some(f) = self.0.as_mut() {
            let _ = writeln!(f, "{msg}").and_then(|_| f.flush());
        }
    }
}

/// UTF-16 码元大小写折叠（仅 ASCII 字母；进程名比较用）
fn lower_u16(c: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&c) {
        c + (b'a' - b'A') as u16
    } else {
        c
    }
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    let mut log = Log::open(&args.zip.with_file_name("install.log"));
    log.line(&format!(
        "hawk-update 开始：pid={} zip={} app={}",
        args.pid,
        args.zip.display(),
        args.app.display()
    ));
    if let Err(msg) = run(&args, &mut log) {
        log.line(&format!("安装失败：{msg}"));
        std::process::exit(1);
    }
    log.line("安装完成");
}

fn run(args: &Args, log: &mut Log) -> Result<(), String> {
    // 路径定锚（canonicalize 保证日志/暂存目录落在更新包同目录的绝对路径上）
    let zip = fs::canonicalize(&args.zip).map_err(|e| format!("更新包不存在 {}：{e}", args.zip.display()))?;
    let dir = zip.parent().ok_or("更新包路径缺少父目录")?.to_path_buf();

    wait_old_process(args.pid, log);

    let extract = dir.join("extract");
    let _ = fs::remove_dir_all(&extract); // 上次失败残留
    extract_zip(&zip, &extract)?;
    log.line("解压完成");

    let root = find_app_root(&extract).ok_or("更新包内未找到 hawk.exe（布局异常）")?;
    copy_tree(&root, &args.app, log)?;
    log.line("应用目录覆盖完成");

    Command::new(args.app.join("hawk.exe"))
        .current_dir(&args.app)
        .spawn()
        .map_err(|e| format!("拉起新实例失败：{e}"))?;
    log.line("已拉起新实例");

    let _ = fs::remove_dir_all(&extract);
    let _ = fs::remove_file(&zip);
    Ok(())
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let (mut pid, mut zip, mut app) = (None, None, None);
    while let Some(key) = it.next() {
        let val = it.next().ok_or(format!("参数 {key} 缺少值"))?;
        match key.as_str() {
            "--pid" => pid = Some(val.parse::<u32>().map_err(|_| format!("--pid 不是数字：{val}"))?),
            "--zip" => zip = Some(PathBuf::from(val)),
            "--app" => app = Some(PathBuf::from(val)),
            other => return Err(format!("未知参数：{other}")),
        }
    }
    Ok(Args {
        pid: pid.unwrap_or(0),
        zip: zip.ok_or("缺少 --zip")?,
        app: app.ok_or("缺少 --app")?,
    })
}

/// 等待旧主进程退出：OpenProcess + WaitForSingleObject（内核信号，比轮询进程列表可靠）。
/// 打开后先校验可执行文件名，PID 已被系统复用给其他进程时直接跳过（等错进程会白等 30s，
/// 用户视角同样是「没反应」）。
fn wait_old_process(pid: u32, log: &mut Log) {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE,
    };
    if pid == 0 {
        return;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            log.line(&format!("旧进程 {pid} 已退出"));
            return;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // 后缀匹配 "hawk.exe"（UTF-16 码元逐位比较，仅 ASCII 字母不区分大小写）
        let is_hawk = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len) != 0
            && buf[..len as usize]
                .iter()
                .rev()
                .zip(b"hawk.exe".iter().rev())
                .all(|(a, b)| lower_u16(*a) == lower_u16(*b as u16));
        if !is_hawk {
            CloseHandle(handle);
            log.line(&format!("PID {pid} 已被其他进程复用，跳过等待"));
            return;
        }
        log.line(&format!("等待旧进程 {pid} 退出…"));
        let start = Instant::now();
        loop {
            match WaitForSingleObject(handle, 300) {
                WAIT_OBJECT_0 => break,
                WAIT_TIMEOUT if start.elapsed() < WAIT_OLD_PROCESS_TIMEOUT => continue,
                _ => {
                    log.line("等待超时，继续安装（由文件复制重试兜底）");
                    break;
                }
            }
        }
        CloseHandle(handle);
    }
}

/// 解压更新包到 dest（防路径穿越；保留条目内 unix 权限位与本程序无关）
fn extract_zip(zip: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(zip).map_err(|e| format!("打开更新包失败：{e}"))?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(|e| format!("读取更新包失败：{e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("更新包条目损坏：{e}"))?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!("更新包含非法路径：{}", entry.name()));
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| format!("建目录失败 {}：{e}", out.display()))?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("建目录失败 {}：{e}", parent.display()))?;
            }
            let mut out_f = File::create(&out).map_err(|e| format!("写文件失败 {}：{e}", out.display()))?;
            std::io::copy(&mut entry, &mut out_f).map_err(|e| format!("写文件失败 {}：{e}", out.display()))?;
        }
    }
    Ok(())
}

/// 更新包内应用根目录：hawk.exe 所在目录（electron-builder zip 为根布局，兼容嵌套目录布局）
fn find_app_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("hawk.exe").is_file() {
        return Some(dir.to_path_buf());
    }
    for entry in fs::read_dir(dir).ok()? {
        let p = entry.ok()?.path();
        if p.is_dir() {
            if let Some(found) = find_app_root(&p) {
                return Some(found);
            }
        }
    }
    None
}

/// 递归覆盖复制（合并语义：文件覆盖、目录合并；不删除应用目录中更新包没有的文件）
fn copy_tree(src: &Path, dst: &Path, log: &mut Log) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("建目录失败 {}：{e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("读目录失败 {}：{e}", src.display()))? {
        let entry = entry.map_err(|e| format!("读目录失败 {}：{e}", src.display()))?;
        let s = entry.path();
        let d = dst.join(entry.file_name());
        if s.is_dir() {
            copy_tree(&s, &d, log)?;
        } else {
            copy_file_retry(&s, &d, log)?;
        }
    }
    Ok(())
}

fn copy_file_retry(src: &Path, dst: &Path, log: &mut Log) -> Result<(), String> {
    let start = Instant::now();
    loop {
        match fs::copy(src, dst) {
            Ok(_) => return Ok(()),
            Err(e) if start.elapsed() < COPY_RETRY_WINDOW => {
                log.line(&format!("{} 被占用（{e}），200ms 后重试", dst.display()));
                sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("覆盖失败 {}：{e}", dst.display())),
        }
    }
}
