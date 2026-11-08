fn main() {
    // The widget style is fixed at compile time in Slint. "cosmic" follows
    // the desktop's light/dark setting and accent colour at runtime (read
    // from the xdg-desktop-portal Settings interface), so it blends in on
    // COSMIC and still looks coherent on KDE, GNOME and others.
    // SLINT_STYLE=fluent (etc.) at build time overrides this for testing.
    let style = std::env::var("SLINT_STYLE").unwrap_or_else(|_| "cosmic".into());
    let config = slint_build::CompilerConfiguration::new().with_style(style);
    slint_build::compile_with_config("ui/app.slint", config).expect("compiling Slint UI");
    println!("cargo:rerun-if-env-changed=SLINT_STYLE");
}
