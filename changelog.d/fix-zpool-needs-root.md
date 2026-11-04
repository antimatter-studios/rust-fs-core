### Fixed

- Linux transfers no longer refuse every image when the ZFS module is loaded and `zpool status -P` fails, provided the kernel lists no imported pool. With a pool imported the refusal stays, and it now says why root is needed: an imported pool writes to its vdevs with no dataset mounted.
