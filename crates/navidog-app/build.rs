fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=assets/logo.rc");
        println!("cargo:rerun-if-changed=assets/logo.ico");
        let _ = embed_resource::compile("assets/logo.rc", embed_resource::NONE).manifest_optional();
    }
}
