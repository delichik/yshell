//! 主题模式与强调色（UI 设计规范 §3.4）。
//!
//! 提供预设色板、按 id 解析与派生色计算（hover / pressed / text / soft）。
//! 本模块只做纯数据计算：不读取系统设置、不读写配置，
//! 「跟随系统」等策略由调用方决定。

/// 主题模式：`system` = 跟随系统。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// 跟随系统的浅色 / 深色设置。
    #[default]
    System,
    /// 始终使用浅色主题。
    Light,
    /// 始终使用深色主题。
    Dark,
}

impl ThemeMode {
    /// 解析 `"system" | "light" | "dark"`（大小写不敏感，允许首尾空白）。
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    /// 规范化标识：`"system"` / `"light"` / `"dark"`。
    pub fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// 8 位 sRGB 颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    /// 红通道。
    pub red: u8,
    /// 绿通道。
    pub green: u8,
    /// 蓝通道。
    pub blue: u8,
}

impl RgbColor {
    /// 构造颜色。
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// 同 `#RRGGBB` 字符串（小写），用于测试与日志。
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
    }

    /// 线性插值：`t = 0` 返回 `self`，`t = 1` 返回 `other`，`t` 会被 clamp 到 `[0, 1]`。
    pub fn mix(self, other: RgbColor, t: f32) -> RgbColor {
        let t = t.clamp(0.0, 1.0);
        let channel = |from: u8, to: u8| -> u8 {
            let value = from as f32 + (to as f32 - from as f32) * t;
            value.round().clamp(0.0, 255.0) as u8
        };
        RgbColor::new(
            channel(self.red, other.red),
            channel(self.green, other.green),
            channel(self.blue, other.blue),
        )
    }

    /// WCAG 相对亮度（`0.0..=1.0`），用于挑选强调色上的文字色。
    pub fn relative_luminance(self) -> f32 {
        // sRGB 通道线性化（WCAG 2.x 公式）。
        fn linearize(channel: u8) -> f32 {
            let channel = channel as f32 / 255.0;
            if channel <= 0.040_45 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        }

        let red = linearize(self.red);
        let green = linearize(self.green);
        let blue = linearize(self.blue);
        // Rec. 709 亮度系数：0.2126 R + 0.7152 G + 0.0722 B。
        0.2126 * red + 0.7152 * green + 0.0722 * blue
    }
}

/// 一个强调色预设（深浅两套基色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentPreset {
    /// 稳定 id，用于 `config.toml` 中 `[appearance].accent` 的持久化。
    pub id: &'static str,
    /// 英文显示名（设置页）。
    pub label_en: &'static str,
    /// 中文显示名（设置页）。
    pub label_zh: &'static str,
    /// 浅色主题基色。
    pub light: RgbColor,
    /// 深色主题基色。
    pub dark: RgbColor,
}

/// 6 个预设，顺序即设置页展示顺序（与规范 §3.4 表格一致）。
pub const ACCENT_PRESETS: [AccentPreset; 6] = [
    // Windows 蓝（默认）。
    AccentPreset {
        id: "blue",
        label_en: "Windows Blue",
        label_zh: "Windows 蓝",
        light: RgbColor::new(0x00, 0x5F, 0xB8),
        dark: RgbColor::new(0x60, 0xCD, 0xFF),
    },
    // 青。
    AccentPreset {
        id: "teal",
        label_en: "Teal",
        label_zh: "青",
        light: RgbColor::new(0x03, 0x83, 0x87),
        dark: RgbColor::new(0x4C, 0xC2, 0xC4),
    },
    // 紫。
    AccentPreset {
        id: "purple",
        label_en: "Purple",
        label_zh: "紫",
        light: RgbColor::new(0x87, 0x64, 0xB8),
        dark: RgbColor::new(0xB1, 0x99, 0xD9),
    },
    // 绿。
    AccentPreset {
        id: "green",
        label_en: "Green",
        label_zh: "绿",
        light: RgbColor::new(0x10, 0x7C, 0x10),
        dark: RgbColor::new(0x6C, 0xCB, 0x5F),
    },
    // 橙。
    AccentPreset {
        id: "orange",
        label_en: "Orange",
        label_zh: "橙",
        light: RgbColor::new(0xCA, 0x50, 0x10),
        dark: RgbColor::new(0xF4, 0xA0, 0x6A),
    },
    // 洋红。
    AccentPreset {
        id: "magenta",
        label_en: "Magenta",
        label_zh: "洋红",
        light: RgbColor::new(0xC2, 0x39, 0xB3),
        dark: RgbColor::new(0xE3, 0x8F, 0xE0),
    },
];

