#!/bin/sh
# Developer build only; never invoked by the installed application.
set -eu
export PATH="/opt/local/bin:/opt/local/sbin:$PATH"
export ACLOCAL_PATH="/opt/local/share/aclocal${ACLOCAL_PATH:+:$ACLOCAL_PATH}"
exec python3 - "$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)" "$@" <<'PY'
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

root = Path(sys.argv[1])
if sys.argv[2:] not in ([], ["--fetch-only"]):
    sys.exit("usage: scripts/build-windows-media.sh [--fetch-only]")
if platform.system() != "Darwin":
    sys.exit("macOS build only; Linux/Windows recipes pending")
manifest = json.loads((root / "assets/windows-media/manifest.json").read_text())
work = root / "target/windows-media" / platform.machine()
work.mkdir(parents=True, exist_ok=True)
# Upstream version detection must not discover the application's parent Git repo.
os.environ["GIT_CEILING_DIRECTORIES"] = str(work.resolve())
prefix = work / "install"
sources = {}
for resource in manifest["resources"]:
    if resource["role"] == "target_boot_asset":
        continue
    archive = work / (resource["id"] + ".archive")
    if archive.exists():
        data = archive.read_bytes()
    else:
        with urllib.request.urlopen(resource["url"], timeout=60) as response:
            data = response.read(resource["size"] + 1)
    if len(data) != resource["size"] or hashlib.sha256(data).hexdigest() != resource["sha256"]:
        sys.exit(f"invalid size/SHA-256: {resource['id']}")
    if not archive.exists():
        with archive.open("xb") as output:
            output.write(data)
    source = work / (resource["id"] + "-" + resource["sha256"][:16])
    if not source.exists():
        with tempfile.TemporaryDirectory(prefix="unpack-", dir=work) as tmp:
            with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as tar:
                members = tar.getmembers()
                for member in members:
                    path = Path(member.name)
                    if path.is_absolute() or ".." in path.parts or not (member.isdir() or member.isfile()):
                        sys.exit(f"unsafe archive entry: {member.name}")
                tar.extractall(tmp, members=members, filter="data")
            entries = list(Path(tmp).iterdir())
            if len(entries) != 1 or not entries[0].is_dir():
                sys.exit(f"expected one source directory: {resource['id']}")
            entries[0].rename(source)
    sources[resource["id"]] = source
    print(f"verified {resource['id']} {resource['sha256']}", flush=True)
if sys.argv[2:] == ["--fetch-only"]:
    sys.exit(0)
for tool in ["cc", "meson", "ninja", "make", "autoreconf", "pkg-config", "glibtoolize"]:
    if not shutil.which(tool):
        sys.exit(f"missing developer prerequisite: {tool}; nothing installed automatically")

def run(args, cwd):
    print("+ " + " ".join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), cwd=cwd, check=True)

udf = sources["libudfread"]
udf_build = work / "udf-build"
if not (udf_build / "build.ninja").exists():
    run(["meson", "setup", udf_build, udf, "--prefix=" + str(prefix),
         "--default-library=static", "--buildtype=release"], root)
run(["meson", "compile", "-C", udf_build], root)
run(["meson", "install", "-C", udf_build], root)
for name, options in [
    ("ntfs-3g", ["--disable-ntfs-3g", "--disable-ntfsprogs", "--disable-mount-helper",
                 "--disable-ldconfig", "--disable-plugins"]),
    ("wimlib-imagex", ["--without-fuse", "--without-ntfs-3g"]),
]:
    source = sources[name]
    build = work / (name + "-build")
    if not (source / "configure").exists():
        run(["autoreconf", "-fi"], source)
    build.mkdir(exist_ok=True)
    if not (build / "Makefile").exists():
        run([source / "configure", "--prefix=" + str(prefix), "--disable-shared",
             "--enable-static", *options], build)
    run(["make", "-j", str(os.cpu_count() or 1)], build)
    run(["make", "install", "rootlibdir=" + str(prefix / "lib")], build)
