#ifndef IMAGE2USB_NTFS_FORMAT_OVERRIDE_H
#define IMAGE2USB_NTFS_FORMAT_OVERRIDE_H

/* Supplied by native/ntfs_format.c; replaces mkntfs' default host I/O. */
struct ntfs_device;
extern struct ntfs_device *i2u_ntfs_device_alloc(const char *name);

#endif
