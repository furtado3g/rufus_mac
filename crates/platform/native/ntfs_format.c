/*
 * Adapts mkntfs (ntfsprogs) to caller-supplied storage callbacks.
 *
 * This file is part of image2usb and is distributed under GPL-3.0-or-later,
 * incorporating GPL-2.0-or-later code from NTFS-3G by linking to mkntfs.
 */
#include "config.h"

#include <errno.h>
#include <getopt.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/disk.h>

#include "attrib.h"
#include "device.h"
#include "dir.h"
#include "inode.h"
#include "layout.h"
#include "volume.h"

struct i2u_ntfs_ctx {
	void *opaque;
	int64_t (*read)(void *, uint64_t, void *, uint64_t);
	int64_t (*write)(void *, uint64_t, const void *, uint64_t);
	int (*sync)(void *);
	uint64_t size;
	uint32_t sector;
	int64_t position;
	int saved_errno;
};

struct i2u_ntfs_mount {
	ntfs_volume *vol;
	struct i2u_ntfs_ctx *ctx;
};

struct i2u_ntfs_file {
	ntfs_inode *ni;
	ntfs_attr *na;
};

static int i2u_open(struct ntfs_device *dev, int flags)
{
	(void)flags;
	if (!dev || !dev->d_private) {
		errno = EINVAL;
		return -1;
	}
	return 0;
}

static int i2u_close(struct ntfs_device *dev)
{
	(void)dev;
	return 0;
}

static s64 i2u_seek(struct ntfs_device *dev, s64 offset, int whence)
{
	struct i2u_ntfs_ctx *ctx = dev->d_private;
	s64 base;
	switch (whence) {
	case SEEK_SET: base = 0; break;
	case SEEK_CUR: base = ctx->position; break;
	case SEEK_END: base = (s64)ctx->size; break;
	default: errno = EINVAL; return -1;
	}
	if (offset < 0 || base > (s64)ctx->size || offset > (s64)ctx->size - base) {
		errno = EINVAL;
		return -1;
	}
	ctx->position = base + offset;
	return ctx->position;
}

static s64 i2u_read(struct ntfs_device *dev, void *buf, s64 count)
{
	struct i2u_ntfs_ctx *ctx = dev->d_private;
	if (count < 0 || (uint64_t)count > ctx->size - (uint64_t)ctx->position) {
		ctx->saved_errno = EINVAL;
		errno = EINVAL;
		return -1;
	}
	int64_t got = ctx->read(ctx->opaque, (uint64_t)ctx->position, buf, (uint64_t)count);
	if (got != count) {
		if (got >= 0) ctx->saved_errno = EIO;
		errno = got >= 0 ? EIO : (ctx->saved_errno ? ctx->saved_errno : EIO);
		return -1;
	}
	ctx->position += count;
	return count;
}

static s64 i2u_write(struct ntfs_device *dev, const void *buf, s64 count)
{
	struct i2u_ntfs_ctx *ctx = dev->d_private;
	if (count < 0 || (uint64_t)count > ctx->size - (uint64_t)ctx->position) {
		ctx->saved_errno = EINVAL;
		errno = EINVAL;
		return -1;
	}
	int64_t put = ctx->write(ctx->opaque, (uint64_t)ctx->position, buf, (uint64_t)count);
	if (put != count) {
		if (put >= 0) ctx->saved_errno = EIO;
		errno = put >= 0 ? EIO : (ctx->saved_errno ? ctx->saved_errno : EIO);
		return -1;
	}
	ctx->position += count;
	return count;
}

static s64 i2u_pread(struct ntfs_device *dev, void *buf, s64 count, s64 offset)
{
	struct i2u_ntfs_ctx *ctx = dev->d_private;
	if (count < 0 || offset < 0 || (uint64_t)offset > ctx->size ||
	    (uint64_t)count > ctx->size - (uint64_t)offset) {
		ctx->saved_errno = EINVAL;
		errno = EINVAL;
		return -1;
	}
	int64_t got = ctx->read(ctx->opaque, (uint64_t)offset, buf, (uint64_t)count);
	if (got != count) {
		if (got >= 0) ctx->saved_errno = EIO;
		errno = got >= 0 ? EIO : (ctx->saved_errno ? ctx->saved_errno : EIO);
		return -1;
	}
	return count;
}

