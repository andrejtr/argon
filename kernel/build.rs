fn main() {
    println!("cargo:rerun-if-changed=../userland/argon-user/src");
    println!("cargo:rerun-if-changed=../userland/shell/src");
}
