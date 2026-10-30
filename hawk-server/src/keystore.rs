//! 凭证存储：租户（service-key）与 hawk-app key 的哈希注册表，以及门令牌（内存）。
//!
//! 存储原则（防离线爆破）：
//! - pepper：data-dir/pepper 首启自生成（0600），是全部哈希的 HMAC 密钥；
//!   注册表泄露但没有 pepper 时，哈希不可逆推 key
//! - service-key：只存 `HMAC-SHA256(pepper, key)`（tenants.json），明文只在创建时打印一次
//! - hawk-app key：daemon 上送 `s = sha256(key)`，server 存 `{ s, h = HMAC(pepper, s) }`（keys.json）。
//!   s 用于随会话下发给目标 daemon 本地执法（server 被攻破不能放大权限）；
//!   h 用于查看者登录验证。server 全程不见 key 明文

use crate::state::Role;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 门令牌 TTL（service-key 验证通过后，须在此窗口内完成 hawk-app key 验证）
pub const GATE_TTL: Duration = Duration::from_secs(5 * 60);
/// 一枚门令牌允许的 hawk-app key 最大尝试次数（防拿一枚门令牌无限试错）
pub const GATE_ATTEMPT_BUDGET: u32 = 5;

/// HMAC-SHA256（RFC 2104；sha2 0.11 无兼容的 hmac crate，结构简单自行实现，RFC 4231 向量验证）
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut pad = [0u8; BLOCK];
    if key.len() > BLOCK {
        pad[..32].copy_from_slice(&sha2::Sha256::digest(key));
    } else {
        pad[..key.len()].copy_from_slice(key);
    }
    let xor = |byte: u8| -> Vec<u8> { pad.iter().map(|b| b ^ byte).collect() };
    let inner = sha2::Sha256::digest([&xor(0x36)[..], msg].concat());
    sha2::Sha256::digest([&xor(0x5c)[..], &inner].concat()).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(text: &str) -> String {
    hex(&sha2::Sha256::digest(text.as_bytes()))
}

/// 租户记录（service-key 的哈希 + 展示标签）
#[derive(Clone, Serialize, Deserialize)]
pub struct TenantRecord {
    pub id: String,
    pub label: String,
    /// HMAC-SHA256(pepper, service_key)
    pub hmac: String,
    pub created_at_ms: u64,
}

/// hawk-app key 记录（daemon 全量同步的设备级 key 映射）
#[derive(Clone, Serialize, Deserialize)]
pub struct AppKeyRecord {
    /// HMAC-SHA256(pepper, s)：查看者登录的比对值
    pub h: String,
    /// sha256(key)：daemon 上送的验证子，会话时下发给目标 daemon 本地执法
    pub s: String,
    pub tenant_id: String,
    pub device_id: String,
    pub role: Role,
}

/// 门令牌（内存，短时效）：service-key 验证通过 → 允许在 TTL 内尝试有限次 hawk-app key
pub struct Gate {
    pub tenant_id: String,
    pub expires: Instant,
    pub attempts_left: u32,
}

/// 注册表以文件为事实源：每次读取重载（几 KB 量级，自托管规模下可忽略），
/// 写锁只串行本进程的写。这样 CLI 子命令（独立进程）对租户的增删，运行中的
/// server 立即感知，无需重启。跨进程并发写为 last-writer-wins（运维动作稀少，可接受）
pub struct KeyStore {
    dir: PathBuf,
    pepper: Vec<u8>,
    write_lock: Mutex<()>,
    gates: Mutex<HashMap<String, Gate>>,
}

impl KeyStore {
    /// 打开（必要时创建）pepper 与数据目录
    pub fn open(dir: &Path) -> io::Result<KeyStore> {
        std::fs::create_dir_all(dir)?;
        let pepper = load_or_create_pepper(dir)?;
        Ok(KeyStore {
            dir: dir.to_path_buf(),
            pepper,
            write_lock: Mutex::new(()),
            gates: Mutex::new(HashMap::new()),
        })
    }

    // ---------- 租户（service-key） ----------

    /// 创建租户：返回 (tenant_id, 明文 service-key)。明文只在此刻可见，不落盘
    pub fn tenant_add(&self, label: &str) -> io::Result<(String, String)> {
        let service_key = format!(
            "hs_{:016x}{:016x}{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>(),
            rand::random::<u64>(),
            rand::random::<u64>()
        );
        let record = TenantRecord {
            id: format!("t_{:016x}", rand::random::<u64>()),
            label: label.to_string(),
            hmac: self.service_key_hmac(&service_key),
            created_at_ms: crate::state::now_ms(),
        };
        let _guard = self.write_lock.lock().unwrap();
        let mut tenants: Vec<TenantRecord> = load_list(&self.dir.join("tenants.json"))?;
        tenants.push(record.clone());
        save_list(&self.dir.join("tenants.json"), &tenants)?;
        Ok((record.id, service_key))
    }

    pub fn tenant_list(&self) -> Vec<TenantRecord> {
        load_list(&self.dir.join("tenants.json")).unwrap_or_default()
    }

