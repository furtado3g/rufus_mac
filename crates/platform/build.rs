use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=IMAGE2USB_WINDOWS_MEDIA_PREFIX");
    if env::var_os("CARGO_FEATURE_WINDOWS_MEDIA").is_none() {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_OS").unwrap(),
        "macos",
        "windows-media native integration is currently macOS-only"
    );
    assert_eq!(
        env::var("HOST").unwrap(),
        env::var("TARGET").unwrap(),
        "windows-media requires a native build, not cross-compilation"
    );
    let prefix = PathBuf::from(env::var_os("IMAGE2USB_WINDOWS_MEDIA_PREFIX").expect(
        "run sh scripts/build-windows-media.sh; set IMAGE2USB_WINDOWS_MEDIA_PREFIX to its absolute install directory",
    ));
    assert!(prefix.is_absolute(), "native prefix must be absolute");
    for name in ["libudfread.a", "libntfs-3g.a", "libi2u_mkntfs.a"] {
        let library = prefix.join("lib").join(name);
        assert!(library.is_file(), "missing {}", library.display());
        println!("cargo:rerun-if-changed={}", library.display());
    }
    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=udfread");
    println!("cargo:rustc-link-lib=static=ntfs-3g");
    println!("cargo:rustc-link-lib=static=i2u_mkntfs");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}