/// 默认强调色 id。
pub const DEFAULT_ACCENT_ID: &str = "blue";

/// 默认预设（`ACCENT_PRESETS` 第一项，即 `blue`）。
const DEFAULT_PRESET: AccentPreset = ACCENT_PRESETS[0];

/// 预设按 id 查找；未知 id 回退到默认预设。
pub fn preset(id: &str) -> AccentPreset {
    ACCENT_PRESETS
        .iter()
        .find(|entry| entry.id == id)
        .copied()
        .unwrap_or(DEFAULT_PRESET)
}

/// 窗口底色：浅色主题（`window.background`，§3.1）。
const WINDOW_BACKGROUND_LIGHT: RgbColor = RgbColor::new(0xF3, 0xF3, 0xF3);
/// 窗口底色：深色主题（`window.background`，§3.1）。
const WINDOW_BACKGROUND_DARK: RgbColor = RgbColor::new(0x20, 0x20, 0x20);
/// 纯黑，用于 pressed 派生。
const BLACK: RgbColor = RgbColor::new(0x00, 0x00, 0x00);
/// 纯白，用于 hover 派生。
const WHITE: RgbColor = RgbColor::new(0xFF, 0xFF, 0xFF);
/// 基色上的文字在黑白之间切换的亮度阈值（规范 §3.4）。
const TEXT_LUMINANCE_THRESHOLD: f32 = 0.45;

/// 解析后的全部强调色（浅 / 深各一套 + 派生色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentColors {
    /// 浅色主题基色（主操作、选中指示）。
    pub light: RgbColor,
    /// 深色主题基色（主操作、选中指示）。
    pub dark: RgbColor,
    /// 浅色主题悬停色。
    pub hover_light: RgbColor,
    /// 深色主题悬停色。
    pub hover_dark: RgbColor,
    /// 浅色主题按下色。
    pub pressed_light: RgbColor,
    /// 深色主题按下色。
    pub pressed_dark: RgbColor,
    /// 浅色主题基色上的文字色：按对比度自动取黑或白。
    pub text_light: RgbColor,
    /// 深色主题基色上的文字色：按对比度自动取黑或白。
    pub text_dark: RgbColor,
    /// 浅色主题选中项浅底：基色与窗口底色混合后的实色。
    pub soft_light: RgbColor,
    /// 深色主题选中项浅底：基色与窗口底色混合后的实色。
    pub soft_dark: RgbColor,
}

