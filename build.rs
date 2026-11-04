use std::fs;

fn main() {
    // RC.EXE chokes on non-ASCII absolute paths, so keep everything relative to the crate root.
    fs::write("app.rc", "1 ICON \"assets/app.ico\"
1 24 \"app.manifest\"
").unwrap();

    embed_resource::compile("app.rc", embed_resource::NONE).manifest_required().unwrap();
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/app.ico");
}