static s64 i2u_pwrite(struct ntfs_device *dev, const void *buf, s64 count, s64 offset)
{
	struct i2u_ntfs_ctx *ctx = dev->d_private;
	if (count < 0 || offset < 0 || (uint64_t)offset > ctx->size ||
	    (uint64_t)count > ctx->size - (uint64_t)offset) {
		ctx->saved_errno = EINVAL;
		errno = EINVAL;
		return -1;
	}
	int64_t put = ctx->write(ctx->opaque, (uint64_t)offset, buf, (uint64_t)count);
	if (put != count) {
		if (put >= 0) ctx->saved_errno = EIO;
		errno = put >= 0 ? EIO : (ctx->saved_errno ? ctx->saved_errno : EIO);
		return -1;
	}
	return count;
}

static int i2u_sync(struct ntfs_device *dev)
{
	struct i2u_ntfs_ctx *ctx = dev->d_private;
	if (ctx->sync(ctx->opaque) != 0) {
		ctx->saved_errno = EIO;
		errno = EIO;
		return -1;
	}
	return 0;
}

static int i2u_stat(struct ntfs_device *dev, struct stat *buf)
{
	struct i2u_ntfs_ctx *ctx = dev->d_private;
	memset(buf, 0, sizeof(*buf));
	buf->st_mode = S_IFBLK | S_IRUSR | S_IWUSR;
	buf->st_size = (off_t)ctx->size;
	buf->st_blksize = (blksize_t)ctx->sector;
	buf->st_rdev = 1;
	return 0;
}

static int i2u_ioctl(struct ntfs_device *dev, unsigned long request, void *argp)
{
	struct i2u_ntfs_ctx *ctx = dev->d_private;
	if (request == DKIOCGETBLOCKSIZE && argp) {
		*(uint32_t *)argp = ctx->sector;
		return 0;
	}
	if (request == DKIOCGETBLOCKCOUNT && argp) {
		*(uint64_t *)argp = ctx->size / ctx->sector;
		return 0;
	}
	errno = ENOTTY;
	return -1;
}

struct ntfs_device_operations i2u_ntfs_device_ops = {
	.open = i2u_open,
	.close = i2u_close,
	.seek = i2u_seek,
	.read = i2u_read,
	.write = i2u_write,
	.pread = i2u_pread,
	.pwrite = i2u_pwrite,
	.sync = i2u_sync,
	.stat = i2u_stat,
	.ioctl = i2u_ioctl,
};

/* Defined by mkntfs.c compiled with -Dmain=image2usb_mkntfs_main. */
int image2usb_mkntfs_main(int argc, char *argv[]);

/* The callback pointer lives in a thread-local so mkntfs' static device
 * allocation path can use it without receiving an extra argument. */
static _Thread_local struct i2u_ntfs_ctx *active_ctx;

int i2u_mkntfs_format(uint64_t size, uint32_t sector, uint64_t part_start,
		uint32_t heads, uint32_t sectors_per_track, const char *label,
		void *opaque,
		int64_t (*read_cb)(void *, uint64_t, void *, uint64_t),
		int64_t (*write_cb)(void *, uint64_t, const void *, uint64_t),
		int (*sync_cb)(void *))
{
	if (!label || !read_cb || !write_cb || !sync_cb || sector < 512 ||
	    sector > 4096 || (sector & (sector - 1)) || size % sector ||
	    size / sector < 4096 || size > INT64_MAX ||
	    part_start > UINT32_MAX || heads < 1 ||
	    heads > 65535 || sectors_per_track < 1 || sectors_per_track > 65535) {
		errno = EINVAL;
		return -1;
	}
	struct i2u_ntfs_ctx ctx;
	memset(&ctx, 0, sizeof(ctx));
	ctx.opaque = opaque;
	ctx.read = read_cb;
	ctx.write = write_cb;
	ctx.sync = sync_cb;
	ctx.size = size;
	ctx.sector = sector;
	active_ctx = &ctx;
	char sector_arg[16], sectors_arg[32], part_arg[32], heads_arg[16], spt_arg[16];
	snprintf(sector_arg, sizeof(sector_arg), "%u", sector);
	snprintf(sectors_arg, sizeof(sectors_arg), "%llu",
		 (unsigned long long)(size / sector));
	snprintf(part_arg, sizeof(part_arg), "%llu", (unsigned long long)part_start);
	snprintf(heads_arg, sizeof(heads_arg), "%u", heads);
	snprintf(spt_arg, sizeof(spt_arg), "%u", sectors_per_track);
	char *argv[] = { (char *)"mkntfs", (char *)"-F", (char *)"-Q",
		(char *)"-q", (char *)"-L", (char *)label,
		(char *)"-s", sector_arg, (char *)"-p", part_arg,
		(char *)"-H", heads_arg, (char *)"-S", spt_arg,
		(char *)"i2u-ntfs", sectors_arg, NULL };
	optind = 1;
#ifdef __APPLE__
	optreset = 1;
#endif
	int result = image2usb_mkntfs_main(16, argv);
	active_ctx = NULL;
	if (result != 0) {
		errno = ctx.saved_errno ? ctx.saved_errno : EIO;
		return -1;
	}
	return 0;
}

