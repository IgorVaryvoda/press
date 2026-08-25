fn main() {
    println!("cargo:rerun-if-changed=src/avif_bridge.c");
    println!("cargo:rerun-if-changed=src/jxl_bridge.c");

    if std::env::var_os("VCPKGRS_TRIPLET").is_some() {
        let avif = vcpkg::Config::new()
            .cargo_metadata(false)
            .find_package("libavif")
            .expect("libavif is required; install libavif[aom,dav1d] with vcpkg");
        let jxl = vcpkg::Config::new()
            .cargo_metadata(false)
            .find_package("libjxl")
            .expect("libjxl is required; install libjxl with vcpkg");
        let include_paths = avif
            .include_paths
            .iter()
            .chain(&jxl.include_paths)
            .cloned()
            .collect::<Vec<_>>();
        compile_bridges(&include_paths);
        for line in avif.cargo_metadata.into_iter().chain(jxl.cargo_metadata) {
            println!("{line}");
        }
        return;
    }

    let avif = pkg_config::Config::new()
        .atleast_version("1.0")
        .cargo_metadata(false)
        .probe("libavif")
        .expect("libavif >= 1.0 is required; install libavif-dev or libavif");
    let jxl = pkg_config::Config::new()
        .atleast_version("0.7")
        .cargo_metadata(false)
        .probe("libjxl")
        .expect("libjxl >= 0.7 is required; install libjxl-dev or libjxl");
    let include_paths = avif
        .include_paths
        .iter()
        .chain(&jxl.include_paths)
        .cloned()
        .collect::<Vec<_>>();
    compile_bridges(&include_paths);

    // Emit the dynamic libraries after the static bridges so linkers using
    // --as-needed retain them.
    pkg_config::Config::new()
        .atleast_version("1.0")
        .probe("libavif")
        .expect("libavif disappeared between configure and link");
    pkg_config::Config::new()
        .atleast_version("0.7")
        .probe("libjxl")
        .expect("libjxl disappeared between configure and link");
}

fn compile_bridges(include_paths: &[std::path::PathBuf]) {
    let mut bridge = cc::Build::new();
    bridge.files(["src/avif_bridge.c", "src/jxl_bridge.c"]);
    for include in include_paths {
        bridge.include(include);
    }
    bridge.compile("press_codec_bridges");
}
