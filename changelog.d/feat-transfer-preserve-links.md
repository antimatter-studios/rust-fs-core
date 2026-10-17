### Added

- **Tree exports can preserve links.** The additive link-aware export API and
  `--preserve=links` arguments copy each regular inode once and recreate symbolic
  targets unchanged without following them. Targets escaping DEST are reported;
  the default refuses symbolic links and reports separately copied hard links.
  Repeated directory identities are refused as loops under either policy.
