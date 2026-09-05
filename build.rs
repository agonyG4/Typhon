fn main() {
    pkg_config::Config::new()
        .atleast_version("1.10.0")
        .probe("xkbcommon")
        .unwrap_or_else(|error| {
            panic!("Typhon requires libxkbcommon >= 1.10.0: {error}");
        });
}
