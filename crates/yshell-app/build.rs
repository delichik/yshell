fn main() {
    // Cargo runs build scripts on the platform's main-thread stack. On Windows
    // MSVC that is ~1 MB, and slint-build's recursive parser/layout passes over
    // the full UI tree can exhaust it (STATUS_STACK_OVERFLOW, 0xc00000fd).
    // Give the build work a dedicated large stack so the host OS stack limit is
    // irrelevant; the final binary's stack is configured separately below.
    let result = std::thread::Builder::new()
        .name("yshell-build-script".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(run)
        .expect("spawn yshell build-script thread")
        .join();
    if result.is_err() {
        std::process::exit(1);
    }
}

fn run() {
    // slint-build emits rerun-if-changed for the .slint files it compiles, but
    // not for the .po files it embeds: without this, editing translations would
    // leave a stale translation table in the binary.
    println!("cargo:rerun-if-changed=../../translations");

    // Windows MSVC defaults the main thread to ~1MB; deep Slint trees (terminal
    // split) can overflow it. Match the common 8MB Unix default as a safety net
    // alongside conditional TerminalView mounting in workspace_split.slint.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some()
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }

    // Pin the widget style so the built-in controls (Button/LineEdit/...) do not
    // drift with the host platform: the design language is WinUI 3 / Fluent 2.
    // Bundle the gettext translations at compile time so the portable build can
    // switch languages at runtime without any file IO.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_bundled_translations("../../translations");
    slint_build::compile_with_config("../../ui/main_window.slint", config)
        .expect("compile Slint UI");
}
