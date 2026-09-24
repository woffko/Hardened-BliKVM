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
}
