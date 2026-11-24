fn main() {
    #[cfg(windows)]
    {
        // The exe's version resource, taken from Cargo.toml so the number has a
        // single source: "0.3.1" becomes the string, 0,3,1,0 the tuple.
        let mut parts: Vec<u32> = env!("CARGO_PKG_VERSION")
            .split('.')
            .map(|part| part.parse().expect("numeric version component"))
            .collect();
        parts.resize(4, 0);
        let commas = parts
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        embed_resource::compile(
            "assets/guardian.rc",
            [format!("VERSION_COMMAS={commas}"),
             format!("VERSION_STRING=\"{}\"", env!("CARGO_PKG_VERSION"))],
        )
        .manifest_optional()
        .expect("embed guardian.rc");
        println!("cargo:rerun-if-changed=assets/guardian.rc");
        println!("cargo:rerun-if-changed=assets/guardian.ico");
        println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
    }
}
