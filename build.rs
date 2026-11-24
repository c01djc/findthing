fn main() {
    slint_build::compile("ui/launcher.slint").expect("编译界面失败");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows") {
        let _ = embed_resource::compile("brand/findthing.rc", embed_resource::NONE);
    }
}
