//! Read-only UDF probe used during native validation.
//!
//! Usage: cargo run -p image2usb-platform --features windows-media \
//!            --example udf_probe -- <image> [path] [offset] [length]

use image2usb_platform::media::Udf;

fn main() {
    let mut args = std::env::args().skip(1);
    let image = args.next().expect("usage: udf_probe <image> [path]");
    let mut udf = Udf::open(std::fs::File::open(image).unwrap()).unwrap();
    let path = args.next();
    match path {
        None => {
            for entry in udf.read_dir("").unwrap() {
                println!("{}\t{}", if entry.is_dir { "d" } else { "f" }, entry.name);
            }
        }
        Some(path) => {
            let offset: u64 = args.next().map_or(0, |v| v.parse().unwrap());
            let length: usize = args.next().map_or(32, |v| v.parse().unwrap());
            let mut buffer = vec![0; length];
            let count = udf.read_at(&path, offset, &mut buffer).unwrap();
            println!("read {count} bytes at {offset}");
            println!("{:02x?}", &buffer[..count]);
        }
    }
}
