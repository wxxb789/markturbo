fn main() {
    println!("cargo:rerun-if-changed=resources/windows/markturbo.rc");
    println!("cargo:rerun-if-changed=resources/icons/markturbo.ico");

    // An icon is not allowed to disappear silently from a Windows release.
    // `NotWindows` is success; a Windows target without a working resource
    // compiler is a build error.
    embed_resource::compile("resources/windows/markturbo.rc", embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed the markturbo Windows icon");
}
