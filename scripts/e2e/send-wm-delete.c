/* 向 X11 窗口发送 WM_DELETE_WINDOW（= 窗口管理器关闭按钮语义）。
 *
 * 为什么不用 `xdotool windowclose`：本仓 E2E 的 Xvfb 没有窗口管理器，而
 * Debian 的 xdotool 3.20160805 的 windowclose 在该环境下实测不生效（xterm 与
 * yshell 都不响应；怀疑其 WM_PROTOCOLS atom 处理）。这个 30 行 helper 直接
 * XSendEvent，实测 xterm 正常退出，可稳定驱动窗口关闭路径。
 *
 * 编译（run-ui-n7.sh 自动完成）：
 *   gcc -O2 -o "$outdir/n7-send-wm-delete" scripts/e2e/send-wm-delete.c -lX11
 * 用法：
 *   DISPLAY=:99 "$outdir/n7-send-wm-delete" <window-id>
 */
#include <X11/Xlib.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <window-id>\n", argv[0]);
        return 2;
    }
    Window window = (Window)strtoul(argv[1], NULL, 0);
    Display *display = XOpenDisplay(NULL);
    if (!display) {
        fprintf(stderr, "cannot open display\n");
        return 1;
    }
    Atom wm_protocols = XInternAtom(display, "WM_PROTOCOLS", False);
    Atom wm_delete = XInternAtom(display, "WM_DELETE_WINDOW", False);
    XEvent event;
    memset(&event, 0, sizeof(event));
    event.xclient.type = ClientMessage;
    event.xclient.message_type = wm_protocols;
    event.xclient.display = display;
    event.xclient.window = window;
    event.xclient.format = 32;
    event.xclient.data.l[0] = (long)wm_delete;
    event.xclient.data.l[1] = CurrentTime;
    Status status = XSendEvent(display, window, False, NoEventMask, &event);
    XFlush(display);
    XCloseDisplay(display);
    return status == 0 ? 1 : 0;
}