/* Referenced by patched mkntfs device allocation. */
struct ntfs_device *i2u_ntfs_device_alloc(const char *name)
{
	return ntfs_device_alloc(name, 0, &i2u_ntfs_device_ops, active_ctx);
}

/* ---- Read-write volume API used by the extraction stage ---- */

static int i2u_lookup_child(ntfs_volume *vol, ntfs_inode *dir,
		const char *name, ntfs_inode **child)
{
	ntfschar *uname = NULL;
	int length;
	u64 mref;

	*child = NULL;
	length = ntfs_mbstoucs(name, &uname);
	if (length < 0)
		return -1;
	if (length > NTFS_MAX_NAME_LEN) {
		free(uname);
		errno = ENAMETOOLONG;
		return -1;
	}
	errno = 0;
	mref = ntfs_inode_lookup_by_name(dir, uname, (u8)length);
	free(uname);
	if (mref == (u64)-1) {
		if (errno == ENOENT)
			return 0;
		return -1;
	}
	*child = ntfs_inode_open(vol, MREF(mref));
	return *child ? 1 : -1;
}

static ntfs_inode *i2u_open_dir_path(ntfs_volume *vol, const char *path)
{
	ntfs_inode *dir;
	char *copy, *part, *save = NULL;

	dir = ntfs_inode_open(vol, FILE_root);
	if (!dir)
		return NULL;
	if (!path || !*path)
		return dir;
	copy = strdup(path);
	if (!copy) {
		ntfs_inode_close(dir);
		return NULL;
	}
	for (part = strtok_r(copy, "/", &save); part;
	     part = strtok_r(NULL, "/", &save)) {
		ntfs_inode *child = NULL;
		int found = i2u_lookup_child(vol, dir, part, &child);
		if (found <= 0) {
			if (found == 0)
				errno = ENOENT;
			free(copy);
			ntfs_inode_close(dir);
			return NULL;
		}
		ntfs_inode_close(dir);
		dir = child;
	}
	free(copy);
	return dir;
}

int i2u_ntfs_mount(uint64_t size, uint32_t sector, void *opaque,
		int64_t (*read_cb)(void *, uint64_t, void *, uint64_t),
		int64_t (*write_cb)(void *, uint64_t, const void *, uint64_t),
		int (*sync_cb)(void *), void **out)
{
	struct i2u_ntfs_ctx *ctx;
	struct i2u_ntfs_mount *mount;
	struct ntfs_device *dev;
	ntfs_volume *vol;

	if (!out)
		return -1;
	*out = NULL;
	if (!read_cb || !write_cb || !sync_cb || sector < 512 ||
	    sector > 4096 || (sector & (sector - 1)) || size % sector ||
	    size / sector < 4096 || size > INT64_MAX) {
		errno = EINVAL;
		return -1;
	}
	ctx = calloc(1, sizeof(*ctx));
	if (!ctx)
		return -1;
	ctx->opaque = opaque;
	ctx->read = read_cb;
	ctx->write = write_cb;
	ctx->sync = sync_cb;
	ctx->size = size;
	ctx->sector = sector;
	active_ctx = ctx;
	dev = ntfs_device_alloc("i2u-ntfs", 0, &i2u_ntfs_device_ops, ctx);
	vol = dev ? ntfs_device_mount(dev, 0) : NULL;
	active_ctx = NULL;
	if (!vol) {
		int saved = ctx->saved_errno ? ctx->saved_errno : errno;
		free(ctx);
		errno = saved;
		return -1;
	}
	mount = malloc(sizeof(*mount));
	if (!mount) {
		ntfs_umount(vol, TRUE);
		free(ctx);
		errno = ENOMEM;
		return -1;
	}
	mount->vol = vol;
	mount->ctx = ctx;
	*out = mount;
	return 0;
}

