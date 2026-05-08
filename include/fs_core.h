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

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* FS_CORE_H */
