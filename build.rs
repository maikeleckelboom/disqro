fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/disqro.rc");
        println!("cargo:rerun-if-changed=assets/brand/disqro.ico");
        embed_resource::compile("assets/disqro.rc", embed_resource::NONE)
            .manifest_required()
            .expect("Windows application resources");
    }
}
