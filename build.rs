// Builds the icons (assets/snaprelay.rc) into the program.
fn main() {
    println!("cargo:rerun-if-changed=assets/snaprelay.rc");
    println!("cargo:rerun-if-changed=assets/icons");
    embed_resource::compile("assets/snaprelay.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
