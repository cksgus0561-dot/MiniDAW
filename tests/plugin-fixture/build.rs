use std::{env, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let vendor = root.join("../../src-tauri/vendor");
    let out = root.join("../local/pdc-fixtures");
    std::fs::create_dir_all(&out).unwrap();
    let mut build = cc::Build::new();
    build.cpp(true);
    let mut cmd = build.get_compiler().to_command();
    cmd.current_dir(&out)
        .args(["/nologo", "/LD", "/MD", "/O2", "/EHsc", "/std:c++17"])
        .arg(format!("/I{}", vendor.display()))
        .arg(format!("/I{}", vendor.join("clap/include").display()))
        .arg(root.join("fixture.cpp"))
        .arg(vendor.join("pluginterfaces/base/funknown.cpp"))
        .arg("/Fe:fixture.dll")
        .arg("ole32.lib");
    assert!(cmd.status().unwrap().success());
    for ext in ["clap", "vst3"] {
        std::fs::copy(
            out.join("fixture.dll"),
            out.join(format!("PdcFixture.{ext}")),
        )
        .unwrap();
    }
    println!("cargo:rerun-if-changed=fixture.cpp");
}
