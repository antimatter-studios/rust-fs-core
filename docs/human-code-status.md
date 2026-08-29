# Human-code findings — status

Tracks every **High** and **Medium** finding from
[`human-code-report-2026-08-28.md`](human-code-report-2026-08-28.md) and what
was done about it.

The report records everything as open because it was written before any of it
was acted on. This file is the current position. Updated 2026-08-29.

**19 findings** — 4 High, 9 Medium, 6 Low. This covers the 13 High and Medium.

| | High | Medium |
|---|---|---|
| Fixed | 3 | 3 |
| Left for a human decision | 1 | 4 |
| Fixable, not yet done | 0 | 2 |

---

## High

### H1 — one bounds condition, two error variants, `OutOfBounds` unreachable — **fixed earlier**

Fixed by [#11](https://github.com/antimatter-studios/rust-fs-core/pull/11), and
the outcome is worth recording because **the finding's premise was wrong**.

`OutOfBounds` is not unreachable. It is constructed on read paths by four
sibling crates — `am-img-qcow2`, `vhd`, `vhdx`, `vmdk` — and surfaces straight
through `BlockRead::read_at`. It is dead only when the device came from this
crate. So the fix was to correct the **documentation**, not the behaviour, and
the slice deduplication (H2) was done instead.

Unifying on one variant would have broken substitutability: `FileDevice`
answers a read at EOF with exactly `ShortRead { got: 0 }` and slices match.

### H2 — three slice types are one type three times — **fixed earlier**

`SliceGeometry` in `src/slice.rs` holds the one range check the three types
share. Fixed by [#11](https://github.com/antimatter-studios/rust-fs-core/pull/11).

### H3 — the README omitted four modules and listed two shipped types as unimplemented — **fixed**

`slice.rs`, `readonly.rs`, `stream.rs` and `ffi.rs` are now in the layout tree.

The worse half was the Roadmap, which listed `SliceReader` and
`ReadOnlyDevice<T>` under **"Planned additions (not yet implemented)"** while
both ship and are re-exported from `lib.rs`. A reader taking the README at its
word would go looking for them in `am-partitions`, or write their own. Both
entries are gone.

### H4 — an empty `impl BlockDevice for X {}` is load-bearing — **needs your decision**

The trait supplies defaults for all three methods, so an empty impl means
"strictly read-only" — deliberate at the three sites in this crate, and
`readonly.rs` documents the intent well.

The report's real concern is what the same design does in a *consumer*: a
driver author who forgets `write_at` gets a device that silently refuses writes
rather than a compile error. That is a genuine trap.

**Not fixed, because every available fix is a breaking change with a real
trade-off.** Removing the defaults makes every read-only implementor write
three stub methods; splitting the trait changes the public shape every consumer
already implements. Which cost is acceptable is a design call about the crate's
public surface, not a defect to correct.

---

## Medium

### M1 — the pointer-returning half of the C ABI hand-rolls `ffi_guard` four times — **fixable, not yet done**

Real duplication: `ffi_guard` returns a code, so the four functions returning
`*mut FsCoreDevice` cannot use it and each reimplements the `catch_unwind`,
error-mapping and message-stashing epilogue.

A pointer-returning sibling guard would fix it. Left for its own change:
it touches the FFI epilogue, where a mistake either leaks the device or reports
the wrong code to a C caller, and it deserves a review focused on that rather
than being folded into a documentation pass.

### M2 — a five-line explanation sat above the wrong function — **fixed**

The comment explaining why `ctx` is round-tripped through `usize` was above
`cb_io_err`, which does not do that. It now sits above the `let ctx_addr =
cfg.ctx as usize;` it explains, and `cb_io_err` has a one-line doc of its own.

### M3 — the three callback adapters are the same eight-line shape — **fixable, not yet done**

Same reasoning as M1: genuine, mechanical, and in the FFI layer. Worth doing
together with M1 in a change that is only about that.

### M4 — `FS_CORE_BAD_STRING` is published and can never be returned — **fixed**

Verified: the only path-taking entry point, `fs_core_open_file`, returns a
**pointer**, so it reports a bad path as NULL plus a message and cannot return
a code at all. No other entry point takes a path.

**Documented rather than removed.** The numbering is published in
`include/fs_core.h`; a consumer may already switch on `8`, and renumbering the
codes after it would be an ABI break for a tidiness gain. Both the Rust enum
and the C header now say it is reserved and why, so the next reader does not
have to rediscover it.

### M5 — `stats()` returns a bare `(u64, u64)` — **needs your decision**

A named struct would be better to read. It is also a public API change to a
shipped crate, and the crate is already published at `0.2.3` with consumers.
Whether that churn is worth it is yours.

### M6 — `invalidate_range` takes four parameters to work around a borrow — **needs your decision**

Same category: the fix is a signature change on a public method.

### M7 — `BlockRead for &T` exists, `BlockDevice for &T` does not — **needs your decision**

Adding the impl is additive and small. But the gap may be deliberate — a
`&T` that can be written through has different aliasing implications from one
that can only be read — and the report does not establish which it is. Adding a
public impl on a guess is the wrong direction to be wrong in.

### M8 — eleven hand-rolled in-memory device fakes — **fixable, not yet done**

Real, and a shared test double would be an improvement. Eleven sites across
several files, each subtly different, so consolidating them means checking that
no test depended on a difference. Worth its own change.

### M9 — `CachingDevice::new` decides the caller's ownership — **needs your decision**

Public API shape, same category as M5 and M6.

---

## Verification

`cargo test` — 42 unit, 7 doc and 6 integration tests pass, unchanged in number:
nothing here changes behaviour. `chore lint` clean.
