//! 终端输入行为（Settings → Terminal）：中键 / 右键、多行粘贴策略、自动复制。
//!
//! 这里只做纯决策：不读剪贴板、不发字节、不碰 UI。调用点（bootstrap 的指针
//! 事件、runtime 的粘贴路径）负责执行，同时让规则可以单独单测。

use yshell_config::{MiddleClickAction, MultiLinePastePolicy, RightClickAction};

/// 终端表面一次点击应执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClickAction {
    /// 忽略这次点击。
    Ignore,
    /// 把剪贴板内容粘贴到当前会话。
    Paste,
    /// 在指针位置打开终端右键菜单。
    ContextMenu,
}

/// 中键点击语义。
#[allow(dead_code)] // 波 3 在终端指针事件里接线。
pub(crate) fn middle_click(action: MiddleClickAction) -> ClickAction {
    match action {
        MiddleClickAction::Paste => ClickAction::Paste,
        MiddleClickAction::DoNothing => ClickAction::Ignore,
    }
}

/// 右键点击语义。
#[allow(dead_code)] // 波 3 在终端指针事件里接线。
pub(crate) fn right_click(action: RightClickAction) -> ClickAction {
    match action {
        RightClickAction::ContextMenu => ClickAction::ContextMenu,
        RightClickAction::Paste => ClickAction::Paste,
        RightClickAction::DoNothing => ClickAction::Ignore,
    }
}

/// 粘贴策略对一次粘贴请求的裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasteDecision {
    /// 直接发送。
    Send,
    /// 先弹多行粘贴确认（确认窗在波 3 接线）。
    Confirm,
    /// 拒绝，且不发送任何字节。
    Block,
}

/// 文本是否跨多行（`\n` / `\r` / `\r\n` 都算）。
pub(crate) fn is_multi_line(text: &str) -> bool {
    text.contains('\n') || text.contains('\r')
}

/// 按配置的多行粘贴策略裁决这次粘贴。
pub(crate) fn paste_decision(policy: MultiLinePastePolicy, text: &str) -> PasteDecision {
    let multi_line = is_multi_line(text);
    match policy {
        MultiLinePastePolicy::Paste => PasteDecision::Send,
        MultiLinePastePolicy::Ask if multi_line => PasteDecision::Confirm,
        MultiLinePastePolicy::Block if multi_line => PasteDecision::Block,
        MultiLinePastePolicy::Ask | MultiLinePastePolicy::Block => PasteDecision::Send,
    }
}

/// 选区结束后是否应立即复制到剪贴板。
pub(crate) fn should_auto_copy(auto_copy: bool, has_selection: bool) -> bool {
    auto_copy && has_selection
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_click_maps_both_policies() {
        assert_eq!(middle_click(MiddleClickAction::Paste), ClickAction::Paste);
        assert_eq!(
            middle_click(MiddleClickAction::DoNothing),
            ClickAction::Ignore
        );
    }

    #[test]
    fn right_click_maps_all_three_policies() {
        assert_eq!(
            right_click(RightClickAction::ContextMenu),
            ClickAction::ContextMenu
        );
        assert_eq!(right_click(RightClickAction::Paste), ClickAction::Paste);
        assert_eq!(
            right_click(RightClickAction::DoNothing),
            ClickAction::Ignore
        );
    }

    #[test]
    fn multi_line_detection_covers_lf_cr_and_crlf() {
        assert!(!is_multi_line(""));
        assert!(!is_multi_line("echo hi"));
        assert!(is_multi_line("a\nb"));
        assert!(is_multi_line("a\rb"));
        assert!(is_multi_line("a\r\nb"));
        assert!(is_multi_line("a\n"));
    }

    #[test]
    fn ask_policy_confirms_only_multi_line_pastes() {
        assert_eq!(
            paste_decision(MultiLinePastePolicy::Ask, "single line"),
            PasteDecision::Send
        );
        assert_eq!(
            paste_decision(MultiLinePastePolicy::Ask, "one\ntwo"),
            PasteDecision::Confirm
        );
    }

    #[test]
    fn block_policy_blocks_only_multi_line_pastes() {
        assert_eq!(
            paste_decision(MultiLinePastePolicy::Block, "single line"),
            PasteDecision::Send
        );
        assert_eq!(
            paste_decision(MultiLinePastePolicy::Block, "one\ntwo"),
            PasteDecision::Block
        );
    }

    #[test]
    fn paste_policy_never_confirms_or_blocks() {
        for text in ["", "single", "one\ntwo"] {
            assert_eq!(
                paste_decision(MultiLinePastePolicy::Paste, text),
                PasteDecision::Send
            );
        }
    }

    #[test]
    fn auto_copy_requires_both_the_switch_and_a_selection() {
        assert!(should_auto_copy(true, true));
        assert!(!should_auto_copy(true, false));
        assert!(!should_auto_copy(false, true));
        assert!(!should_auto_copy(false, false));
    }
}
