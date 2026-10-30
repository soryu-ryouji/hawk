//! 访问角色：长期密钥、会话和进程管理员共用的固定能力预设。

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
