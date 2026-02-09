fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=dictator.rc");
        println!("cargo:rerun-if-changed=assets/dictator.ico");
        embed_resource::compile("dictator.rc", embed_resource::NONE);
    }
}
