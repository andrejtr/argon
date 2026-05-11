fn main() {
    // Use CARGO_MANIFEST_DIR (absolute) so the linker script is found regardless
    // of the working directory cargo uses when building artifact dependencies.
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg=-T{dir}/../argon.ld");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rerun-if-changed={dir}/../argon.ld");
}
