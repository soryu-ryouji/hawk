//! 端口契约：daemon 严格绑定 `--port`（不静默漂移），占用时以退出码 4 + stderr
//! `PORT_IN_USE <port>` 退出——壳层（Electron）据此在启动屏询问用户换端口。
//! 浏览器插件的本地模式按 27371–27400 段探活，依赖的正是「端口只由壳层决定」这一点。

use std::process::Command;

/// 端口被占：退出码 4 + 可解析的固定文案
#[test]
fn occupied_port_exits_with_port_in_use() {
    // 占住一个端口（不 drop，进程内持有到测试结束）
    let holder = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = holder.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_hawk-daemon"))
        .args([
            "remote",
            "--port",
            &port.to_string(),
            "--remote-config",
            dir.path().join("remote.toml").to_str().unwrap(),
        ])
        .env("HAWK_ADMIN_TOKEN", "port-test-token")
        .output()
        .expect("拉起 hawk-daemon");

    assert_eq!(output.status.code(), Some(4), "端口占用应以退出码 4 退出");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("PORT_IN_USE {port}")),
        "stderr 应含可解析的端口占用标记，实际: {stderr}"
    );
}

/// 端口空闲：正常进入启动流程（这里用 SIGKILL 前的最小存活窗口断言它没有立刻因端口失败退出）
#[test]
fn free_port_does_not_fail_binding() {
    let dir = tempfile::tempdir().unwrap();
    let port = {
        let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        probe.local_addr().unwrap().port()
    };

    let mut child = Command::new(env!("CARGO_BIN_EXE_hawk-daemon"))
        .args([
            "remote",
            "--port",
            &port.to_string(),
            "--remote-config",
            dir.path().join("remote.toml").to_str().unwrap(),
        ])
        .env("HAWK_ADMIN_TOKEN", "port-test-token")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("拉起 hawk-daemon");

    // 给足退出窗口：若端口绑定失败会立刻退出（退出码 4/5）
    std::thread::sleep(std::time::Duration::from_millis(800));
    match child.try_wait() {
        Ok(Some(status)) => panic!("空闲端口不应启动失败，实际退出码 {:?}", status.code()),
        Ok(None) => {}
        Err(e) => panic!("等待子进程失败: {e}"),
    }
    let _ = child.kill();
    let _ = child.wait();
}
