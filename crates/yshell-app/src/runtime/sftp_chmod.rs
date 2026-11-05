//! SFTP chmod 窗（compact `560×400`）的纯逻辑。
//!
//! 原型：[`docs/prototypes/sftp-chmod/index.html`]。
//! 窗口本身（`ui/windows/sftp_chmod_window.slint`）用 Slint 做八进制输入与
//! 9 个勾选框的双向同步；这里提供宿主侧同语义的校验/转换函数：
//! 提交时把八进制文本解析成 mode，供既有 `chmod_sftp_selected` / `chmod_sftp_path`
//! 使用。入口（SFTP 行右键）在波 3-G 接线，故本模块的转换函数目前只被单测与
//! 后续接线使用。
//!
//! 位序与原型表格一致：Owner / Group / Other × Read(4) / Write(2) / Execute(1)。
#![allow(dead_code)] // 接线在波 3-G：SFTP 行右键 → sftp-chmod 窗

use crate::error::{AppError, AppResult};

use super::{AppProjection, AppRuntime};

/// 九个权限位（`[owner-r, owner-w, owner-x, group-r, ... other-x]`）。
pub(crate) const PERMISSION_BIT_COUNT: usize = 9;

/// 一位权限对应的八进制权重（按 `index % 3`）。
const BIT_WEIGHTS: [u32; 3] = [4, 2, 1];

/// 单个八进制数字里最高允许的位（0..=7）。
const MAX_OWNER_DIGIT: u32 = 7;

/// 去掉非八进制字符、限制为最多 3 位（与原型输入框的 `maxlength=3` 一致）。
#[must_use]
pub(crate) fn sanitize_octal_text(text: &str) -> String {
    text.chars()
        .filter(|ch| matches!(ch, '0'..='7'))
        .take(3)
        .collect()
}

/// 解析三位八进制 mode（`755` / `0o755`）；不足三位或越界都报错。
pub(crate) fn parse_octal_mode(text: &str) -> AppResult<u32> {
    let trimmed = text.trim();
    let octal = trimmed
        .strip_prefix("0o")
        .or_else(|| trimmed.strip_prefix("0O"))
        .unwrap_or(trimmed);
    let sanitized = sanitize_octal_text(octal);
    if sanitized != octal || octal.len() != 3 {
        return Err(AppError::new(
            "permissions must be a three-digit octal value such as 644 or 755",
        ));
    }
    u32::from_str_radix(&sanitized, 8)
        .map_err(|_| AppError::new("permissions must be valid octal digits (0-7)"))
}

/// mode → 三位八进制文本（`0o755` 的展示形态始终是 `755`）。
#[must_use]
pub(crate) fn format_octal_mode(mode: u32) -> String {
    format!("{mode:03o}")
}

/// mode 的三个数字（owner / group / other），已按 8 取模。
fn mode_digits(mode: u32) -> [u32; 3] {
    [
        (mode >> 6) & MAX_OWNER_DIGIT,
        (mode >> 3) & MAX_OWNER_DIGIT,
        mode & MAX_OWNER_DIGIT,
    ]
}

/// mode → 9 个勾选位（供宿主断言/测试；Slint 窗口内自行同步）。
#[must_use]
pub(crate) fn mode_to_permission_bits(mode: u32) -> [bool; PERMISSION_BIT_COUNT] {
    let mut bits = [false; PERMISSION_BIT_COUNT];
    for (class, digit) in mode_digits(mode).into_iter().enumerate() {
        for (offset, weight) in BIT_WEIGHTS.into_iter().enumerate() {
            bits[class * 3 + offset] = digit & weight != 0;
        }
    }
    bits
}

/// 9 个勾选位 → mode；`bits` 长度不足 9 时按缺失为 false 处理。
#[must_use]
pub(crate) fn permission_bits_to_mode(bits: &[bool]) -> u32 {
    let mut mode = 0u32;
    for class in 0..3 {
        let mut digit = 0u32;
        for (offset, weight) in BIT_WEIGHTS.into_iter().enumerate() {
            if bits.get(class * 3 + offset).copied().unwrap_or(false) {
                digit |= weight;
            }
        }
        mode |= digit << (6 - (class as u32) * 3);
    }
    mode
}

