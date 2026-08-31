//! 启动状态：先监听、后索引。就绪前 /health 返回 503、/api/* 被就绪网关拦截
//! （app/startup 除外）；进度经 /api/v1/app/startup 查询。

use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct StartupInfo {
    pub phase: String,
    pub processed: i32,
    pub total: i32,
}

#[derive(Default)]
pub struct StartupState {
    inner: Mutex<StartupInner>,
}

struct StartupInner {
    phase: String,
    processed: i32,
    total: i32,
    is_ready: bool,
    error: Option<String>,
}

impl StartupInner {
    fn new() -> StartupInner {
        StartupInner {
            phase: "scan".to_string(),
            processed: 0,
            total: 0,
            is_ready: false,
            error: None,
        }
    }
}

impl Default for StartupInner {
    fn default() -> Self {
        Self::new()
    }
}

impl StartupState {
    pub fn report(&self, phase: &str, processed: i32, total: i32) {
        let mut inner = self.inner.lock().unwrap();
        inner.phase = phase.to_string();
        inner.processed = processed;
        inner.total = total;
    }

    pub fn snapshot(&self) -> (bool, Option<String>, StartupInfo) {
        let inner = self.inner.lock().unwrap();
        (
            inner.is_ready,
            inner.error.clone(),
            StartupInfo {
                phase: inner.phase.clone(),
                processed: inner.processed,
                total: inner.total,
            },
        )
    }

    pub fn is_ready(&self) -> bool {
        self.inner.lock().unwrap().is_ready
    }

    pub fn mark_ready(&self) {
        self.inner.lock().unwrap().is_ready = true;
    }

    /// 初始索引失败原因写入（进程保留，错误经 app/startup 暴露）；当前注水路径失败即panic前不可用
    #[allow(dead_code)]
    pub fn fail(&self, message: &str) {
        self.inner.lock().unwrap().error = Some(message.to_string());
    }
}