/// 派生规则：
/// - `hover = mix(base, 白, 0.12)`，即向白混合 12%，略亮于基色；
/// - `pressed = mix(base, 黑, 0.12)`，即向黑混合 12%，略暗于基色；
/// - `text = relative_luminance(base) > 0.45` 时取 `#000000`，否则取 `#FFFFFF`；
/// - `soft = mix(base, 窗口底色, 0.16)`，浅色窗口底色 `#F3F3F3`、深色 `#202020`。
pub fn resolve_accent(id: &str) -> AccentColors {
    let base = preset(id);
    // 基色偏亮时用黑字，偏暗时用白字，保证对比度。
    let text_on = |color: RgbColor| {
        if color.relative_luminance() > TEXT_LUMINANCE_THRESHOLD {
            BLACK
        } else {
            WHITE
        }
    };

    AccentColors {
        light: base.light,
        dark: base.dark,
        hover_light: base.light.mix(WHITE, 0.12),
        hover_dark: base.dark.mix(WHITE, 0.12),
        pressed_light: base.light.mix(BLACK, 0.12),
        pressed_dark: base.dark.mix(BLACK, 0.12),
        text_light: text_on(base.light),
        text_dark: text_on(base.dark),
        soft_light: base.light.mix(WINDOW_BACKGROUND_LIGHT, 0.16),
        soft_dark: base.dark.mix(WINDOW_BACKGROUND_DARK, 0.16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 浮点断言辅助：避免直接比较浮点相等。
    fn assert_close(actual: f32, expected: f32, tolerance: f32) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected} ± {tolerance}, got {actual}"
        );
    }

    #[test]
    fn theme_mode_from_id_accepts_known_ids() {
        assert_eq!(ThemeMode::from_id("system"), Some(ThemeMode::System));
        assert_eq!(ThemeMode::from_id("light"), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::from_id("dark"), Some(ThemeMode::Dark));
    }

    #[test]
    fn theme_mode_from_id_is_case_insensitive_and_trims() {
        assert_eq!(ThemeMode::from_id(" SyStEm "), Some(ThemeMode::System));
        assert_eq!(ThemeMode::from_id("\tLIGHT\n"), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::from_id(" Dark"), Some(ThemeMode::Dark));
    }

    #[test]
    fn theme_mode_from_id_rejects_unknown_ids() {
        assert_eq!(ThemeMode::from_id(""), None);
        assert_eq!(ThemeMode::from_id("auto"), None);
        assert_eq!(ThemeMode::from_id("dark mode"), None);
    }

    #[test]
    fn theme_mode_id_round_trips_and_defaults_to_system() {
        for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
            assert_eq!(ThemeMode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(ThemeMode::default(), ThemeMode::System);
    }

    #[test]
    fn rgb_color_mix_returns_endpoints_exactly() {
        let black = RgbColor::new(0x00, 0x00, 0x00);
        let white = RgbColor::new(0xFF, 0xFF, 0xFF);
        assert_eq!(black.mix(white, 0.0), black);
        assert_eq!(black.mix(white, 1.0), white);
        assert_eq!(
            RgbColor::new(0x12, 0x34, 0x56).mix(RgbColor::new(0x65, 0x43, 0x21), 0.0),
            RgbColor::new(0x12, 0x34, 0x56)
        );
    }

    #[test]
    fn rgb_color_mix_clamps_out_of_range_t() {
        let black = RgbColor::new(0x00, 0x00, 0x00);
        let white = RgbColor::new(0xFF, 0xFF, 0xFF);
        assert_eq!(black.mix(white, -0.5), black);
        assert_eq!(black.mix(white, 1.5), white);
    }

    #[test]
    fn rgb_color_mix_interpolates_and_rounds() {
        let black = RgbColor::new(0x00, 0x00, 0x00);
        let white = RgbColor::new(0xFF, 0xFF, 0xFF);
        assert_eq!(black.mix(white, 0.5), RgbColor::new(0x80, 0x80, 0x80));
        assert_eq!(
            RgbColor::new(10, 20, 30).mix(RgbColor::new(20, 40, 60), 0.25),
            RgbColor::new(13, 25, 38)
        );
    }

    #[test]
    fn rgb_color_relative_luminance_matches_reference_points() {
        assert_close(
            RgbColor::new(0x00, 0x00, 0x00).relative_luminance(),
            0.0,
            1e-6,
        );
        assert_close(
            RgbColor::new(0xFF, 0xFF, 0xFF).relative_luminance(),
            1.0,
            1e-6,
        );
        // 中灰在线性空间位于两端点之间（#808080 ≈ 0.2159）。
        let mid_gray = RgbColor::new(0x80, 0x80, 0x80).relative_luminance();
        assert_close(mid_gray, 0.2159, 1e-3);
        assert!(mid_gray > 0.0 && mid_gray < 1.0);
    }

    #[test]
    fn rgb_color_to_hex_uses_lowercase() {
        assert_eq!(RgbColor::new(0x00, 0x5F, 0xB8).to_hex(), "#005fb8");
        assert_eq!(RgbColor::new(0x60, 0xCD, 0xFF).to_hex(), "#60cdff");
        assert_eq!(RgbColor::new(0x00, 0x00, 0x00).to_hex(), "#000000");
    }

    #[test]
    fn accent_presets_match_spec_table() {
        assert_eq!(ACCENT_PRESETS.len(), 6);

        let expected = [
            (
                "blue",
                "Windows Blue",
                "Windows 蓝",
                RgbColor::new(0x00, 0x5F, 0xB8),
                RgbColor::new(0x60, 0xCD, 0xFF),
            ),
            (
                "teal",
                "Teal",
                "青",
                RgbColor::new(0x03, 0x83, 0x87),
                RgbColor::new(0x4C, 0xC2, 0xC4),
            ),
            (
                "purple",
                "Purple",
                "紫",
                RgbColor::new(0x87, 0x64, 0xB8),
                RgbColor::new(0xB1, 0x99, 0xD9),
            ),
            (
                "green",
                "Green",
                "绿",
                RgbColor::new(0x10, 0x7C, 0x10),
                RgbColor::new(0x6C, 0xCB, 0x5F),
            ),
            (
                "orange",
                "Orange",
                "橙",
                RgbColor::new(0xCA, 0x50, 0x10),
                RgbColor::new(0xF4, 0xA0, 0x6A),
            ),
            (
                "magenta",
                "Magenta",
                "洋红",
                RgbColor::new(0xC2, 0x39, 0xB3),
                RgbColor::new(0xE3, 0x8F, 0xE0),
            ),
        ];

        for (entry, (id, label_en, label_zh, light, dark)) in ACCENT_PRESETS.iter().zip(expected) {
            assert_eq!(entry.id, id);
            assert_eq!(entry.label_en, label_en);
            assert_eq!(entry.label_zh, label_zh);
            assert_eq!(entry.light, light, "{id} light");
            assert_eq!(entry.dark, dark, "{id} dark");
        }

        // 展示顺序与规范表格一致。
        let ids: Vec<&str> = ACCENT_PRESETS.iter().map(|entry| entry.id).collect();
        assert_eq!(
            ids,
            ["blue", "teal", "purple", "green", "orange", "magenta"]
        );
    }

    #[test]
    fn accent_preset_ids_are_unique() {
        let mut ids: Vec<&str> = ACCENT_PRESETS.iter().map(|entry| entry.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total);
    }

    #[test]
    fn preset_lookup_returns_matching_entry_and_falls_back_to_default() {
        for expected in ACCENT_PRESETS {
            assert_eq!(preset(expected.id), expected);
        }

        let default = preset(DEFAULT_ACCENT_ID);
        assert_eq!(default, DEFAULT_PRESET);
        assert_eq!(default.id, DEFAULT_ACCENT_ID);
        assert_eq!(preset("nope"), default);
        assert_eq!(preset(""), default);
        // id 精确匹配：大小写不归一化，未知值一律回退。
        assert_eq!(preset("Blue"), default);
    }

    #[test]
    fn resolve_accent_blue_matches_derived_rules() {
        let accent = resolve_accent("blue");

        assert_eq!(accent.light, RgbColor::new(0x00, 0x5F, 0xB8));
        assert_eq!(accent.dark, RgbColor::new(0x60, 0xCD, 0xFF));

        // 深色基色 #60CDFF 亮度 > 0.45 → 黑字；浅色基色 → 白字。
        assert!(accent.dark.relative_luminance() > 0.45);
        assert!(accent.light.relative_luminance() < 0.45);
        assert_eq!(accent.text_dark, BLACK);
        assert_eq!(accent.text_light, WHITE);

        // hover 向白混合、pressed 向黑混合 → 亮度单调。
        assert!(accent.hover_light.relative_luminance() > accent.light.relative_luminance());
        assert!(accent.hover_dark.relative_luminance() > accent.dark.relative_luminance());
        assert!(accent.pressed_light.relative_luminance() < accent.light.relative_luminance());
        assert!(accent.pressed_dark.relative_luminance() < accent.dark.relative_luminance());

        // 锁定取整后的派生值。
        assert_eq!(accent.hover_light.to_hex(), "#1f72c1");
        assert_eq!(accent.pressed_light.to_hex(), "#0054a2");
        assert_eq!(accent.hover_dark.to_hex(), "#73d3ff");
        assert_eq!(accent.pressed_dark.to_hex(), "#54b4e0");
        assert_eq!(accent.soft_light.to_hex(), "#2777c1");
        assert_eq!(accent.soft_dark.to_hex(), "#56b1db");

        // soft = 基色与窗口底色 16% 混合。
        assert_eq!(
            accent.soft_light,
            accent.light.mix(WINDOW_BACKGROUND_LIGHT, 0.16)
        );
        assert_eq!(
            accent.soft_dark,
            accent.dark.mix(WINDOW_BACKGROUND_DARK, 0.16)
        );
    }

    #[test]
    fn resolve_accent_falls_back_to_default_preset() {
        assert_eq!(resolve_accent("nope"), resolve_accent(DEFAULT_ACCENT_ID));
        assert_eq!(resolve_accent(""), resolve_accent(DEFAULT_ACCENT_ID));
    }

    #[test]
    fn derived_tokens_keep_luminance_order_for_all_presets() {
        for entry in ACCENT_PRESETS {
            let accent = resolve_accent(entry.id);
            assert!(
                accent.hover_light.relative_luminance() > accent.light.relative_luminance(),
                "{} hover_light 应亮于基色",
                entry.id
            );
            assert!(
                accent.hover_dark.relative_luminance() > accent.dark.relative_luminance(),
                "{} hover_dark 应亮于基色",
                entry.id
            );
            assert!(
                accent.pressed_light.relative_luminance() < accent.light.relative_luminance(),
                "{} pressed_light 应暗于基色",
                entry.id
            );
            assert!(
                accent.pressed_dark.relative_luminance() < accent.dark.relative_luminance(),
                "{} pressed_dark 应暗于基色",
                entry.id
            );
        }
    }
}