int i2u_ntfs_mkdir(void *handle, const char *path)
{
	struct i2u_ntfs_mount *mount = handle;
	ntfs_inode *dir;
	char *copy, *part, *save = NULL;

	if (!mount || !path || !*path) {
		errno = EINVAL;
		return -1;
	}
	dir = ntfs_inode_open(mount->vol, FILE_root);
	if (!dir)
		return -1;
	copy = strdup(path);
	if (!copy) {
		ntfs_inode_close(dir);
		return -1;
	}
	for (part = strtok_r(copy, "/", &save); part;
	     part = strtok_r(NULL, "/", &save)) {
		ntfs_inode *child = NULL;
		int found = i2u_lookup_child(mount->vol, dir, part, &child);
		if (found < 0) {
			free(copy);
			ntfs_inode_close(dir);
			return -1;
		}
		if (found == 0) {
			ntfschar *uname = NULL;
			int length = ntfs_mbstoucs(part, &uname);
			if (length < 0 || length > NTFS_MAX_NAME_LEN) {
				free(uname);
				if (length > NTFS_MAX_NAME_LEN)
					errno = ENAMETOOLONG;
				free(copy);
				ntfs_inode_close(dir);
				return -1;
			}
			child = ntfs_create(dir, 0, uname, (u8)length, S_IFDIR);
			free(uname);
			if (!child) {
				free(copy);
				ntfs_inode_close(dir);
				return -1;
			}
		}
		ntfs_inode_close(dir);
		dir = child;
	}
	free(copy);
	ntfs_inode_close(dir);
	return 0;
}

void *i2u_ntfs_file_create(void *handle, const char *path,
		int make_parents)
{
	struct i2u_ntfs_mount *mount = handle;
	struct i2u_ntfs_file *file;
	ntfs_inode *parent;
	ntfschar *uname = NULL;
	char *copy, *slash;
	int length;

	if (!mount || !path || !*path) {
		errno = EINVAL;
		return NULL;
	}
	copy = strdup(path);
	if (!copy)
		return NULL;
	slash = strrchr(copy, '/');
	if (slash) {
		*slash = '\0';
		if (make_parents && i2u_ntfs_mkdir(mount, copy)) {
			free(copy);
			return NULL;
		}
		parent = i2u_open_dir_path(mount->vol, copy);
		slash++;
	} else {
		parent = ntfs_inode_open(mount->vol, FILE_root);
		slash = copy;
	}
	if (!parent || !*slash) {
		free(copy);
		if (parent)
			ntfs_inode_close(parent);
		else
			errno = EINVAL;
		return NULL;
	}
	length = ntfs_mbstoucs(slash, &uname);
	if (length < 0 || length > NTFS_MAX_NAME_LEN) {
		free(uname);
		if (length > NTFS_MAX_NAME_LEN)
			errno = ENAMETOOLONG;
		free(copy);
		ntfs_inode_close(parent);
		return NULL;
	}
	ntfs_inode *ni = ntfs_create(parent, 0, uname, (u8)length, S_IFREG);
	free(uname);
	ntfs_inode_close(parent);
	free(copy);
	if (!ni)
		return NULL;
	ntfs_attr *na = ntfs_attr_open(ni, AT_DATA, AT_UNNAMED, 0);
	if (!na) {
		ntfs_inode_close(ni);
		return NULL;
	}
	file = malloc(sizeof(*file));
	if (!file) {
		ntfs_attr_close(na);
		ntfs_inode_close(ni);
		errno = ENOMEM;
		return NULL;
	}
	file->ni = ni;
	file->na = na;
	return file;
}

