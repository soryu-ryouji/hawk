//! 颜色工具：hex 解析/格式化、sRGB→CIELAB 转换、CIE76 ΔE 距离。纯函数。

/// CIELAB 色彩空间中的一个颜色（感知均匀，用于颜色相似度比较）
#[derive(Clone, Copy, Debug)]
pub struct LabColor {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

/// 解析 "#344441" / "344441"（大小写不敏感）为 RGB；非法返回 None
pub fn parse_hex(text: Option<&str>) -> Option<(u8, u8, u8)> {
    let s = text?.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// RGB → "#344441"（小写 # 前缀）
pub fn to_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// sRGB → CIELAB（D65 光源）
pub fn rgb_to_lab(r: u8, g: u8, b: u8) -> LabColor {
    let rl = pivot_rgb(r as f64 / 255.0);
    let gl = pivot_rgb(g as f64 / 255.0);
    let bl = pivot_rgb(b as f64 / 255.0);

    let x = (rl * 0.4124564 + gl * 0.3575761 + bl * 0.1804375) / 0.95047;
    let y = rl * 0.2126729 + gl * 0.7151522 + bl * 0.0721750;
    let z = (rl * 0.0193339 + gl * 0.1191920 + bl * 0.9503041) / 1.08883;

    let fx = pivot_xyz(x);
    let fy = pivot_xyz(y);
    let fz = pivot_xyz(z);

    LabColor {
        l: 116.0 * fy - 16.0,
        a: 500.0 * (fx - fy),
        b: 200.0 * (fy - fz),
    }
}

/// CIE76 ΔE 的平方。与阈值的平方比较，免去逐像素开方
pub fn delta_e_squared(a: LabColor, b: LabColor) -> f64 {
    let dl = a.l - b.l;
    let da = a.a - b.a;
    let db = a.b - b.b;
    dl * dl + da * da + db * db
}

fn pivot_rgb(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn pivot_xyz(t: f64) -> f64 {
    if t > 0.008856 {
        t.cbrt()
    } else {
        7.787 * t + 16.0 / 116.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(parse_hex(Some("#344441")), Some((0x34, 0x44, 0x41)));
        assert_eq!(parse_hex(Some("344441")), Some((0x34, 0x44, 0x41)));
        assert_eq!(parse_hex(Some("#ABCDEF")), Some((0xab, 0xcd, 0xef)));
        assert_eq!(to_hex(0x34, 0x44, 0x41), "#344441");
        assert_eq!(parse_hex(Some("red")), None);
        assert_eq!(parse_hex(Some("#12345")), None);
        assert_eq!(parse_hex(None), None);
    }

    #[test]
    fn lab_known_values() {
        // D65 sRGB 基准值（系数浮点噪声，容差放宽到 1e-4）
        let white = rgb_to_lab(255, 255, 255);
        assert!((white.l - 100.0).abs() < 1e-4);
        assert!(white.a.abs() < 1e-4);
        assert!(white.b.abs() < 1e-4);

        let black = rgb_to_lab(0, 0, 0);
        assert!(black.l.abs() < 1e-6);

        // 中灰的参考 Lab（sRGB #808080 → L*≈53.585）
        let gray = rgb_to_lab(128, 128, 128);
        assert!((gray.l - 53.585).abs() < 0.01, "gray L = {}", gray.l);
    }

    #[test]
    fn delta_e_monotonic() {
        let red = rgb_to_lab(255, 0, 0);
        let near = rgb_to_lab(238, 0, 0);
        let far = rgb_to_lab(0, 255, 0);
        assert!(delta_e_squared(red, near) < delta_e_squared(red, far));
        assert_eq!(delta_e_squared(red, red), 0.0);
    }
}
