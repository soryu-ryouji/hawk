//! 租户密钥注册表：tenant_key 只存 HMAC-SHA256(pepper, key)，明文仅创建时返回。
//! AccessKey 不进入 hawk-server；设备签名 nonce 与 Session 由 auth 模块保存在内存。

use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// HMAC-SHA256（RFC 2104；RFC 4231 向量验证）。
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

#[derive(Clone, Serialize, Deserialize)]
pub struct TenantRecord {
    pub id: String,
    pub label: String,
    /// HMAC-SHA256(pepper, tenant_key)
    pub hmac: String,
    pub created_at_ms: u64,
}

/// 注册表以文件为事实源：CLI 独立进程的增删无需重启 server 即可生效。
pub struct KeyStore {
    dir: PathBuf,
    pepper: Vec<u8>,
    write_lock: Mutex<()>,
}

impl KeyStore {
    pub fn open(dir: &Path) -> io::Result<KeyStore> {
        std::fs::create_dir_all(dir)?;
        let pepper = load_or_create_pepper(dir)?;
        Ok(KeyStore {
            dir: dir.to_path_buf(),
            pepper,
            write_lock: Mutex::new(()),
        })
    }

    /// 创建租户：返回 (tenant_id, 明文 tenant_key)。明文只在此刻可见，不落盘。
    pub fn tenant_add(&self, label: &str) -> io::Result<(String, String)> {
        let tenant_key = format!(
            "hs_{:016x}{:016x}{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>(),
            rand::random::<u64>(),
            rand::random::<u64>()
        );
        let record = TenantRecord {
            id: format!("t_{:016x}", rand::random::<u64>()),
            label: label.to_string(),
            hmac: self.tenant_key_hmac(&tenant_key),
            created_at_ms: crate::state::now_ms(),
        };
        let _guard = self.write_lock.lock().unwrap();
        let mut tenants: Vec<TenantRecord> = load_list(&self.dir.join("tenants.json"))?;
        tenants.push(record.clone());
        save_list(&self.dir.join("tenants.json"), &tenants)?;
        Ok((record.id, tenant_key))
    }

    pub fn tenant_list(&self) -> Vec<TenantRecord> {
        load_list(&self.dir.join("tenants.json")).unwrap_or_default()
    }

    pub fn tenant(&self, id: &str) -> Option<TenantRecord> {
        load_list::<TenantRecord>(&self.dir.join("tenants.json"))
            .ok()?
            .into_iter()
            .find(|tenant| tenant.id == id)
    }

    pub fn tenant_revoke(&self, id: &str) -> io::Result<bool> {
        let _guard = self.write_lock.lock().unwrap();
        let mut tenants: Vec<TenantRecord> = load_list(&self.dir.join("tenants.json"))?;
        let before = tenants.len();
        tenants.retain(|tenant| tenant.id != id);
        if tenants.len() == before {
            return Ok(false);
        }
        save_list(&self.dir.join("tenants.json"), &tenants)?;
        Ok(true)
    }

    pub fn resolve_tenant(&self, tenant_key: &str) -> Option<TenantRecord> {
        let hmac = self.tenant_key_hmac(tenant_key);
        load_list::<TenantRecord>(&self.dir.join("tenants.json"))
            .ok()?
            .into_iter()
            .find(|tenant| crate::auth::constant_time_eq(tenant.hmac.as_bytes(), hmac.as_bytes()))
    }

    fn tenant_key_hmac(&self, tenant_key: &str) -> String {
        hex(&hmac_sha256(&self.pepper, tenant_key.as_bytes()))
    }
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
        let out = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex(&out),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn hmac_sha256_matches_rfc4231_case2() {
        let out = hmac_sha256(b"Jefe", "what do ya want for nothing?".as_bytes());
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
        assert_eq!(store.tenant(&id).unwrap().id, id);
        assert!(store.resolve_tenant("hs_wrong").is_none());
        let raw = std::fs::read_to_string(dir.path().join("tenants.json")).unwrap();
        assert!(!raw.contains(&key));
        let reopened = KeyStore::open(dir.path()).unwrap();
        assert_eq!(reopened.resolve_tenant(&key).unwrap().id, id);
        assert!(reopened.tenant_revoke(&id).unwrap());
        assert!(reopened.resolve_tenant(&key).is_none());
        assert!(reopened.tenant(&id).is_none());
    }
}
