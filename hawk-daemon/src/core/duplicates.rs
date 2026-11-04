//! 重复图片分组：内容 hash 相同（同一 item）的多个库内位置。
//! 只做精确内容判定，不做感知哈希或视觉相似度推断。

use crate::core::item::ItemLocation;

#[derive(Clone, Debug)]
pub struct DuplicateItem {
    pub id: String,
    /// 当前用户可见的非回收站位置
    pub locations: Vec<ItemLocation>,
}

#[derive(Clone, Debug)]
pub struct DuplicateMember {
    pub id: String,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct DuplicateGroup {
    pub id: String,
    pub members: Vec<DuplicateMember>,
}

/// 相同内容：同一内容 hash（item id）的可见库内位置数 > 1。
/// 按组大小降序、组 id 升序保证分页稳定。
pub fn exact_groups(items: &[DuplicateItem]) -> Vec<DuplicateGroup> {
    let mut groups: Vec<DuplicateGroup> = items
        .iter()
        .filter(|item| item.locations.len() > 1)
        .map(|item| DuplicateGroup {
            id: item.id.clone(),
            members: item
                .locations
                .iter()
                .map(|location| DuplicateMember {
                    id: item.id.clone(),
                    path: location.path.clone(),
                })
                .collect(),
        })
        .collect();
    groups.sort_by(|a, b| {
        b.members
            .len()
            .cmp(&a.members.len())
            .then_with(|| a.id.cmp(&b.id))
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, paths: &[&str]) -> DuplicateItem {
        DuplicateItem {
            id: id.to_string(),
            locations: paths
                .iter()
                .map(|path| ItemLocation {
                    path: path.to_string(),
                    size: 1,
                    modification_time: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn exact_groups_only_include_multiple_visible_locations() {
        let items = vec![item("a", &["a.png", "copy.png"]), item("b", &["b.png"])];
        let groups = exact_groups(&items);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "a");
        assert_eq!(groups[0].members.len(), 2);
        assert_eq!(groups[0].members[1].path, "copy.png");
    }
}
