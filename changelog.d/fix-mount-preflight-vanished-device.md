### Fixed

- Linux transfer mount checks no longer abort when an unrelated loop device's backing attribute disappears during the sysfs walk. Checks for the image's backing metadata, devices, partitions and holders still fail closed.
