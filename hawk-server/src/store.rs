//! 设备注册表持久化：单用户设备量级小，整表读入内存，变更时原子写盘（tmp + rename）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub device_id: String,
    /// 所属租户（池）；空串 = 全局设备（服务端的「网页版」查看设备，不属于任何池）
    #[serde(default)]
    pub tenant_id: String,
    pub name: String,
    pub platform: String,
    /// iroh EndpointId（Ed25519 公钥 hex；握手按公钥认证，无单独钉扎环节）
    pub endpoint_id: String,
    pub library_name: String,
    pub allow_remote: bool,
    /// 目标设备是否允许广域网写会话（由设备注册/心跳上报）
    #[serde(default)]
    pub allow_import: bool,
    pub registered_at_ms: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct DeviceTable {
    devices: Vec<DeviceRecord>,
}

pub struct Store {
    path: PathBuf,
    devices: HashMap<String, DeviceRecord>,
}

impl Store {
    pub fn load(data_dir: &Path) -> io::Result<Store> {
        fs::create_dir_all(data_dir)?;
        let path = data_dir.join("devices.json");
        let devices = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<DeviceTable>(&bytes) {
                Ok(table) => table
                    .devices
                    .into_iter()
                    .map(|d| (d.device_id.clone(), d))
                    .collect(),
                Err(e) => {
                    tracing::warn!("设备表损坏（{e}），从空表重建");
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Store { path, devices })
    }

    pub fn get(&self, device_id: &str) -> Option<&DeviceRecord> {
        self.devices.get(device_id)
    }

    pub fn list(&self) -> Vec<DeviceRecord> {
        let mut devices: Vec<DeviceRecord> = self.devices.values().cloned().collect();
        devices.sort_by_key(|d| d.registered_at_ms);
        devices
    }

    /// 某租户的设备列表（不含全局设备）
    pub fn list_tenant(&self, tenant_id: &str) -> Vec<DeviceRecord> {
        self.list()
            .into_iter()
            .filter(|d| d.tenant_id == tenant_id)
            .collect()
    }

    pub fn insert(&mut self, record: DeviceRecord) -> io::Result<()> {
        self.devices.insert(record.device_id.clone(), record);
        self.save()
    }

    /// 就地修改并落盘；设备不存在返回 None
    pub fn update(
        &mut self,
        device_id: &str,
        f: impl FnOnce(&mut DeviceRecord),
    ) -> io::Result<Option<DeviceRecord>> {
        let Some(record) = self.devices.get_mut(device_id) else {
            return Ok(None);
        };
        f(record);
        let record = record.clone();
        self.save()?;
        Ok(Some(record))
    }

    pub fn remove(&mut self, device_id: &str) -> io::Result<Option<DeviceRecord>> {
        let removed = self.devices.remove(device_id);
        if removed.is_some() {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> io::Result<()> {
        let table = DeviceTable {
            devices: self.list(),
        };
        let bytes = serde_json::to_vec_pretty(&table)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &self.path)
    }
}
