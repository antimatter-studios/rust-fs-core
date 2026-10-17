### Added

- **Transfers coordinate access to their backing image.** `ImageLock` gives
  import callers an exclusive, non-blocking guard before they open a writable
  driver or consume input. Exports with an image path hold a shared guard;
  when another writer holds the image, they continue with a stderr warning
  that exported bytes may be changing. Dropping the guard releases the lock.
