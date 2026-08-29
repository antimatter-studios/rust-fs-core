/*
 * fs-core C ABI — block-device handle and error conventions shared
 * across am-fs-core, am-img-qcow2, am-partitions, am-fs-ext4, and
 * future am-fs-* / am-img-* sibling crates.
 *
 * Link with libam_fs_core.a (or its sister-crate equivalent that
 * re-exports the same symbols) and include this header.
 *
 * MIT license. (c) 2026 Antimatter Studios.
 */

#ifndef FS_CORE_H
#define FS_CORE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* -------------------------------------------------------------------------
 * Error codes. Stable: do not renumber.
 *
 * On over-run, which code you get depends on the direction of the request
 * and on which crate owns the bytes under the handle:
 *
 *   - a read off the end of a file-backed handle (fs_core_file_open), or
 *     past the end of a slice's own range, returns FS_CORE_SHORT_READ;
 *   - a write refused for the same reason returns FS_CORE_OUT_OF_BOUNDS;
 *   - handles from a sister crate whose container declares a virtual size
 *     (the img-* readers) return FS_CORE_OUT_OF_BOUNDS for over-reads, and
 *     the caching / read-only / slice wrappers pass that through unchanged;
 *   - a callback-backed handle returns FS_CORE_IO whenever the host
 *     callback fails, whatever the reason.
 *
 * A caller that just wants to know "the read overran the device" and does
 * not control which crate opened the handle should accept both
 * FS_CORE_SHORT_READ and FS_CORE_OUT_OF_BOUNDS, and should not assume
 * those two are exhaustive: over a callback-backed handle the same
 * overrun arrives as FS_CORE_IO.
 * ------------------------------------------------------------------------- */

typedef enum {
    FS_CORE_OK            = 0,
    FS_CORE_IO            = 1,
    FS_CORE_SHORT_READ    = 2,
    FS_CORE_READ_ONLY     = 3,
    FS_CORE_OUT_OF_BOUNDS = 4,
    FS_CORE_CUSTOM        = 5,
    FS_CORE_NULL_ARG      = 6,
    FS_CORE_PANIC         = 7,
    /* Reserved; never returned. See FsCoreErrorCode::BadString in ffi.rs:
       the only path-taking entry point returns a pointer, not a code. */
    FS_CORE_BAD_STRING    = 8,
} FsCoreErrorCode;

/* -------------------------------------------------------------------------
 * Opaque device handle. Allocated by a sister crate's constructor (e.g.
 * `qcow2_open`, `fs_core_file_open`); freed via `fs_core_device_close`.
 * The same handle type is used by every sibling crate.
 * ------------------------------------------------------------------------- */

typedef struct FsCoreDevice FsCoreDevice;

/* -------------------------------------------------------------------------
 * Last-error retrieval. Errno-style: every fallible call stashes a
 * human-readable message in a thread-local; this returns a pointer to
 * it. Owned by the framework — do not free, do not use across calls.
 * Returns NULL when there is no current error.
 * ------------------------------------------------------------------------- */

const char *fs_core_last_error_message(void);

/* -------------------------------------------------------------------------
 * Device operations. NULL handle → `FS_CORE_NULL_ARG`.
 * ------------------------------------------------------------------------- */

void              fs_core_device_close(FsCoreDevice *handle);
uint64_t          fs_core_device_size_bytes(const FsCoreDevice *handle);
bool              fs_core_device_is_writable(const FsCoreDevice *handle);
FsCoreErrorCode   fs_core_device_read_at(const FsCoreDevice *handle,
                                          uint64_t offset,
                                          uint8_t *buf,
                                          size_t len);
FsCoreErrorCode   fs_core_device_write_at(const FsCoreDevice *handle,
                                           uint64_t offset,
                                           const uint8_t *buf,
                                           size_t len);
FsCoreErrorCode   fs_core_device_flush(const FsCoreDevice *handle);

/* -------------------------------------------------------------------------
 * Convenience constructor: open a regular file as a device. Saves
 * callers from needing a sister-crate dependency for the simple case.
 *
 * On failure returns NULL and `fs_core_last_error_message()` has detail.
 * ------------------------------------------------------------------------- */

FsCoreDevice *fs_core_file_open(const char *path, bool writable);

/* -------------------------------------------------------------------------
 * Callback-backed device. Use when the caller already owns the underlying
 * resource (an FSKit FSBlockDeviceResource, a Go file handle, a C-side fd)
 * and wants to expose it as an `FsCoreDevice` so it can be stacked under a
 * container reader (qcow2, vhd, ...) before reaching a filesystem driver.
 *
 * Callback contract: return 0 on success, non-zero (errno-like) on failure.
 * Read callbacks must fully fill `len` bytes — short reads count as I/O
 * errors. `write` may be NULL for read-only devices. `flush` may be NULL
 * (treated as a no-op).
 *
 * `ctx` is opaque to fs-core and passed back verbatim to every callback.
 * The caller owns `ctx` and must keep it valid until `fs_core_device_close`
 * is called on the returned handle.
 * ------------------------------------------------------------------------- */

typedef int (*FsCoreReadCb)(void *ctx, uint64_t offset, uint8_t *buf, size_t len);
typedef int (*FsCoreWriteCb)(void *ctx, uint64_t offset, const uint8_t *buf, size_t len);
typedef int (*FsCoreFlushCb)(void *ctx);

typedef struct {
    FsCoreReadCb  read;
    FsCoreWriteCb write;   /* NULL → device is read-only */
    FsCoreFlushCb flush;   /* NULL → flush is a no-op */
    void         *ctx;
    uint64_t      size;
} FsCoreCallbackCfg;

FsCoreDevice *fs_core_device_from_callbacks(const FsCoreCallbackCfg *cfg);

/* -------------------------------------------------------------------------
 * Slice constructors. Return a child device whose byte 0 maps to
 * `start` of `parent` and whose addressable range is `length` bytes.
 *
 * Used by partition-table walkers + container readers that want to
 * hand a sub-range to a downstream consumer (filesystem driver, fsck,
 * etc) without copying.
 *
 * The slice keeps an Arc reference to the parent — closing the parent
 * before the slice is fine. Free the slice with `fs_core_device_close`.
 *
 * `_ro` always rejects writes (FS_CORE_READ_ONLY) regardless of the
 * parent's writability. `_rw` forwards writes to the parent at
 * (start + offset); writes outside [0, length) return
 * FS_CORE_OUT_OF_BOUNDS, and a non-writable parent surfaces
 * FS_CORE_READ_ONLY.
 * ------------------------------------------------------------------------- */

FsCoreDevice *fs_core_device_slice_ro(const FsCoreDevice *parent,
                                       uint64_t start, uint64_t length);
FsCoreDevice *fs_core_device_slice_rw(const FsCoreDevice *parent,
                                       uint64_t start, uint64_t length);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* FS_CORE_H */
