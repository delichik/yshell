//! Active terminal input, resize, scroll and selection behaviour.

use tempfile::tempdir;

use super::*;

#[test]
fn active_terminal_input_flows_through_shell_boundary() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    let projection = runtime
        .send_active_terminal_input("pwd\n")
        .expect("send input");

    assert!(projection.status_text.contains("Sent"));
    assert!(projection
        .terminal_body_text
        .contains("fake-shell received input"));
}

#[test]
fn terminal_projection_exposes_grid_lines_and_cursor_position() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    let projection = runtime
        .send_active_terminal_input("echo grid\n")
        .expect("send input");

    assert!(projection
        .terminal_visible_lines
        .iter()
        .any(|line| line.contains("fake-shell received input")));
    assert!(projection.terminal_cursor_row >= 0);
    assert!(projection.terminal_cursor_column >= 0);
}

#[test]
fn active_terminal_resize_flows_through_shell_boundary() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    let projection = runtime
        .resize_active_terminal(100, 40)
        .expect("resize terminal");

    assert!(projection.status_text.contains("100x40"));
    assert!(projection.terminal_body_text.contains("fake-shell resized"));
}

#[test]
fn passive_terminal_resize_skips_duplicate_dimensions() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    let first = runtime
        .sync_active_terminal_size_passive(100, 40)
        .expect("first passive resize");
    assert!(first.is_some());

    let second = runtime
        .sync_active_terminal_size_passive(100, 40)
        .expect("second passive resize");
    assert!(second.is_none());
}

#[test]
fn terminal_key_input_flows_through_the_shell_boundary() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    let typed = runtime
        .send_active_terminal_key("x", false, false, false, false)
        .expect("send key");
    assert!(typed.status_text.contains("Sent 1 bytes"));
    assert!(typed
        .terminal_body_text
        .contains("fake-shell received input: x"));

    let entered = runtime
        .send_active_terminal_key("\u{000a}", false, false, false, false)
        .expect("send enter");
    assert!(entered
        .terminal_body_text
        .contains("fake-shell received input: \\r"));

    let control = runtime
        .send_active_terminal_key("c", true, false, false, false)
        .expect("send ctrl+c");
    assert!(control.status_text.contains("Sent 1 bytes"));
}

#[test]
fn active_terminal_resize_keeps_the_render_grid_in_sync() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");

    let projection = runtime.resize_active_terminal(50, 12).expect("resize");

    assert_eq!(projection.terminal_visible_lines.len(), 12);
    let metrics = runtime
        .active_terminal_viewport_metrics()
        .expect("viewport metrics");
    assert_eq!(metrics.columns, 50);
    assert_eq!(metrics.rows, 12);
}

#[test]
fn terminal_scroll_and_selection_round_trip_through_the_projection() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let _ = runtime
        .resize_active_terminal(24, 4)
        .expect("narrow the terminal");
    let _ = runtime
        .send_active_terminal_input("one\ntwo\nthree\nfour\nfive\n")
        .expect("feed output");
    let frame_before = runtime.projection().terminal_frame_id;
    assert!(
        runtime
            .active_terminal_viewport_metrics()
            .expect("metrics")
            .top_absolute_row
            > 0
    );

    let top = runtime
        .active_terminal_viewport_metrics()
        .expect("metrics")
        .top_absolute_row;
    let _ = runtime
        .begin_active_terminal_selection(0, u16::try_from(top).expect("row fits"))
        .expect("begin selection");
    let selected = runtime
        .update_active_terminal_selection(2, u16::try_from(top).expect("row fits"))
        .expect("extend selection");
    assert!(selected.terminal_selection_active);
    assert!(selected.terminal_frame_id > frame_before);

    let copied = runtime
        .copy_active_terminal_selection()
        .expect("copy selection");
    assert!(copied.status_text.contains("Copied"));
    assert!(runtime.terminal_clipboard_text().chars().count() >= 3);

    let scrolled = runtime.scroll_active_terminal(1).expect("scroll back");
    assert_eq!(scrolled.terminal_scroll_offset, 1);

    let all = runtime
        .select_all_active_terminal()
        .expect("select all terminal");
    assert!(all.terminal_selection_active);
    assert!(runtime.active_terminal_selection_text().contains("one"));

    let bottom = runtime
        .scroll_active_terminal_to_bottom()
        .expect("scroll to bottom");
    assert_eq!(bottom.terminal_scroll_offset, 0);
}

