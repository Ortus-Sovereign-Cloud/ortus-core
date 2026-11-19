//! Checks that the web client has been built before it is embedded.

fn main() {
    println!("cargo:rerun-if-changed=web/dist");
    if !std::path::Path::new("web/dist/index.html").exists() {
        panic!("the web client is not built: run `npm ci && npm run build` in web/ first");
    }
}
