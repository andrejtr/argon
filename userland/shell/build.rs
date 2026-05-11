fn main() {
    println!("cargo:rustc-link-arg=-T../../userland/argon.ld");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rerun-if-changed=../../userland/argon.ld");
}
