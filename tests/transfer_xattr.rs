//! Extended-attribute exports checked with independent host attribute tools.
#![cfg(feature = "cli")]
use fs_core::cli::clap::Command;
use fs_core::cli::transfer::{
    self, ExportRequest, ExtendedAttribute, LinkOptions, Output, TimeOptions, TreeEntry, TreeKind,
    TreeMetadata, TreeOptions, TreeSource, XattrOptions,
};
use fs_core::cli::{CliError, Json, Outcome};
use std::path::{Path, PathBuf};

struct Source {
    attributes: Option<Vec<ExtendedAttribute>>,
}
impl TreeSource for Source {
    fn metadata(&mut self, _: &Path) -> Result<TreeMetadata, CliError> {
        Ok(TreeMetadata {
            kind: TreeKind::File,
            size: 4,
            inode: 1,
            accessed: None,
            modified: None,
        })
    }
    fn read_dir(&mut self, _: &Path) -> Result<Vec<TreeEntry>, CliError> {
        unreachable!()
    }
    fn read_file_at(&mut self, _: &Path, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        let bytes = &b"data"[offset as usize..];
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }
    fn extended_attributes(&mut self, _: &Path) -> Result<Vec<ExtendedAttribute>, CliError> {
        self.attributes
            .clone()
            .ok_or_else(|| CliError::refused("source xattrs unavailable"))
    }
}
fn destination(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("core-xattr-{}-{tag}", std::process::id()))
}
fn run(tag: &str, source: &mut dyn TreeSource, preserve: bool) -> (PathBuf, Outcome) {
    let dest = destination(tag);
    let result = transfer::export_tree_with_xattrs(
        &ExportRequest {
            path: "file".into(),
            output: Output::File(dest.clone()),
            replace: false,
        },
        source,
        TreeOptions::default(),
        LinkOptions::default(),
        TimeOptions::default(),
        XattrOptions {
            preserve_xattrs: preserve,
        },
    )
    .unwrap();
    (dest, result)
}
fn count(result: &Outcome, key: &str) -> u64 {
    match result.report.as_ref().unwrap().get(key).unwrap() {
        Json::UInt(n) => *n,
        other => panic!("unexpected count: {other:?}"),
    }
}
#[test]
fn xattrs_combine_with_existing_preserve_policies() {
    let matches = Command::new("export")
        .args(transfer::export_args_with_xattrs())
        .try_get_matches_from([
            "export",
            "file",
            "dest",
            "--preserve=xattrs,times,links,sparse",
        ])
        .unwrap();
    assert!(
        XattrOptions::from_matches(&matches)
            .unwrap()
            .preserve_xattrs
    );
    assert!(TimeOptions::from_matches(&matches).unwrap().preserve_times);
    assert!(LinkOptions::from_matches(&matches).unwrap().preserve_links);
    assert!(
        transfer::SparseOptions::from_matches(&matches)
            .unwrap()
            .preserve_sparse
    );
}
#[test]
fn older_argument_sets_default_to_no_xattrs() {
    let matches = Command::new("export")
        .args(transfer::export_args_with_tree())
        .try_get_matches_from(["export", "file", "dest"])
        .unwrap();
    assert!(
        !XattrOptions::from_matches(&matches)
            .unwrap()
            .preserve_xattrs
    );
}
#[test]
fn disabled_preservation_does_not_query_the_optional_source() {
    let (dest, result) = run("disabled", &mut Source { attributes: None }, false);
    assert_eq!(count(&result, "completed"), 1);
    assert_eq!(std::fs::read(&dest).unwrap(), b"data");
    std::fs::remove_file(dest).unwrap();
}
#[test]
fn a_source_attribute_refusal_is_reported_per_item() {
    let (dest, result) = run("refused", &mut Source { attributes: None }, true);
    assert_eq!(count(&result, "refused"), 1);
    assert_ne!(result.code, 0);
    assert!(format!("{:?}", result.report).contains("source xattrs unavailable"));
    std::fs::remove_file(dest).unwrap();
}
#[test]
fn a_supported_source_with_no_attributes_completes() {
    let (dest, result) = run(
        "empty",
        &mut Source {
            attributes: Some(vec![]),
        },
        true,
    );
    assert_eq!(count(&result, "completed"), 1);
    std::fs::remove_file(dest).unwrap();
}
#[test]
fn an_unrepresentable_attribute_name_is_refused_and_named() {
    let (dest, result) = run(
        "invalid",
        &mut Source {
            attributes: Some(vec![ExtendedAttribute {
                name: "bad\0name".into(),
                value: vec![1],
            }]),
        },
        true,
    );
    assert_eq!(count(&result, "refused"), 1);
    assert!(format!("{:?}", result.report).contains("bad"));
    std::fs::remove_file(dest).unwrap();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn host_attribute(path: &Path, name: &str) -> Vec<u8> {
    use std::process::Command;
    #[cfg(target_os = "linux")]
    let output = Command::new("python3")
        .args(["-c", "import os,sys; sys.stdout.buffer.write(os.getxattr(sys.argv[1], sys.argv[2], follow_symlinks=False))"])
        .arg(path).arg(name).output().expect("install python3 for the host xattr oracle");
    #[cfg(target_os = "macos")]
    let output = Command::new("/usr/bin/xattr")
        .args(["-px", name])
        .arg(path)
        .output()
        .expect("the host xattr oracle is required");
    assert!(output.status.success(), "host xattr oracle: {output:?}");
    #[cfg(target_os = "linux")]
    {
        output.stdout
    }
    #[cfg(target_os = "macos")]
    {
        let hex: String = String::from_utf8(output.stdout)
            .unwrap()
            .split_whitespace()
            .collect();
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn exported_binary_and_empty_attributes_match_the_host_tool() {
    let name = "user.core-transfer";
    let empty = "user.core-empty";
    let bytes = vec![0, 255, 10, 42];
    let (dest, result) = run(
        "oracle",
        &mut Source {
            attributes: Some(vec![
                ExtendedAttribute {
                    name: name.into(),
                    value: bytes.clone(),
                },
                ExtendedAttribute {
                    name: empty.into(),
                    value: vec![],
                },
            ]),
        },
        true,
    );
    assert_eq!(count(&result, "completed"), 1, "{:?}", result.report);
    assert_eq!(host_attribute(&dest, name), bytes);
    assert_eq!(host_attribute(&dest, empty), Vec::<u8>::new());
    std::fs::remove_file(dest).unwrap();
}
#[cfg(target_os = "linux")]
#[test]
fn a_privileged_namespace_is_refused_by_the_host() {
    // security.capability has a kernel-defined value format. One arbitrary byte
    // is refused even on a privileged runner; ordinary users also lack access.
    let (dest, result) = run(
        "namespace",
        &mut Source {
            attributes: Some(vec![ExtendedAttribute {
                name: "security.capability".into(),
                value: vec![255],
            }]),
        },
        true,
    );
    assert_eq!(count(&result, "refused"), 1);
    assert!(format!("{:?}", result.report).contains("security.capability"));
    std::fs::remove_file(dest).unwrap();
}
