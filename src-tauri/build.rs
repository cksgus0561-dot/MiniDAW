fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=src/plugins/bridge.cpp");
        println!("cargo:rerun-if-changed=vendor/pluginterfaces");
        println!("cargo:rerun-if-changed=vendor/clap/include");
        cc::Build::new().cpp(true).std("c++17").opt_level(2)
            .flag_if_supported("/EHsc").define("NDEBUG", None)
            .include("vendor").include("vendor/clap/include")
            .file("src/plugins/bridge.cpp")
            .file("vendor/pluginterfaces/base/funknown.cpp")
            .compile("minidaw_plugins");
        println!("cargo:rustc-link-lib=ole32");
        println!("cargo:rustc-link-lib=user32");
    }
    if std::env::var_os("CARGO_FEATURE_ASIO").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
    {
        let sdk = std::path::PathBuf::from(
            std::env::var_os("CPAL_ASIO_DIR")
                .expect("Set CPAL_ASIO_DIR for an ASIO build (scripts/with-asio.ps1)."),
        );
        assert!(
            sdk.join("common/asio.h").is_file(),
            "CPAL_ASIO_DIR must contain common/asio.h"
        );
        println!("cargo:rerun-if-env-changed=CPAL_ASIO_DIR");
        println!("cargo:rerun-if-changed=src/audio/asio_bridge.cpp");
        cc::Build::new()
            .cpp(true)
            .include(sdk.join("common"))
            .file("src/audio/asio_bridge.cpp")
            .compile("minidaw_asio_bridge");
    }
    println!("cargo:rerun-if-changed=src/audio/stretch_bridge.cpp");
    println!("cargo:rerun-if-changed=vendor/signalsmith");
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .opt_level(2)
        .include("vendor/signalsmith")
        .file("src/audio/stretch_bridge.cpp")
        .compile("minidaw_stretch");
    tauri_build::build();
}
