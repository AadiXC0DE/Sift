fn main() {
    // Embed Google OAuth client id/secret at compile time (public for Desktop apps per Google docs).
    println!("cargo:rerun-if-env-changed=SIFT_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=SIFT_GOOGLE_CLIENT_SECRET");
    tauri_build::build()
}
