fn main() {
    println!(
        "cargo:rustc-env=RECAST_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
