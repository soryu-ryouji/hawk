//! 请求授权：Principal + Channel + CapabilitySet。
//! 角色只提供固定能力预设，最终能力再与通道上限求交集。

use crate::api::envelope::{codes, ApiError};
use axum::http::{Method, StatusCode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Read,
    Import,
    Edit,
    Manage,
}

impl Capability {
    const fn bit(self) -> u8 {
        match self {
            Capability::Read => 1,
            Capability::Import => 1 << 1,
            Capability::Edit => 1 << 2,
            Capability::Manage => 1 << 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilitySet(u8);

impl CapabilitySet {
    const READ: CapabilitySet = CapabilitySet(Capability::Read.bit());
    const ALL: CapabilitySet = CapabilitySet(15);

    const fn contains(self, capability: Capability) -> bool {
        self.0 & capability.bit() != 0
    }

    const fn intersection(self, other: CapabilitySet) -> CapabilitySet {
        CapabilitySet(self.0 & other.0)
    }
}

pub use crate::core::access::Role;

impl Role {
    const fn capabilities(self) -> CapabilitySet {
        match self {
            Role::Viewer => CapabilitySet::READ,
            Role::Importer => CapabilitySet(Capability::Read.bit() | Capability::Import.bit()),
            Role::Editor => CapabilitySet(
                Capability::Read.bit() | Capability::Import.bit() | Capability::Edit.bit(),
            ),
            Role::Admin => CapabilitySet::ALL,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Loopback,
    LanWeb,
    LanPeer,
    Wan,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Loopback => "loopback",
            Channel::LanWeb => "lan_web",
            Channel::LanPeer => "lan_peer",
            Channel::Wan => "wan",
        }
    }

    const fn capability_ceiling(self) -> CapabilitySet {
        match self {
            Channel::Loopback => CapabilitySet::ALL,
            Channel::LanWeb | Channel::LanPeer => CapabilitySet(
                Capability::Read.bit() | Capability::Import.bit() | Capability::Edit.bit(),
            ),
            Channel::Wan => CapabilitySet(Capability::Read.bit() | Capability::Import.bit()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    ProcessAdmin,
    LocalIntegration { id: String },
    Device { id: String },
    AccessKey { id: String },
}

#[derive(Clone, Debug)]
pub struct AccessContext {
    pub principal: Principal,
    pub role: Role,
    pub channel: Channel,
    capabilities: CapabilitySet,
}

impl AccessContext {
    pub fn new(principal: Principal, role: Role, channel: Channel) -> AccessContext {
        let capabilities = role
            .capabilities()
            .intersection(channel.capability_ceiling());
        AccessContext {
            principal,
            role,
            channel,
            capabilities,
        }
    }

    pub fn require(&self, capability: Capability) -> Result<(), ApiError> {
        if self.capabilities.contains(capability) {
            return Ok(());
        }
        Err(ApiError::new(
            codes::READ_ONLY,
            StatusCode::FORBIDDEN,
            format!(
                "{} role cannot perform {:?} over {}",
                self.role.as_str(),
                capability,
                self.channel.as_str()
            ),
        ))
    }
}

/// 路由能力表：所有非豁免端点必须在这里声明，未知方法/路径返回 None 并拒绝。
/// 认证豁免（admin-token、配对创建与状态轮询）在 auth 中间件中单独处理。
pub(crate) fn required_capability(method: &Method, path: &str) -> Option<Capability> {
    if path == "/api/v1/app/connection" {
        return if method == Method::GET {
            Some(Capability::Import)
        } else {
            None
        };
    }

    if path == "/api/v1/library/info" {
        return if method == Method::GET {
            Some(Capability::Read)
        } else if method == Method::PATCH {
            Some(Capability::Manage)
        } else {
            None
        };
    }

    if MANAGE_PATHS.contains(&path)
        || path.starts_with("/api/v1/app/lan/device-keys/")
        || path.starts_with("/api/v1/remote/devices/")
        || path.starts_with("/api/v1/remote/keys/")
        || path.starts_with("/api/v1/app/lan/pair-requests/")
    {
        return Some(Capability::Manage);
    }

    if method == Method::GET && GET_READ_PATHS.contains(&path) {
        return Some(Capability::Read);
    }
    if method == Method::POST && POST_READ_PATHS.contains(&path) {
        return Some(Capability::Read);
    }
    if method == Method::POST && IMPORT_PATHS.contains(&path) {
        return Some(Capability::Import);
    }
    if matches!(*method, Method::POST | Method::PUT | Method::DELETE) && EDIT_PATHS.contains(&path)
    {
        return Some(Capability::Edit);
    }
    None
}

const GET_READ_PATHS: &[&str] = &[
    "/api/v1/app/info",
    "/api/v1/app/startup",
    "/api/v1/app/status",
    "/api/v1/category/list",
    "/api/v1/events",
    "/api/v1/folder/list",
    "/api/v1/global_filter/list",
    "/api/v1/item/count",
    "/api/v1/item/detail",
    "/api/v1/item/file",
    "/api/v1/item/thumbnail",
    "/api/v1/lock/list",
    "/api/v1/tag/list",
    "/api/v1/view/preferences",
];

const POST_READ_PATHS: &[&str] = &[
    "/api/v1/remote/access-session",
    "/api/v1/remote/lease",
    "/api/v1/app/session",
    "/api/v1/item/aggregate",
    "/api/v1/item/list",
    "/api/v1/item/skeleton",
    "/api/v1/lock/unlock",
];

const IMPORT_PATHS: &[&str] = &[
    "/api/v1/folder/create",
    "/api/v1/item/add",
    "/api/v1/item/upload",
];

const EDIT_PATHS: &[&str] = &[
    "/api/v1/category/create",
    "/api/v1/category/delete",
    "/api/v1/category/update",
    "/api/v1/folder/delete",
    "/api/v1/folder/restore",
    "/api/v1/folder/update",
    "/api/v1/global_filter",
    "/api/v1/item/batch_update",
    "/api/v1/item/delete",
    "/api/v1/item/refresh_thumbnail",
    "/api/v1/item/replace",
    "/api/v1/item/restore",
    "/api/v1/item/update",
    "/api/v1/tag/create",
    "/api/v1/tag/delete",
    "/api/v1/tag/update",
    "/api/v1/trash/clear",
    "/api/v1/view/preference",
];

const MANAGE_PATHS: &[&str] = &[
    "/api/v1/app/connection",
    "/api/v1/app/lan",
    "/api/v1/app/lan/clients",
    "/api/v1/app/lan/device-keys",
    "/api/v1/app/lan/pair-requests",
    "/api/v1/app/lan/peers",
    "/api/v1/item/duplicates",
    "/api/v1/library/cleanup_index",
    "/api/v1/library/refresh_cache",
    "/api/v1/library/reindex",
    "/api/v1/library/rescan",
    "/api/v1/library/scan",
    "/api/v1/lock/remove",
    "/api/v1/lock/set",
    "/api/v1/remote",
    "/api/v1/remote/device",
    "/api/v1/remote/device/rotate",
    "/api/v1/remote/access-connect",
    "/api/v1/remote/devices",
    "/api/v1/remote/keys",
    "/api/v1/remote/session",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_and_channel_form_effective_capabilities() {
        let editor = AccessContext::new(
            Principal::AccessKey { id: "k".into() },
            Role::Editor,
            Channel::LanPeer,
        );
        assert!(editor.require(Capability::Edit).is_ok());
        assert!(editor.require(Capability::Manage).is_err());

        let remote = AccessContext::new(
            Principal::Device { id: "s".into() },
            Role::Editor,
            Channel::Wan,
        );
        assert!(remote.require(Capability::Import).is_ok());
        assert!(remote.require(Capability::Edit).is_err());
        assert!(remote.require(Capability::Manage).is_err());
    }
}
