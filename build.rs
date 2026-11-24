fn main() {
    #[cfg(windows)]
    {
        embed_resource::compile("assets/guardian.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("embed guardian.rc");
        println!("cargo:rerun-if-changed=assets/guardian.rc");
        println!("cargo:rerun-if-changed=assets/guardian.ico");
    }
}
