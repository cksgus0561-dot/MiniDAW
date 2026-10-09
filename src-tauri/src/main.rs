#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|s|s=="--plugin-scan") {
        if let (Some(path),Some(format),Some(out))=(args.get(2),args.get(3),args.get(4)) {
            let result=minidaw_lib::plugins::scan_one(std::path::Path::new(path),format);
            let value=match result {Ok(plugins)=>serde_json::json!({"plugins":plugins}),Err(error)=>serde_json::json!({"error":error})};
            let _=std::fs::write(out,serde_json::to_vec(&value).unwrap());
        }
        return;
    }
    if let Err(error) = minidaw_lib::run() {
        eprintln!("MiniDAW를 시작할 수 없습니다: {error}");
        std::process::exit(1);
    }
}
