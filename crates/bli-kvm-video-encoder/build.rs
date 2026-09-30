fn main() {
    println!("cargo:rerun-if-env-changed=BLI_KVM_LIBVPX_DIR");
    println!("cargo:rerun-if-changed=native/vp8_shim.c");
    #[cfg(feature = "vp8")]
    {
        let root = std::path::PathBuf::from(
            std::env::var_os("BLI_KVM_LIBVPX_DIR")
                .expect("BLI_KVM_LIBVPX_DIR must name a scripts/build-libvpx.sh output"),
        );
        assert!(root.is_absolute(), "BLI_KVM_LIBVPX_DIR must be absolute");
        cc::Build::new()
            .file("native/vp8_shim.c")
            .include(root.join("include"))
            .warnings(true)
            .extra_warnings(true)
            .warnings_into_errors(true)
            .compile("bk_vp8_shim");
        println!(
            "cargo:rustc-link-search=native={}",
            root.join("lib").display()
        );
        println!("cargo:rustc-link-lib=static=vpx");
    }
    println!("cargo:rerun-if-env-changed=BLI_KVM_X264_DIR");
    println!("cargo:rerun-if-changed=native/h264_shim.c");
    #[cfg(feature = "h264")]
    {
        let root = std::path::PathBuf::from(
            std::env::var_os("BLI_KVM_X264_DIR")
                .expect("BLI_KVM_X264_DIR must name a scripts/build-x264.sh output"),
        );
        assert!(root.is_absolute(), "BLI_KVM_X264_DIR must be absolute");
        cc::Build::new()
            .file("native/h264_shim.c")
            .include(root.join("include"))
            .warnings(true)
            .extra_warnings(true)
            .warnings_into_errors(true)
            .compile("bk_h264_shim");
        println!(
            "cargo:rustc-link-search=native={}",
            root.join("lib").display()
        );
        println!("cargo:rustc-link-lib=static=x264");
        // musl keeps threads and maths in libc; glibc hosts need them named.
        if !std::env::var("TARGET").unwrap_or_default().contains("musl") {
            println!("cargo:rustc-link-lib=pthread");
            println!("cargo:rustc-link-lib=m");
        }
    }
}
