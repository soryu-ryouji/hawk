//! SSE 事件总线。订阅者各自持有有界广播通道；订阅者消费跟不上时断开其订阅
//! （前端重连后通过 item/skeleton + folder/list 全量对齐）。
//! 事件总线（订阅者积压 1024 条时断开）。

use serde_json::Value;
use tokio::sync::broadcast;

const SUBSCRIBER_CAPACITY: usize = 1024;

#[derive(Clone, Debug)]
pub struct LibraryEvent {
    pub kind: &'static str,
    pub payload: Value,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<LibraryEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        EventBus::new()
    }
}

impl EventBus {
    pub fn new() -> EventBus {
        let (tx, _) = broadcast::channel(SUBSCRIBER_CAPACITY);
        EventBus { tx }
    }

    /// 订阅。返回的 Receiver 用于 SSE 端点；lagged 即消费跟不上，调用方应断开该订阅
    pub fn subscribe(&self) -> broadcast::Receiver<LibraryEvent> {
        self.tx.subscribe()
    }

    pub fn publish(&self, kind: &'static str, payload: Value) {
        // send 失败（无订阅者）是常态，忽略
        let _ = self.tx.send(LibraryEvent { kind, payload });
    }
}

/// 后台任务进度快照(task.progress 事件与 app/status 端点共用)。
/// phase/processed/total 仅 index 任务在扫描期间携带；非扫描期间省略（None）
#[derive(Clone, serde::Serialize, utoipa::ToSchema)]
pub struct TaskProgress {
    pub task: &'static str,
    pub pending: i32,
    pub active: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processed: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i32>,
}

/// folder.changed 事件负载。reason 目前恒为 external，客户端应忽略取值(结构为将来预留)
pub fn folder_changed_payload(reason: &str) -> Value {
    serde_json::json!({ "reason": reason })
}

pub const REASON_EXTERNAL: &str = "external";
