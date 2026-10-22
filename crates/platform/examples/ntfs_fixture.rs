//! Developer fixture generator for independent NTFS inspection.
//!
//! Usage: cargo run -p image2usb-platform --features windows-media \
//!            --example ntfs_fixture -- target/ntfs-fixture.img [size-mib]
//!
//! This example is not shipped and never opens a physical device.

use image2usb_core::FileDevice;
use image2usb_platform::media::{NtfsFormatOptions, format_ntfs};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: ntfs_fixture <file> [size-mib]");
    let mib: u64 = args.next().map_or(64, |value| value.parse().unwrap());
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(mib * 1024 * 1024).unwrap();
    drop(file);
    let mut io = FileDevice::open(&path).unwrap();
    format_ntfs(&mut io, &NtfsFormatOptions::new("I2U NTFS", 2048)).unwrap();
    println!("formatted {} ({} MiB)", path, mib);
}
