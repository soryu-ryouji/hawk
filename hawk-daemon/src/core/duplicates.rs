//! 重复图片分组：同内容多位置与 pHash 近重复（BK-tree + 并查集）。
//! BK-tree 在距离 0 时会覆盖/忽略重复值，因此先按 64 位 pHash 预分组，确保不同 item 不丢失。

use crate::core::item::ItemLocation;
use crate::core::perceptual_hash;
use bktree::BkTree;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct DuplicateItem {
    pub id: String,
    /// 当前算法版本的 pHash；非图像、未计算或旧版本均为 None
    pub phash: Option<u64>,
    /// 当前用户可见的非回收站位置
    pub locations: Vec<ItemLocation>,
}

#[derive(Clone, Debug)]
pub struct DuplicateMember {
    pub id: String,
    pub path: String,
    pub distance: u32,
}

#[derive(Clone, Debug)]
pub struct DuplicateGroup {
    pub id: String,
    pub members: Vec<DuplicateMember>,
}

/// 相同内容：同一 item 的可见库内位置数 > 1。按组大小降序、组 id 升序保证分页稳定
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
                    distance: 0,
                })
                .collect(),
        })
        .collect();
    sort_groups(&mut groups);
    groups
}

/// 相似内容：BK-tree 查找近邻，并查集合并为连通分组。只含不同 item；同 item 多位置仍随成员展开
pub fn similar_groups(items: &[DuplicateItem], max_distance: u32) -> Vec<DuplicateGroup> {
    let candidates: Vec<&DuplicateItem> =
        items.iter().filter(|item| item.phash.is_some()).collect();
    if candidates.len() < 2 {
        return Vec::new();
    }

    // pHash 相同的不同 item 先聚到同一 bits 桶，规避 BK-tree 对距离 0 元素的忽略行为
    let mut by_bits: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, item) in candidates.iter().enumerate() {
        by_bits.entry(item.phash.unwrap()).or_default().push(index);
    }
    let mut all_bits: Vec<u64> = by_bits.keys().copied().collect();
    all_bits.sort_unstable();
    let mut union = UnionFind::new(candidates.len());
    if max_distance <= 10 {
        // 4×16bit 分带：距离 ≤10 时至少一段距离 ≤2；严格档（≤5）只需 0/1 位变体。
        // 候选后再计算完整距离，保证不漏近邻，且不被 BK-tree 高维最坏情况拖慢。
        let bands = BandedIndex::new(&all_bits);
        for bits in &all_bits {
            for other in bands.candidates(*bits, max_distance) {
                for left in &by_bits[bits] {
                    for right in &by_bits[&other] {
                        union.union(*left, *right);
                    }
                }
            }
        }
    } else {
        // 非常用的大阈值仍走通用度量树，避免分带变体组合爆炸
        let mut tree = BkTree::new(|a: &u64, b: &u64| perceptual_hash::distance(*a, *b) as isize);
        tree.insert_all(all_bits.iter().copied());
        for bits in &all_bits {
            for (other, _) in tree.find(*bits, max_distance as isize) {
                if bits == other {
                    continue;
                }
                for left in &by_bits[bits] {
                    for right in &by_bits[other] {
                        union.union(*left, *right);
                    }
                }
            }
        }
    }

    let mut grouped: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..candidates.len() {
        grouped.entry(union.find(index)).or_default().push(index);
    }
    let mut groups: Vec<DuplicateGroup> = grouped
        .into_values()
        .filter(|members| members.len() > 1)
        .map(|mut members| {
            members.sort_by(|a, b| candidates[*a].id.cmp(&candidates[*b].id));
            let representative = members[0];
            let representative_hash = candidates[representative].phash.unwrap();
            let group_id = candidates[representative].id.clone();
            let mut entries = Vec::new();
            for index in members {
                let item = candidates[index];
                let distance = perceptual_hash::distance(representative_hash, item.phash.unwrap());
                for location in &item.locations {
                    entries.push(DuplicateMember {
                        id: item.id.clone(),
                        path: location.path.clone(),
                        distance,
                    });
                }
            }
            entries.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.path.cmp(&b.path)));
            DuplicateGroup {
                id: group_id,
                members: entries,
            }
        })
        .collect();
    sort_groups(&mut groups);
    groups
}

fn sort_groups(groups: &mut [DuplicateGroup]) {
    groups.sort_by(|a, b| {
        b.members
            .len()
            .cmp(&a.members.len())
            .then_with(|| a.id.cmp(&b.id))
    });
}

struct BandedIndex {
    bands: [HashMap<u16, Vec<u64>>; 4],
}

impl BandedIndex {
    fn new(bits: &[u64]) -> Self {
        let mut bands: [HashMap<u16, Vec<u64>>; 4] = std::array::from_fn(|_| HashMap::new());
        for value in bits {
            for (index, bucket) in bands.iter_mut().enumerate() {
                bucket.entry(band(*value, index)).or_default().push(*value);
            }
        }
        Self { bands }
    }

