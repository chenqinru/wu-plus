fn main() {
    // A newly added or removed icon is not covered by existing include_bytes! dependencies.
    println!("cargo:rerun-if-changed=../../assets/icons");
}
