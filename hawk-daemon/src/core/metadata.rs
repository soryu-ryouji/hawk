//! 素材参数元数据（.hawk/metadata/<hash>.toml，唯一权威数据源，参与网盘同步）。
//! 宽高与调色板是「内容的纯函数」直接入 TOML；解析用 toml crate（宽容缺省），
//! 序列化手写以精确控制输出格式（标量在前、[[paths]] 在后，缺省字段省略）。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq)]
pub struct PathEntry {
    pub path: String,
    pub size: i64,
    pub modification_time: i64,
}

/// 调色板条目（对应 TOML 的 [[palette]] 表）
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PaletteEntry {
    pub color: String,
    pub percentage: f32,
}

#[derive(Clone, Default)]
pub struct ItemMetadata {
    pub paths: Vec<PathEntry>,
    pub url: Option<String>,
    pub tags: Vec<String>,
    pub categories: Vec<String>,
    pub star: i32,
    pub annotation: Option<String>,
    /// 图像宽（像素）；0 = 未知/非图像
    pub width: i32,
    /// 图像高（像素）；0 = 未知/非图像
    pub height: i32,
    /// 调色板（按占比降序，最多 10 色）；None = 未提炼，Some(vec![]) = 已提炼但无有效像素（负缓存）
    pub palette: Option<Vec<PaletteEntry>>,
}

impl ItemMetadata {
    pub fn find_path(&self, path: &str) -> Option<&PathEntry> {
        self.paths.iter().find(|p| p.path == path)
    }

    pub fn find_path_mut(&mut self, path: &str) -> Option<&mut PathEntry> {
        self.paths.iter_mut().find(|p| p.path == path)
    }
}

#[derive(Deserialize, Default)]
struct RawMetadata {
    url: Option<String>,
    star: Option<i64>,
    annotation: Option<String>,
    tags: Option<Vec<String>>,
    categories: Option<Vec<String>>,
    paths: Option<Vec<RawPath>>,
    width: Option<i64>,
    height: Option<i64>,
    palette: Option<Vec<PaletteEntry>>,
}

#[derive(Deserialize)]
struct RawPath {
    path: String,
    size: Option<i64>,
    modification_time: Option<i64>,
}

/// 解析 TOML 文本（宽容：缺省字段取默认；解析失败由调用方处理）
pub fn parse(toml_text: &str) -> Result<ItemMetadata, toml::de::Error> {
    let raw: RawMetadata = toml::from_str(toml_text)?;
    let mut meta = ItemMetadata {
        url: raw.url,
        star: raw.star.unwrap_or(0) as i32,
        annotation: raw.annotation,
        tags: raw.tags.unwrap_or_default(),
        categories: raw.categories.unwrap_or_default(),
        width: raw.width.unwrap_or(0) as i32,
        height: raw.height.unwrap_or(0) as i32,
        palette: raw.palette,
        paths: Vec::new(),
    };
    if let Some(paths) = raw.paths {
        meta.paths = paths
            .into_iter()
            .map(|p| PathEntry {
                path: p.path,
                size: p.size.unwrap_or(0),
                modification_time: p.modification_time.unwrap_or(0),
            })
            .collect();
    }
    Ok(meta)
}

/// 序列化为 TOML。schema 固定，手写序列化以精确控制输出格式
pub fn serialize(meta: &ItemMetadata) -> String {
    let mut sb = String::new();
    if let Some(url) = &meta.url {
        sb.push_str("url = ");
        sb.push_str(&toml_string(url));
        sb.push('\n');
    }
    if !meta.tags.is_empty() {
        sb.push_str("tags = [");
        sb.push_str(
            &meta.tags.iter().map(|t| toml_string(t)).collect::<Vec<_>>().join(", "),
        );
        sb.push_str("]\n");
    }
    if !meta.categories.is_empty() {
        sb.push_str("categories = [");
        sb.push_str(
            &meta.categories.iter().map(|c| toml_string(c)).collect::<Vec<_>>().join(", "),
        );
        sb.push_str("]\n");
    }
    if meta.star > 0 {
        sb.push_str(&format!("star = {}\n", meta.star));
    }
    if let Some(annotation) = &meta.annotation {
        sb.push_str("annotation = ");
        sb.push_str(&toml_string(annotation));
        sb.push('\n');
    }
    if meta.width > 0 {
        sb.push_str(&format!("width = {}\n", meta.width));
    }
    if meta.height > 0 {
        sb.push_str(&format!("height = {}\n", meta.height));
    }
    for p in &meta.paths {
        sb.push('\n');
        sb.push_str("[[paths]]\n");
        sb.push_str("path = ");
        sb.push_str(&toml_string(&p.path));
        sb.push('\n');
        sb.push_str(&format!("size = {}\n", p.size));
        sb.push_str(&format!("modification_time = {}\n", p.modification_time));
    }
    // 调色板:空表是负缓存(已提炼无有效像素),同样持久化
    if let Some(palette) = &meta.palette {
        for p in palette {
            sb.push('\n');
            sb.push_str("[[palette]]\n");
            sb.push_str("color = ");
            sb.push_str(&toml_string(&p.color));
            sb.push('\n');
            sb.push_str(&format!("percentage = {:.1}\n", p.percentage));
        }
    }
    sb
}