void *i2u_ntfs_file_open(void *handle, const char *path)
{
	struct i2u_ntfs_mount *mount = handle;
	struct i2u_ntfs_file *file;
	ntfs_inode *parent;
	ntfs_inode *ni = NULL;
	char *copy, *slash;
	int found;

	if (!mount || !path || !*path) {
		errno = EINVAL;
		return NULL;
	}
	copy = strdup(path);
	if (!copy)
		return NULL;
	slash = strrchr(copy, '/');
	if (slash) {
		*slash = '\0';
		parent = i2u_open_dir_path(mount->vol, copy);
		slash++;
	} else {
		parent = ntfs_inode_open(mount->vol, FILE_root);
		slash = copy;
	}
	if (!parent || !*slash) {
		free(copy);
		if (parent)
			ntfs_inode_close(parent);
		else
			errno = EINVAL;
		return NULL;
	}
	found = i2u_lookup_child(mount->vol, parent, slash, &ni);
	ntfs_inode_close(parent);
	free(copy);
	if (found <= 0) {
		if (found == 0)
			errno = ENOENT;
		return NULL;
	}
	ntfs_attr *na = ntfs_attr_open(ni, AT_DATA, AT_UNNAMED, 0);
	if (!na) {
		ntfs_inode_close(ni);
		return NULL;
	}
	file = malloc(sizeof(*file));
	if (!file) {
		ntfs_attr_close(na);
		ntfs_inode_close(ni);
		errno = ENOMEM;
		return NULL;
	}
	file->ni = ni;
	file->na = na;
	return file;
}

int64_t i2u_ntfs_file_read(void *file_handle, uint64_t offset,
		void *buffer, uint64_t length)
{
	struct i2u_ntfs_file *file = file_handle;
	uint64_t total = 0;

	if (!file || (!buffer && length) || offset > INT64_MAX ||
	    length > (uint64_t)INT64_MAX - offset) {
		errno = EINVAL;
		return -1;
	}
	while (total < length) {
		s64 got = ntfs_attr_pread(file->na, (s64)(offset + total),
				(s64)(length - total),
				(u8 *)buffer + total);
		if (got < 0)
			return -1;
		if (got == 0)
			break;
		total += (uint64_t)got;
	}
	return (int64_t)total;
}

int64_t i2u_ntfs_file_write(void *file_handle, uint64_t offset,
		const void *buffer, uint64_t length)
{
	struct i2u_ntfs_file *file = file_handle;
	uint64_t total = 0;

	if (!file || (!buffer && length) || offset > INT64_MAX ||
	    length > (uint64_t)INT64_MAX - offset ||
	    length > (uint64_t)INT64_MAX) {
		errno = EINVAL;
		return -1;
	}
	while (total < length) {
		s64 written = ntfs_attr_pwrite(file->na, (s64)(offset + total),
				(s64)(length - total),
				(const u8 *)buffer + total);
		if (written <= 0)
			return -1;
		total += (uint64_t)written;
	}
	return (int64_t)total;
}

int i2u_ntfs_file_truncate(void *file_handle, uint64_t size)
{
	struct i2u_ntfs_file *file = file_handle;
	if (!file || size > INT64_MAX) {
		errno = EINVAL;
		return -1;
	}
	return ntfs_attr_truncate(file->na, (s64)size);
}

int i2u_ntfs_file_close(void *file_handle)
{
	struct i2u_ntfs_file *file = file_handle;
	int result = 0;
	int saved = 0;

	if (!file)
		return 0;
	if (ntfs_attr_pclose(file->na)) {
		saved = errno;
		result = -1;
	}
	if (ntfs_inode_close(file->ni) && result == 0) {
		saved = errno;
		result = -1;
	}
	free(file);
	if (saved)
		errno = saved;
	return result;
}

int i2u_ntfs_sync(void *handle)
{
	struct i2u_ntfs_mount *mount = handle;
	if (!mount) {
		errno = EINVAL;
		return -1;
	}
	return ntfs_device_sync(mount->vol->dev);
}

int i2u_ntfs_umount(void *handle)
{
	struct i2u_ntfs_mount *mount = handle;
	if (!mount) {
		errno = EINVAL;
		return -1;
	}
	if (ntfs_umount(mount->vol, TRUE))
		return -1;
	free(mount->ctx);
	free(mount);
	return 0;
}
