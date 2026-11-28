fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux")
        || std::env::var_os("CARGO_FEATURE_EMBEDDED").is_none()
    {
        return;
    }
    println!("cargo:rerun-if-changed=src/native.c");
    let mpv = pkg_config::Config::new()
        .atleast_version("2.5.0")
        .probe("mpv")
        .expect("libmpv 2.5.0 development headers are required");
    let mut build = cc::Build::new();
    build.file("src/native.c").includes(mpv.include_paths);
    if std::env::var_os("CARGO_FEATURE_EGL").is_some() {
        for library in ["egl", "gl"] {
            let gl = pkg_config::probe_library(library).expect("EGL and OpenGL are required");
            build.includes(gl.include_paths);
        }
        build.define("SPARROW_EGL", None);
    }
    build.compile("sparrow_mpv_native");
}