pub fn toml_string(value: &str) -> String {
    let mut sb = String::with_capacity(value.len() + 2);
    sb.push('"');
    for c in value.chars() {
        match c {
            '\\' => sb.push_str("\\\\"),
            '"' => sb.push_str("\\\""),
            '\n' => sb.push_str("\\n"),
            '\r' => sb.push_str("\\r"),
            '\t' => sb.push_str("\\t"),
            c if c.is_control() => sb.push_str(&format!("\\u{:04X}", c as u32)),
            c => sb.push(c),
        }
    }
    sb.push('"');
    sb
}

/// 只识别 64 位小写 hex 命名的元数据文件（同步冲突副本等一律忽略）
pub fn is_valid_hash_file_name(name: &str) -> bool {
    name.len() == 64 && name.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_document() {
        let text = r##"
url = "https://example.com/photo.jpg"
tags = ["nature", "sunset"]
categories = ["海报", "灵感参考"]
star = 4
annotation = "Beautiful sunset"
width = 1920
height = 1080

[[paths]]
path = "posters/2024/sunset-photo.jpg"
size = 245760
modification_time = 1700000000000

[[palette]]
color = "#344441"
percentage = 3.1
"##;
        let meta = parse(text).unwrap();
        assert_eq!(meta.url.as_deref(), Some("https://example.com/photo.jpg"));
        assert_eq!(meta.tags, vec!["nature", "sunset"]);
        assert_eq!(meta.categories, vec!["海报", "灵感参考"]);
        assert_eq!(meta.star, 4);
        assert_eq!(meta.width, 1920);
        assert_eq!(meta.paths.len(), 1);
        assert_eq!(meta.paths[0].path, "posters/2024/sunset-photo.jpg");
        assert_eq!(meta.paths[0].size, 245760);
        assert_eq!(meta.palette.as_ref().unwrap()[0].color, "#344441");
        assert!((meta.palette.as_ref().unwrap()[0].percentage - 3.1).abs() < 1e-6);
    }

    #[test]
    fn parse_empty_document() {
        let meta = parse("").unwrap();
        assert_eq!(meta.star, 0);
        assert!(meta.paths.is_empty());
        assert!(meta.palette.is_none());
    }

    #[test]
    fn serialize_omits_defaults_and_orders_scalars_first() {
        let meta = ItemMetadata {
            tags: vec!["nature".to_string()],
            star: 4,
            width: 10,
            height: 8,
            paths: vec![PathEntry {
                path: "a/b.png".to_string(),
                size: 100,
                modification_time: 42,
            }],
            palette: Some(vec![PaletteEntry {
                color: "#344441".to_string(),
                percentage: 100.0,
            }]),
            ..ItemMetadata::default()
        };
        let text = serialize(&meta);
        assert_eq!(
            text,
            "tags = [\"nature\"]\nstar = 4\nwidth = 10\nheight = 8\n\n[[paths]]\npath = \"a/b.png\"\nsize = 100\nmodification_time = 42\n\n[[palette]]\ncolor = \"#344441\"\npercentage = 100.0\n"
        );
        // 往返
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.tags, meta.tags);
        assert_eq!(parsed.paths, meta.paths);
        assert_eq!(parsed.palette, meta.palette);
    }

    #[test]
    fn serialize_escapes_strings() {
        assert_eq!(toml_string("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    }

    #[test]
    fn hash_file_name_validation() {
        assert!(is_valid_hash_file_name(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_valid_hash_file_name("ABC123"));
        assert!(!is_valid_hash_file_name(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd.sync-conflict-20250101"
        ));
        // 大写 hex 不识别
        assert!(!is_valid_hash_file_name(
            "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF"
        ));
    }
}
