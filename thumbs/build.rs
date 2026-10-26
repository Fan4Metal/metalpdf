// The COM entry points DllGetClassObject and DllCanUnloadNow are found by
// the system through GetProcAddress, never linked against: no import
// library nor exports file is made (/NOIMPLIB, /NOEXP; making them, MSVC's
// linker said so on stdout, which cargo shows as a warning), and the
// warning LNK4104 that such exports belong out of the library (PRIVATE,
// which rustc cannot say) is silenced.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-cdylib-link-arg=/NOIMPLIB");
        println!("cargo:rustc-cdylib-link-arg=/NOEXP");
        println!("cargo:rustc-cdylib-link-arg=/IGNORE:4104");
    }
}
