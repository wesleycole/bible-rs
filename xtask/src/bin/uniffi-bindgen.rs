//! `cargo run -p xtask --bin uniffi-bindgen -- generate --library <liblinger_ffi> --language swift --out-dir <dir>`
//!
//! UniFFI's generator, built for the host. It lives here rather than in `linger-ffi` so the CLI and
//! its dependencies are never compiled into the library shipped to iOS.

fn main() {
    uniffi::uniffi_bindgen_main()
}
