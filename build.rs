fn main() {
    println!("cargo:rustc-check-cfg=cfg(clarion_exit_trampoline)");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
    {
        // The ELF startup object refers to main, while Rust's explicit process
        // exit refers to exit. Drain GPU owners before either enters libc's
        // atexit chain: Vulkan layers can register a newer cleanup per instance.
        println!("cargo:rustc-cfg=clarion_exit_trampoline");
        println!("cargo:rustc-link-arg=-Wl,--wrap=main");
        println!("cargo:rustc-link-arg=-Wl,--wrap=exit");
    }
    println!("cargo:rustc-check-cfg=cfg(have_libinput_plugin_system)");
    if pkg_config::Config::new()
        .atleast_version("1.30.0")
        .probe("libinput")
        .is_ok()
    {
        println!("cargo:rustc-cfg=have_libinput_plugin_system")
    }
}