/// 解析 → 回写 9 个位：窗口打开时的默认态（当前文件 mode）。
#[must_use]
pub(crate) fn bits_from_octal_text(text: &str) -> [bool; PERMISSION_BIT_COUNT] {
    parse_octal_mode(text)
        .map(mode_to_permission_bits)
        .unwrap_or([false; PERMISSION_BIT_COUNT])
}

// --- dialogs-closeout-a：SFTP chmod 窗的宿主状态机 ---------------------------
impl AppRuntime {
    /// 打开 SFTP chmod 窗：预填目标（单文件路径 / N 个选择）与当前八进制权限。
    pub fn open_sftp_chmod_dialog(&mut self) -> AppProjection {
        let selection = self.sftp_selected_paths();
        let count = selection.len();
        let (path_text, mode_text) = if count > 1 {
            (
                format!("{count} selected item(s)"),
                self.sftp_selected_permissions_text(),
            )
        } else if let Some(entry) = self.selected_sftp_entry() {
            (
                entry.path.clone(),
                format_octal_mode(entry.permissions & 0o777),
            )
        } else if let Some(path) = selection.first() {
            (path.clone(), self.sftp_selected_permissions_text())
        } else {
            self.set_status_kind(
                "sftp-chmod-requires-selection",
                "Select an SFTP entry before changing permissions.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        };
        let mode_text = if mode_text.is_empty() {
            "644".to_owned()
        } else {
            mode_text
        };
        self.sftp_chmod_visible = true;
        self.sftp_chmod_path_text = path_text;
        self.sftp_chmod_mode_text = mode_text;
        self.sftp_chmod_error_text.clear();
        self.sftp_chmod_selection_count = count.max(1);
        self.projection()
    }

    /// 八进制输入变更：仅做内联校验（勾选联动在 Slint 侧完成）。
    pub fn update_sftp_chmod_mode(&mut self, value: &str) -> AppProjection {
        self.sftp_chmod_mode_text = value.to_owned();
        self.sftp_chmod_error_text = if value.trim().is_empty() {
            String::new()
        } else {
            match parse_octal_mode(value) {
                Ok(_) => String::new(),
                Err(error) => error.to_string(),
            }
        };
        self.projection()
    }

    /// 应用（Cancel | OK 的唯一提交路径）：批量选择走 `chmod_sftp_selection`，
    /// 单文件走 `chmod_sftp_selected`；成功关闭窗口，失败留在窗内显示原因。
    pub fn apply_sftp_chmod_dialog(&mut self) -> AppProjection {
        let mode = match parse_octal_mode(&self.sftp_chmod_mode_text) {
            Ok(mode) => mode,
            Err(error) => {
                self.sftp_chmod_error_text = error.to_string();
                return self.projection();
            }
        };
        let octal = format_octal_mode(mode);
        let result = if self.sftp_chmod_selection_count > 1 {
            self.chmod_sftp_selection(&octal)
        } else {
            self.chmod_sftp_selected(&octal)
        };
        match result {
            Ok(projection) => {
                self.sftp_chmod_visible = false;
                self.sftp_chmod_error_text.clear();
                projection
            }
            Err(error) => {
                self.sftp_chmod_error_text = format!("Could not change permissions: {error}");
                self.set_status_kind(
                    "sftp-chmod-failed",
                    self.sftp_chmod_error_text.clone(),
                    error.to_string(),
                    String::new(),
                );
                self.projection()
            }
        }
    }

    /// Cancel / Esc / ×：关闭窗口且不应用。
    pub fn cancel_sftp_chmod_dialog(&mut self) -> AppProjection {
        self.sftp_chmod_visible = false;
        self.sftp_chmod_error_text.clear();
        self.projection()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_text_round_trips_through_permission_bits() {
        for (octal, expected_mode) in [("755", 0o755), ("644", 0o644), ("600", 0o600), ("000", 0)] {
            let mode = parse_octal_mode(octal).expect("valid octal");
            assert_eq!(mode, expected_mode);
            let bits = mode_to_permission_bits(mode);
            assert_eq!(permission_bits_to_mode(&bits), mode);
            assert_eq!(format_octal_mode(mode), octal);
        }
    }

    #[test]
    fn permission_bits_match_owner_group_other_layout() {
        // 755 = owner rwx / group r-x / other r-x
        let bits = mode_to_permission_bits(0o755);
        assert_eq!(
            bits,
            [
                true, true, true, // owner
                true, false, true, // group
                true, false, true, // other
            ]
        );
        // 只勾 Other execute + Group write + Owner read
        let mut custom = [false; PERMISSION_BIT_COUNT];
        custom[0] = true; // owner read 4
        custom[4] = true; // group write 2
        custom[8] = true; // other execute 1
        assert_eq!(permission_bits_to_mode(&custom), 0o421);
        assert_eq!(format_octal_mode(0o421), "421");
    }

    #[test]
    fn sanitize_octal_text_keeps_only_three_octal_digits() {
        // 输入框过滤只保留 0-7 且最多 3 位（`0o` 前缀由 parse_octal_mode 处理）。
        assert_eq!(sanitize_octal_text("0o755"), "075");
        assert_eq!(sanitize_octal_text("7x5y9"), "75");
        assert_eq!(sanitize_octal_text("12345"), "123");
        assert_eq!(sanitize_octal_text(""), "");
    }

    #[test]
    fn parse_octal_mode_rejects_bad_input() {
        for bad in ["", "75", "7555", "78", "0x1", "rwx"] {
            assert!(parse_octal_mode(bad).is_err(), "{bad} should be rejected");
        }
        assert_eq!(parse_octal_mode("0o600").expect("prefixed octal"), 0o600);
        assert_eq!(parse_octal_mode(" 640 ").expect("trimmed octal"), 0o640);
    }

    #[test]
    fn bits_from_octal_text_tolerates_invalid_input() {
        assert_eq!(bits_from_octal_text("644"), mode_to_permission_bits(0o644));
        assert_eq!(bits_from_octal_text("nope"), [false; PERMISSION_BIT_COUNT]);
    }

    // --- dialogs-closeout-a：窗口状态机 -------------------------------------

    fn test_runtime() -> (tempfile::TempDir, AppRuntime) {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        (temp, runtime)
    }

    #[test]
    fn chmod_dialog_requires_a_selection_then_validates_mode() {
        let (_temp, mut runtime) = test_runtime();
        // 没有 SFTP 选择：不打开窗口，只给状态提示。
        let projection = runtime.open_sftp_chmod_dialog();
        assert!(!projection.sftp_chmod_visible);
        assert_eq!(runtime.status_kind, "sftp-chmod-requires-selection");

        // 宿主状态机：非法八进制 → 内联错误；合法 → 清除。
        runtime.sftp_chmod_visible = true;
        let projection = runtime.update_sftp_chmod_mode("9x");
        assert!(!projection.sftp_chmod_error_text.is_empty());
        let projection = runtime.apply_sftp_chmod_dialog();
        assert!(
            projection.sftp_chmod_visible,
            "invalid mode keeps the window open"
        );
        let projection = runtime.update_sftp_chmod_mode("755");
        assert_eq!(projection.sftp_chmod_error_text, "");
        let projection = runtime.cancel_sftp_chmod_dialog();
        assert!(!projection.sftp_chmod_visible);
    }

    #[test]
    fn close_and_quit_consequences_are_empty_without_open_tabs() {
        let (_temp, runtime) = test_runtime();
        let (rows, note) = runtime.close_tabs_consequences(&[], 0);
        assert!(rows.is_empty());
        assert_eq!(note, "");
        let (rows, note) = runtime.quit_consequences();
        assert!(rows.is_empty());
        assert_eq!(note, "");
    }
}