#[test]
fn terminal_scrollbar_projection_and_scroll_to_line_clamp_at_both_ends() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    let _ = runtime
        .resize_active_terminal(24, 4)
        .expect("narrow the terminal");
    let _ = runtime
        .send_active_terminal_input("one\ntwo\nthree\nfour\nfive\n")
        .expect("feed output");

    let projection = runtime.projection();
    let rows = projection.terminal_viewport_rows;
    let total = projection.terminal_scrollback_lines;
    assert_eq!(rows, 4);
    assert!(total > rows, "expected scrollback lines, got {total} lines");
    let max_offset = total - rows;

    // Line 0 pins the viewport to the oldest line: the scroll offset is at its max.
    let top = runtime
        .scroll_active_terminal_to_line(0)
        .expect("scroll to top");
    assert_eq!(
        top.terminal_scroll_offset,
        u64::try_from(max_offset).expect("max offset fits")
    );
    assert_eq!(
        runtime
            .active_terminal_viewport_metrics()
            .expect("metrics")
            .top_absolute_row,
        0
    );

    // Past the top / past the bottom both clamp instead of wrapping around.
    let above_top = runtime
        .scroll_active_terminal_to_line(-5)
        .expect("clamp above the top");
    assert_eq!(above_top.terminal_scroll_offset, top.terminal_scroll_offset);
    let below_bottom = runtime
        .scroll_active_terminal_to_line(max_offset + 99)
        .expect("clamp below the bottom");
    assert_eq!(below_bottom.terminal_scroll_offset, 0);

    // The scrollbar's bottom-most line is the live view (offset 0).
    let last_line = runtime
        .scroll_active_terminal_to_line(max_offset)
        .expect("scroll to the last line");
    assert_eq!(last_line.terminal_scroll_offset, 0);

    // A line in the middle positions the viewport top exactly on it.
    let middle = max_offset / 2;
    let scrolled = runtime
        .scroll_active_terminal_to_line(middle)
        .expect("scroll to the middle line");
    assert_eq!(
        scrolled.terminal_scroll_offset,
        u64::try_from(max_offset - middle).expect("offset fits")
    );
    assert_eq!(
        runtime
            .active_terminal_viewport_metrics()
            .expect("metrics")
            .top_absolute_row,
        usize::try_from(middle).expect("line fits")
    );
    // The geometry projection keeps reporting the same content extent.
    assert_eq!(scrolled.terminal_scrollback_lines, total);
    assert_eq!(scrolled.terminal_viewport_rows, rows);
}

#[test]
fn copy_paste_clear_and_find_work_on_visible_terminal_text() {
    let temp = tempdir().expect("tempdir");
    let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
    runtime
        .handle_quick_connect("alice@example.com:2200")
        .expect("quick connect");
    runtime
        .send_active_terminal_input("echo secret\n")
        .expect("send input");

    let copied = runtime
        .copy_active_terminal_visible_text()
        .expect("copy visible terminal");
    assert!(copied.status_text.contains("Copied"));

    let found = runtime
        .find_in_active_terminal("secret")
        .expect("find in terminal");
    assert_eq!(found.terminal_search_kind_text, "found");
    assert_eq!(found.terminal_search_match_count, 1);
    assert_eq!(found.terminal_search_current_index, 1);
    assert_eq!(found.terminal_search_query_text, "secret");

    let next = runtime.select_next_terminal_match().expect("next match");
    assert_eq!(next.terminal_search_kind_text, "found");
    assert_eq!(next.terminal_search_current_index, 1);

    let previous = runtime
        .select_previous_terminal_match()
        .expect("previous match");
    assert_eq!(previous.terminal_search_kind_text, "found");
    assert_eq!(previous.terminal_search_current_index, 1);

    let cleared = runtime.clear_active_terminal().expect("clear terminal");
    assert!(cleared.status_text.contains("Cleared"));
    assert!(cleared.terminal_body_text.contains("Terminal view cleared"));

    let pasted = runtime.paste_terminal_clipboard().expect("paste clipboard");
    assert!(pasted.status_text.contains("Sent"));
    assert!(pasted.terminal_body_text.contains("echo secret"));
}