    /// 吊销租户（service-key 失效；该池设备记录保留但无法再通过鉴权；池内 key 映射一并清除）
    pub fn tenant_revoke(&self, id: &str) -> io::Result<bool> {
        let _guard = self.write_lock.lock().unwrap();
        let mut tenants: Vec<TenantRecord> = load_list(&self.dir.join("tenants.json"))?;
        let before = tenants.len();
        tenants.retain(|t| t.id != id);
        if tenants.len() == before {
            return Ok(false);
        }
        save_list(&self.dir.join("tenants.json"), &tenants)?;
        let mut keys: Vec<AppKeyRecord> = load_list(&self.dir.join("keys.json"))?;
        keys.retain(|k| k.tenant_id != id);
        save_list(&self.dir.join("keys.json"), &keys)?;
        Ok(true)
    }

    /// service-key → 租户（验证失败返回 None）
    pub fn resolve_tenant(&self, service_key: &str) -> Option<TenantRecord> {
        let hmac = self.service_key_hmac(service_key);
        load_list::<TenantRecord>(&self.dir.join("tenants.json"))
            .ok()?
            .into_iter()
            .find(|t| crate::auth::constant_time_eq(t.hmac.as_bytes(), hmac.as_bytes()))
    }

    fn service_key_hmac(&self, service_key: &str) -> String {
        hex(&hmac_sha256(&self.pepper, service_key.as_bytes()))
    }

    // ---------- hawk-app key ----------

    /// 全量同步某设备的 key 集合（daemon 推送；条目先按格式校验，任一非法整体拒绝）
    pub fn sync_device_keys(
        &self,
        tenant_id: &str,
        device_id: &str,
        keys: &[AppKeySync],
    ) -> io::Result<()> {
        let mut records = Vec::with_capacity(keys.len());
        for k in keys {
            records.push(AppKeyRecord {
                h: hex(&hmac_sha256(&self.pepper, k.s.as_bytes())),
                s: k.s.clone(),
                tenant_id: tenant_id.to_string(),
                device_id: device_id.to_string(),
                role: k.role,
            });
        }
        let _guard = self.write_lock.lock().unwrap();
        let mut all: Vec<AppKeyRecord> = load_list(&self.dir.join("keys.json"))?;
        // 覆盖本设备集合；同时按哈希全局去重——设备重注册换新 device_id 后，
        // 同一 key 的孤儿映射（挂在旧设备记录下）会被最新同步顶掉，
        // 否则 resolve 可能命中设备已不存在的旧映射
        all.retain(|k| !(k.tenant_id == tenant_id && k.device_id == device_id));
        all.retain(|k| !records.iter().any(|r| r.h == k.h));
        all.extend(records);
        save_list(&self.dir.join("keys.json"), &all)
    }

    /// 移除某设备的全部 key（设备注销时清理）
    pub fn remove_device_keys(&self, device_id: &str) -> io::Result<()> {
        let _guard = self.write_lock.lock().unwrap();
        let mut all: Vec<AppKeyRecord> = load_list(&self.dir.join("keys.json"))?;
        let before = all.len();
        all.retain(|k| k.device_id != device_id);
        if all.len() != before {
            save_list(&self.dir.join("keys.json"), &all)?;
        }
        Ok(())
    }

    /// 查看者登录验证：hawk-app key → 映射记录（tenant/device/权限）
    pub fn resolve_app_key(&self, presented: &str) -> Option<AppKeyRecord> {
        let h = hex(&hmac_sha256(&self.pepper, sha256_hex(presented).as_bytes()));
        load_list::<AppKeyRecord>(&self.dir.join("keys.json"))
            .ok()?
            .into_iter()
            .find(|k| crate::auth::constant_time_eq(k.h.as_bytes(), h.as_bytes()))
    }

    // ---------- 门令牌 ----------

