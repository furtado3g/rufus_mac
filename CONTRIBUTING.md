# Contributing

## Build

```console
$ cargo build --workspace
$ cargo test --workspace
$ cargo clippy --workspace --all-targets -- -D warnings
$ cargo fmt --all
```

Rust 1.98.1 (edition 2024) is pinned in `rust-toolchain.toml`.

## Testing without a USB drive

The write pipeline can target a regular file:

```console
$ python3 scripts/make-test-iso.py /tmp/test.iso
$ truncate -s 64m /tmp/disk.img        # macOS: mkfile 64m /tmp/disk.img
$ cargo run -p image2usb-cli -- analyze /tmp/test.iso
$ cargo run -p image2usb-cli -- write /tmp/test.iso \
      --output /tmp/disk.img --yes --verify --mode dd
```

The synthetic image is **not bootable**; it only exercises analysis and the
write/verify pipeline. Use QEMU with a real distro ISO for boot tests.

## Device tests

Only run these with a spare USB drive. Confirm the device twice:

```console
$ cargo run -p image2usb-cli -- list
$ sudo cargo run -p image2usb-cli -- write ubuntu.iso --device /dev/diskN --verify
```

Never add a device id to a test fixture or script.

## Code conventions

- `unsafe` is forbidden in every crate except `image2usb-platform`.
- Library code never panics on untrusted input; parsers return errors.
- New safety checks require a test that proves the check rejects the bad case.
- Public items need rustdoc.
- Commits follow Conventional Commits (`feat:`, `fix:`, `docs:`, `test:`…).

## Platform matrix

| Host | Backend | Verification |
| --- | --- | --- |
| macOS 12+ | `platform/src/macos.rs` | local + CI |
| Linux | `platform/src/linux.rs` | CI + QEMU |
| Windows 10+ | `platform/src/windows.rs` | CI |

Cross-platform code must compile with `cargo check --target ...`; CI builds all
three operating systems on every pull request.