helper = prefix / "bin/wimlib-imagex"
run([helper, "--version"], root)
run(["otool", "-L", helper], root)
with tempfile.TemporaryDirectory(prefix="wim-check-", dir=work) as tmp:
    test = Path(tmp)
    (test / "source").mkdir()
    payload = b"Windows media helper smoke test\n"
    (test / "source/check.txt").write_bytes(payload)
    run([helper, "capture", test / "source", test / "test.wim", "smoke", "--check"], root)
    run([helper, "verify", test / "test.wim"], root)
    run([helper, "apply", test / "test.wim", "1", test / "restored"], root)
    if (test / "restored/check.txt").read_bytes() != payload:
        sys.exit("helper smoke test: restored bytes differ")

# Adapt mkntfs to callbacks: replace its single default-device allocation.
ntfs_source = sources["ntfs-3g"]
ntfs_build = work / "ntfs-3g-build"
mkntfs = ntfs_source / "ntfsprogs/mkntfs.c"
original = "ntfs_device_alloc(opts.dev_name, 0, &ntfs_device_default_io_ops, NULL)"
adapted = "i2u_ntfs_device_alloc(opts.dev_name)"
text = mkntfs.read_text()
if original in text:
    mkntfs.write_text(text.replace(original, adapted))
elif adapted not in text:
    sys.exit("unrecognized mkntfs device allocation; refusing to patch")
# mkntfs is a one-shot program; reset its globals so a later format in the same
# process cannot traverse a freed allocation list or reuse stale geometry.
cleanup = """\tp = g_allocation;
\twhile (p) {
\t\tq = p->next;
\t\tfree(p);
\t\tp = q;
\t}
}"""
reset = """\tp = g_allocation;
\twhile (p) {
\t\tq = p->next;
\t\tfree(p);
\t\tp = q;
\t}
\tg_allocation = NULL;
\tfree(g_upcaseinfo);
\tg_upcaseinfo = NULL;
\tg_mft_lcn = 0;
\tg_mftmirr_lcn = 0;
\tg_logfile_lcn = 0;
\tg_logfile_size = 0;
\tg_mft_size = 0;
\tg_mft_zone_end = 0;
\tg_num_bad_blocks = 0;
\tg_lcn_bitmap_byte_size = 0;
\tg_mft_bitmap_byte_size = 0;
}"""
text = mkntfs.read_text()
if reset in text:
    pass
elif cleanup in text:
    mkntfs.write_text(text.replace(cleanup, reset, 1))
else:
    sys.exit("unrecognized mkntfs cleanup; refusing to patch repeat-call state")
objects = work / "mkntfs-objects"
objects.mkdir(exist_ok=True)
include_flags = [
    "-DHAVE_CONFIG_H", "-I" + str(ntfs_build),
    "-I" + str(ntfs_source / "include/ntfs-3g"),
    "-I" + str(ntfs_source / "include"),
    "-I" + str(ntfs_source / "ntfsprogs"),
]
common = ["cc", "-O2", "-fPIC", "-Dmain=image2usb_mkntfs_main",
          "-Dlibintl_setlocale=setlocale", *include_flags]
override = root / "crates/platform/native/ntfs_format_override.h"
run([*common, "-include", override, "-c", ntfs_source / "ntfsprogs/mkntfs.c",
     "-o", objects / "mkntfs.c.o"], root)
for name in ["attrdef.c", "boot.c", "sd.c", "utils.c"]:
    run([*common, "-c", ntfs_source / "ntfsprogs" / name, "-o", objects / (name + ".o")], root)
run(["cc", "-O2", "-fPIC", *include_flags, "-c", root / "crates/platform/native/ntfs_format.c",
     "-o", objects / "ntfs_format.o"], root)
archive = prefix / "lib/libi2u_mkntfs.a"
run(["ar", "rcs", archive, *sorted(objects.glob("*.o"))], root)
run(["nm", "-g", archive], root)
# Read-only developer validator; it is not packaged or invoked by the app.
run(["cc", "-O2", "-Dlibintl_setlocale=setlocale", *include_flags,
     "-c", ntfs_source / "ntfsprogs/ntfsinfo.c", "-o", objects / "ntfsinfo.o"], root)
run(["cc", "-O2", objects / "ntfsinfo.o", objects / "utils.c.o",
     prefix / "lib/libntfs-3g.a", "-o", prefix / "bin/ntfsinfo",
     "-liconv", "-framework", "CoreFoundation"], root)
run([prefix / "bin/ntfsinfo", "-V"], root)
print("Native artifacts built; Rust integration of the formatter and boot remain pending.")
PY
