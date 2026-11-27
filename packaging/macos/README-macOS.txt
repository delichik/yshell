gTerm for macOS — First Launch Guide / 首次打开说明
==================================================

English
-------
This build is signed ad-hoc and is not yet notarized with an Apple Developer ID.
Because macOS Gatekeeper blocks downloaded apps that are not notarized, Finder
may report that "gTerm.app" is damaged or cannot be verified.

To open gTerm:
1. Drag gTerm.app into the Applications folder.
2. Open Terminal and run:

     xattr -dr com.apple.quarantine /Applications/gTerm.app

3. Open gTerm from Applications or Launchpad as usual.

中文说明
--------
当前安装包为自签名（ad-hoc）构建，尚未经过 Apple Developer ID 签名与公证。
macOS Gatekeeper 会拦截未公证的下载应用，因此可能会提示“已损坏，无法打开”或
“无法验证开发者”。

打开步骤：
1. 将 gTerm.app 拖入“应用程序（Applications）”文件夹。
2. 打开“终端（Terminal）”并执行以下命令移除隔离属性：

     xattr -dr com.apple.quarantine /Applications/gTerm.app

3. 之后即可正常双击或从启动台打开 gTerm。
