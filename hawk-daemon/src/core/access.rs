//! 访问角色与请求通道：长期密钥、会话和进程管理员共用的固定预设。

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    Viewer,
    Importer,
    Editor,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Importer => "importer",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }
}

/// 请求穿过哪条信任边界。由监听器或可信隧道会话写入，客户端不能经 HTTP 头伪造。
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
}
