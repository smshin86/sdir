fn main() {
    println!("cargo:rerun-if-changed=assets/sdir.ico");
    println!("cargo:rerun-if-changed=assets/sdir.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/sdir.rc", embed_resource::NONE)
            .manifest_required()
            .expect("embed the SDIR icon");
    }
}
