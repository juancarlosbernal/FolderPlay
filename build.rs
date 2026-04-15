fn main() {
    glib_build_tools::compile_resources(
        &["src", "data", "."],
        "src/folderplay.gresource.xml",
        "folderplay.gresource",
    );
}