    fn candidates(&self, value: u64, max_distance: u32) -> std::collections::HashSet<u64> {
        let mut found = std::collections::HashSet::new();
        let max_band_distance = max_distance / 4;
        for (index, band_index) in self.bands.iter().enumerate() {
            let target = band(value, index);
            for_each_variant(target, max_band_distance, |variant| {
                if let Some(bucket) = band_index.get(&variant) {
                    for other in bucket {
                        if *other != value
                            && perceptual_hash::distance(value, *other) <= max_distance
                        {
                            found.insert(*other);
                        }
                    }
                }
            });
        }
        found
    }
}

fn band(value: u64, index: usize) -> u16 {
    ((value >> (index * 16)) & 0xffff) as u16
}

/// 枚举一个 16 位段的 0/1/2 位变体；调用方根据阈值决定枚举深度
fn for_each_variant(value: u16, max_distance: u32, mut visit: impl FnMut(u16)) {
    visit(value);
    if max_distance == 0 {
        return;
    }
    for a in 0..16 {
        let a_value = value ^ (1 << a);
        visit(a_value);
        if max_distance >= 2 {
            for b in (a + 1)..16 {
                visit(a_value ^ (1 << b));
            }
        }
    }
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
        }
    }

    fn find(&mut self, value: usize) -> usize {
        let mut root = value;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut current = value;
        while self.parent[current] != current {
            let next = self.parent[current];
            self.parent[current] = root;
            current = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let a = self.find(a);
        let b = self.find(b);
        if a != b {
            self.parent[b] = a;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, hash: Option<u64>, paths: &[&str]) -> DuplicateItem {
        DuplicateItem {
            id: id.to_string(),
            phash: hash,
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
        let items = vec![
            item("a", None, &["a.png", "copy.png"]),
            item("b", None, &["b.png"]),
        ];
        let groups = exact_groups(&items);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "a");
        assert_eq!(groups[0].members.len(), 2);
        assert!(groups[0].members.iter().all(|member| member.distance == 0));
    }

    #[test]
    fn similar_groups_preserve_distinct_items_with_identical_phash() {
        let items = vec![
            item("a", Some(0), &["a.png"]),
            item("b", Some(0), &["b.png"]),
            item("c", Some(1), &["c.png"]),
            item("far", Some(u64::MAX), &["far.png"]),
        ];
        let groups = similar_groups(&items, 1);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "a");
        assert_eq!(groups[0].members.len(), 3);
        assert!(groups[0]
            .members
            .iter()
            .any(|member| member.id == "b" && member.distance == 0));
        assert!(groups[0]
            .members
            .iter()
            .any(|member| member.id == "c" && member.distance == 1));
    }

    #[test]
    fn similarity_index_matches_brute_force_components() {
        let mut state = 42u64;
        let items: Vec<DuplicateItem> = (0..80)
            .map(|index| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                item(&format!("{index:02x}"), Some(state & 0xff), &["x.png"])
            })
            .collect();
        // 0/1/2 位分带路径 + 大阈值 BK-tree 回退都必须与暴力分组一致
        for threshold in [0, 1, 3, 12] {
            let actual = similar_groups(&items, threshold);
            let expected = brute_components(&items, threshold);
            assert_eq!(actual.len(), expected.len(), "threshold={threshold}");
            for (group, expected_ids) in actual.iter().zip(expected.iter()) {
                let ids: Vec<String> = group
                    .members
                    .iter()
                    .map(|member| member.id.clone())
                    .collect();
                assert_eq!(&ids, expected_ids, "threshold={threshold}");
            }
        }
    }

    fn brute_components(items: &[DuplicateItem], max_distance: u32) -> Vec<Vec<String>> {
        let mut union = UnionFind::new(items.len());
        for (i, a) in items.iter().enumerate() {
            for (j, b) in items.iter().enumerate().skip(i + 1) {
                if perceptual_hash::distance(a.phash.unwrap(), b.phash.unwrap()) <= max_distance {
                    union.union(i, j);
                }
            }
        }
        let mut groups: HashMap<usize, Vec<String>> = HashMap::new();
        for (index, item) in items.iter().enumerate() {
            let root = union.find(index);
            groups.entry(root).or_default().push(item.id.clone());
        }
        let mut groups: Vec<Vec<String>> = groups
            .into_values()
            .filter(|members| members.len() > 1)
            .map(|mut members| {
                members.sort();
                members
            })
            .collect();
        groups.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a[0].cmp(&b[0])));
        groups
    }

    /// 手动性能基准：cargo test similar_groups_large_benchmark -- --ignored --nocapture
    #[test]
    #[ignore]
    fn similar_groups_large_benchmark() {
        let mut state = 42u64;
        let items: Vec<DuplicateItem> = (0..100_000)
            .map(|index| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                item(&format!("{index:064x}"), Some(state), &["x.png"])
            })
            .collect();
        let started = std::time::Instant::now();
        let groups = similar_groups(&items, 5);
        eprintln!(
            "100000 items banded index + union-find: {:?}, groups={}",
            started.elapsed(),
            groups.len()
        );
    }
}
