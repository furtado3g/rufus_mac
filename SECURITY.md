# Security policy

image2usb writes directly to block devices. A bug can destroy data, so the
following invariants are enforced in code and covered by tests:

1. A device is only opened if it appeared in the most recent enumeration.
2. Internal disks, the system disk and read-only media are never accepted.
3. The identity token (model, serial, size) is re-checked immediately before
   opening the device, to defeat device-swap races.
4. No write happens without explicit confirmation (`--yes` or typing the serial).
5. Volumes are unmounted/locked before the raw device is opened.
6. Write errors report the exact byte offset; nothing is retried destructively.
7. Temporary directories are created with `0700` and cleaned up; symlinks are
   never followed when extracting images.
8. External programs are executed with argument arrays, never through a shell.

## Reporting a vulnerability

Please do **not** open a public issue for vulnerabilities that could lead to data
loss or privilege escalation. Send a private report to the maintainers listed in
`Cargo.toml` / the repository security advisory page.

Include:

- image2usb version and platform;
- steps to reproduce, ideally with a synthetic image;
- the affected device or image size;
- whether the process ran elevated.

We aim to acknowledge reports within 72 hours.

## Threat model (phase 0)

- The user is expected to run image2usb with administrator/root privileges,
  because raw device access requires it.
- Images are untrusted input: the ISO 9660 and El Torito parsers are fuzzed and
  bounded (no unbounded allocation from on-disk lengths).
- The tool does not fetch anything from the network in phase 0.
- The privileged process only exposes "write this image to this validated
  device"; it does not execute image content.