    /// 签发门令牌（service-key 已验证）
    pub fn issue_gate(&self, tenant_id: &str) -> String {
        let token = format!(
            "gt_{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        );
        self.gates.lock().unwrap().insert(
            token.clone(),
            Gate {
                tenant_id: tenant_id.to_string(),
                expires: Instant::now() + GATE_TTL,
                attempts_left: GATE_ATTEMPT_BUDGET,
            },
        );
        token
    }

    /// 门令牌所属租户（池冷却检查用，不消耗尝试预算；过期返回 None）
    pub fn gate_tenant(&self, token: &str) -> Option<String> {
        let gates = self.gates.lock().unwrap();
        let gate = gates.get(token)?;
        (gate.expires > Instant::now()).then(|| gate.tenant_id.clone())
    }

    /// 校验门令牌并消耗一次尝试（仅在实际验证 hawk-app key 时调用；过期/耗尽返回 None）
    pub fn consume_gate_attempt(&self, token: &str) -> Option<String> {
        let mut gates = self.gates.lock().unwrap();
        // 顺路惰性清理过期门令牌
        gates.retain(|_, g| g.expires > Instant::now());
        let gate = gates.get_mut(token)?;
        if gate.attempts_left == 0 {
            gates.remove(token);
            return None;
        }
        gate.attempts_left -= 1;
        Some(gate.tenant_id.clone())
    }
}

/// daemon 同步上送的单条 key（s = sha256(key)）
#[derive(Deserialize)]
pub struct AppKeySync {
    pub s: String,
    #[serde(default)]
    pub role: Role,
}

fn pepper_file(dir: &Path) -> PathBuf {
    dir.join("pepper")
}

fn load_or_create_pepper(dir: &Path) -> io::Result<Vec<u8>> {
    let file = pepper_file(dir);
    match std::fs::read(&file) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let bytes: Result<Vec<u8>, _> = (0..text.trim().len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text.trim()[i..i + 2], 16))
                .collect();
            let bytes = bytes.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            if bytes.len() != 32 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "pepper 文件长度异常（应为 64 位十六进制）",
                ));
            }
            Ok(bytes)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let pepper = rand::random::<[u8; 32]>();
            let tmp = file.with_extension("tmp");
            std::fs::write(&tmp, hex(&pepper))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
            }
            std::fs::rename(&tmp, &file)?;
            tracing::info!("已生成凭证胡椒（{}）", file.display());
            Ok(pepper.to_vec())
        }
        Err(e) => Err(e),
    }
}

fn load_list<T: serde::de::DeserializeOwned>(file: &Path) -> io::Result<Vec<T>> {
    match std::fs::read(file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn save_list<T: serde::Serialize>(file: &Path, list: &[T]) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(list)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha256_matches_rfc4231_case1() {
        // RFC 4231 test case 1: key = 20×0x0b, msg = "Hi There"
        let out = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex(&out),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn hmac_sha256_matches_rfc4231_case2() {
        // RFC 4231 test case 2: key = "Jefe", msg = "what do ya want for nothing?"
        let out = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&out),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn tenant_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::open(dir.path()).unwrap();
        let (id, key) = store.tenant_add("home").unwrap();
        assert!(key.starts_with("hs_"));
        assert_eq!(store.resolve_tenant(&key).unwrap().id, id);
        assert!(store.resolve_tenant("hs_wrong").is_none());
        // 重启后仍可用（哈希落盘），明文不出现于 tenants.json
        let raw = std::fs::read_to_string(dir.path().join("tenants.json")).unwrap();
        assert!(!raw.contains(&key));
        let reopened = KeyStore::open(dir.path()).unwrap();
        assert_eq!(reopened.resolve_tenant(&key).unwrap().id, id);
        assert!(reopened.tenant_revoke(&id).unwrap());
        assert!(reopened.resolve_tenant(&key).is_none());
    }

    #[test]
    fn app_key_sync_and_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::open(dir.path()).unwrap();
        let (tenant, _) = store.tenant_add("home").unwrap();
        let s = sha256_hex("my-custom-key");
        store
            .sync_device_keys(
                &tenant,
                "d_1",
                &[AppKeySync {
                    s: s.clone(),
                    role: Role::Importer,
                }],
            )
            .unwrap();
        let found = store.resolve_app_key("my-custom-key").unwrap();
        assert_eq!(found.device_id, "d_1");
        assert_eq!(found.role, Role::Importer);
        assert!(store.resolve_app_key("other").is_none());
        // 再次同步（空集合）= 清空该设备
        store.sync_device_keys(&tenant, "d_1", &[]).unwrap();
        assert!(store.resolve_app_key("my-custom-key").is_none());
    }

    #[test]
    fn sync_dedupes_orphaned_mappings_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::open(dir.path()).unwrap();
        let (tenant, _) = store.tenant_add("home").unwrap();
        let s = sha256_hex("shared-key");
        // 设备重注册前的旧映射
        store
            .sync_device_keys(
                &tenant,
                "d_old",
                &[AppKeySync {
                    s: s.clone(),
                    role: Role::Viewer,
                }],
            )
            .unwrap();
        // 设备换新 id 后重推同一批 key：旧映射被顶掉
        store
            .sync_device_keys(
                &tenant,
                "d_new",
                &[AppKeySync {
                    s: s.clone(),
                    role: Role::Viewer,
                }],
            )
            .unwrap();
        let found = store.resolve_app_key("shared-key").unwrap();
        assert_eq!(found.device_id, "d_new");
        // 全局只有一条
        let raw: Vec<AppKeyRecord> = load_list(&dir.path().join("keys.json")).unwrap();
        assert_eq!(raw.iter().filter(|k| k.s == s).count(), 1);
    }

    #[test]
    fn gate_budget_and_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::open(dir.path()).unwrap();
        let gate = store.issue_gate("t_1");
        for _ in 0..GATE_ATTEMPT_BUDGET {
            assert_eq!(store.consume_gate_attempt(&gate).as_deref(), Some("t_1"));
        }
        // 预算耗尽后再消费即失效
        assert!(store.consume_gate_attempt(&gate).is_none());
    }
}
