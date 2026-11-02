### Added

- **A tree import can keep holes and attributes together (#404).**
  `import_tree_with_preserve` takes both `SparseOptions` and `XattrOptions`, so
  a driver can offer `import --recursive --preserve=sparse,xattrs`.
