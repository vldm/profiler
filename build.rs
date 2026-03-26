fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let autodetect = std::env::var("CARGO_FEATURE_AUTO_DETECT").unwrap_or_default() == "1";

    if autodetect {
        // enable feature based on target os.
        if target_os == "macos" {
            println!("cargo:rustc-cfg=feature=\"kperf\"");
        } else if target_os == "linux" {
            println!("cargo:rustc-cfg=feature=\"perf_event\"");
        }
    }
}
