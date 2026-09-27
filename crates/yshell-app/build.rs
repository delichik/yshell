fn main() {
    // slint-build emits rerun-if-changed for the .slint files it compiles, but
    // not for the .po files it embeds: without this, editing translations would
    // leave a stale translation table in the binary.
    println!("cargo:rerun-if-changed=../../translations");

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
