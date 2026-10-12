### Fixed

- **semver-check tells a crates.io refusal from a crate with no release.**
  Any answer but 200 meant "unpublished", so a transient 403 sent a renamed
  crate down the former-name path to die in a JSON parse. A 404 alone now
  means no release; anything else is asked again, and reported if it keeps
  coming.
