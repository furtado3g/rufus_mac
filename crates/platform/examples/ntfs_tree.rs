//! Developer fixture generator for the read-write NTFS API.
//!
//! Usage: cargo run -p image2usb-platform --features windows-media \
//!            --example ntfs_tree -- target/ntfs-tree.img [size-mib]
//!
//! The example only opens the supplied regular file. It never scans or writes
//! a physical device.

use image2usb_core::FileDevice;
use image2usb_platform::media::{NtfsFormatOptions, NtfsVolume, format_ntfs};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: ntfs_tree <file> [size-mib]");
    let mib: u64 = args.next().map_or(64, |value| value.parse().unwrap());
    let large = args.any(|value| value == "--large");
    let fill = args.any(|value| value == "--fill");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(mib * 1024 * 1024).unwrap();
    drop(file);
    let mut io = FileDevice::open(&path).unwrap();
    format_ntfs(&mut io, &NtfsFormatOptions::new("I2U TREE", 2048)).unwrap();
    {
        let mut volume = NtfsVolume::mount(&mut io).unwrap();
        volume.mkdir("sources/nested").unwrap();
        {
            let mut file = volume.create_file("sources/nested/data.bin").unwrap();
            let block = vec![0x5a; 1024 * 1024];
            file.write_at(0, &block).unwrap();
            file.write_at(5 * 1024 * 1024, b"tail").unwrap();
            file.close().unwrap();
        }
        if large {
            const BIG: u64 = 4 * 1024 * 1024 * 1024 + 1024 * 1024;
            let mut file = volume.create_file("sources/big.bin").unwrap();
            file.truncate(BIG).unwrap();
            if fill {
                let chunk = vec![0xa5; 16 * 1024 * 1024];
                let mut offset = 0;
                while offset < BIG {
                    let count = (BIG - offset).min(chunk.len() as u64) as usize;
                    file.write_at(offset, &chunk[..count]).unwrap();
                    offset += count as u64;
                }
            }
            file.write_at(0, b"HEAD").unwrap();
            file.write_at(BIG - 4, b"TAIL").unwrap();
            file.close().unwrap();
        }
        volume.sync().unwrap();
        volume.unmount().unwrap();
    }
    println!("formatted and populated {} ({} MiB)", path, mib);
}
