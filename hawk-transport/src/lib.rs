//! 原生移动端共享传输：C ABI 启动/停止 AccessKey Loopback 代理。
//! UI 不接触 iroh、rendezvous 或 AccessKey 的协议细节，只读取返回的本地代理地址。

use hawk_daemon::core::events::EventBus;
use hawk_daemon::remote::config::RemoteConfigStore;
use hawk_daemon::remote::identity::Identity;
use hawk_daemon::remote::session::{EndpointSlot, SessionManager};
use libc::{c_char, c_void};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

struct SessionHandle {
    manager: Arc<SessionManager>,
    base: String,
}

static SESSIONS: OnceLock<Mutex<HashMap<String, SessionHandle>>> = OnceLock::new();
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn sessions() -> &'static Mutex<HashMap<String, SessionHandle>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("hawk transport runtime")
    })
}

fn text(pointer: *const c_char) -> Result<String, String> {
    if pointer.is_null() {
        return Err("参数为空".to_string());
    }
    let value = unsafe { CStr::from_ptr(pointer) };
    value
        .to_str()
        .map(str::to_string)
        .map_err(|e| format!("参数不是有效 UTF-8: {e}"))
}

fn response(value: Value) -> *mut c_char {
    let text = value.to_string();
    match CString::new(text) {
        Ok(text) => text.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn error(message: impl Into<String>) -> *mut c_char {
    response(json!({ "error": message.into() }))
}

/// 启动 AccessKey 原生代理。返回 JSON：
/// `{ session_id, api_base, credential, role, target_device_id }`，或 `{ error }`。
#[no_mangle]
pub extern "C" fn hawk_transport_connect_access(
    config_dir: *const c_char,
    server_url: *const c_char,
    target_device_id: *const c_char,
    target_identity: *const c_char,
    access_key: *const c_char,
) -> *mut c_char {
    let result = (|| {
        let config_dir = text(config_dir)?;
        let base = text(server_url)?.trim_end_matches('/').to_string();
        let target_device_id = text(target_device_id)?;
        let target_identity = text(target_identity)?;
        let access_key = text(access_key)?;
        let remote_config = RemoteConfigStore::open(
            Some(
                std::path::Path::new(&config_dir)
                    .join("remote.toml")
                    .to_str()
                    .ok_or_else(|| "config_dir 非 UTF-8".to_string())?,
            ),
            None,
        );
        let slot: EndpointSlot = Default::default();
        let manager = SessionManager::new(EventBus::new(), remote_config, slot);
        let identity = Identity::load_or_create(&config_dir)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| format!("HTTP 客户端初始化失败: {e}"))?;
        let dto = runtime().block_on(manager.connect_access(
            &http,
            &base,
            &identity,
            &target_device_id,
            &target_identity,
            &access_key,
        ));
        let dto = dto.map_err(|e| e.message)?;
        sessions().lock().unwrap().insert(
            dto.session_id.clone(),
            SessionHandle {
                manager: manager.clone(),
                base: base.clone(),
            },
        );
        Ok(response(json!({
            "session_id": dto.session_id,
            "api_base": format!("http://127.0.0.1:{}", dto.proxy_port),
            "credential": dto.session_token,
            "role": dto.role.as_str(),
            "target_device_id": dto.target_device_id,
        })))
    })();
    result.unwrap_or_else(|e: String| error(e))
}

/// 停止会话并取消 rendezvous/Device 会话。返回 `{ "status": "success" }` 或 `{ error }`。
#[no_mangle]
pub extern "C" fn hawk_transport_disconnect(session_id: *const c_char) -> *mut c_char {
    let result = (|| {
        let session_id = text(session_id)?;
        let handle = sessions()
            .lock()
            .unwrap()
            .remove(&session_id)
            .ok_or_else(|| "会话不存在".to_string())?;
        let http = reqwest::Client::new();
        runtime().block_on(handle.manager.disconnect(&http, &handle.base, ""));
        Ok(response(json!({ "status": "success" })))
    })();
    result.unwrap_or_else(|e: String| error(e))
}

/// 释放 connect/disconnect 返回的字符串。
#[no_mangle]
pub extern "C" fn hawk_transport_free_string(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(pointer.cast::<c_char>());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_argument_is_an_error_not_a_crash() {
        let response = hawk_transport_connect_access(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        );
        assert!(!response.is_null());
        let value = unsafe { CStr::from_ptr(response) }
            .to_str()
            .unwrap()
            .to_string();
        assert!(value.contains("参数为空"));
        hawk_transport_free_string(response.cast());
    }
}
