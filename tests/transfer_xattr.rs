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
    fn named_streams(&mut self, _: &Path) -> Result<Vec<transfer::NamedStream>, CliError> {
        Ok(vec![])
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

struct Legacy(Source);
impl TreeSource for Legacy {
    fn metadata(&mut self, p: &Path) -> Result<TreeMetadata, CliError> {
        self.0.metadata(p)
    }
    fn read_dir(&mut self, p: &Path) -> Result<Vec<TreeEntry>, CliError> {
        self.0.read_dir(p)
    }
    fn read_file_at(&mut self, p: &Path, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        self.0.read_file_at(p, offset, buf)
    }
}
#[test]
fn an_existing_source_defaults_to_explicit_refusal() {
    let (dest, result) = run("legacy", &mut Legacy(Source { attributes: None }), true);
    assert_eq!(count(&result, "refused"), 1);
    assert!(format!("{:?}", result.report).contains("not supported by this source"));
    std::fs::remove_file(dest).unwrap();
}

struct Directory(Source);
impl TreeSource for Directory {
    fn named_streams(&mut self, _: &Path) -> Result<Vec<transfer::NamedStream>, CliError> {
        Ok(vec![])
    }
    fn metadata(&mut self, p: &Path) -> Result<TreeMetadata, CliError> {
        let mut m = self.0.metadata(p)?;
        if p.as_os_str().is_empty() {
            m.kind = TreeKind::Directory;
            m.inode = 2;
        }
        Ok(m)
    }
    fn read_dir(&mut self, _: &Path) -> Result<Vec<TreeEntry>, CliError> {
        Ok(vec![TreeEntry {
            name: "child".into(),
            metadata: self.metadata(Path::new("child"))?,
        }])
    }
    fn read_file_at(&mut self, p: &Path, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        self.0.read_file_at(p, offset, buf)
    }
    fn extended_attributes(&mut self, p: &Path) -> Result<Vec<ExtendedAttribute>, CliError> {
        if p.as_os_str().is_empty() {
            Err(CliError::refused("directory namespace refused"))
        } else {
            Ok(vec![])
        }
    }
}
#[test]
fn a_directory_refusal_keeps_children_running_without_side_files() {
    let dest = destination("directory");
    let result = transfer::export_tree_with_xattrs(
        &ExportRequest {
            path: "".into(),
            output: Output::File(dest.clone()),
            replace: false,
        },
        &mut Directory(Source { attributes: None }),
        TreeOptions {
            recursive: true,
            ..TreeOptions::default()
        },
        LinkOptions::default(),
        TimeOptions::default(),
        XattrOptions {
            preserve_xattrs: true,
        },
    )
    .unwrap();
    assert_eq!(count(&result, "refused"), 1);
    assert_eq!(count(&result, "completed"), 1);
    assert_eq!(std::fs::read(dest.join("child")).unwrap(), b"data");
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 1);
    std::fs::remove_dir_all(dest).unwrap();
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

struct Streams(Option<Vec<transfer::NamedStream>>);
impl TreeSource for Streams {
    fn metadata(&mut self, p: &Path) -> Result<TreeMetadata, CliError> {
        Source { attributes: None }.metadata(p)
    }
    fn read_dir(&mut self, _: &Path) -> Result<Vec<TreeEntry>, CliError> {
        unreachable!()
    }
    fn read_file_at(&mut self, p: &Path, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        Source { attributes: None }.read_file_at(p, offset, buf)
    }
    fn extended_attributes(&mut self, _: &Path) -> Result<Vec<ExtendedAttribute>, CliError> {
        Ok(vec![])
    }
    fn named_streams(&mut self, _: &Path) -> Result<Vec<transfer::NamedStream>, CliError> {
        self.0
            .clone()
            .ok_or_else(|| CliError::refused("stream enumeration unavailable"))
    }
}
#[derive(Default)]
struct StreamSink(Vec<(PathBuf, transfer::NamedStream)>);
impl transfer::NamedStreamSink for StreamSink {
    fn set_named_stream(
        &mut self,
        path: &Path,
        stream: &transfer::NamedStream,
    ) -> Result<(), CliError> {
        self.0.push((path.into(), stream.clone()));
        Ok(())
    }
}
struct NoStreams;
impl transfer::NamedStreamSink for NoStreams {}
fn streams() -> Vec<transfer::NamedStream> {
    vec![
        transfer::NamedStream {
            name: "secret".into(),
            value: vec![0, 255, 10],
        },
        transfer::NamedStream {
            name: "empty".into(),
            value: vec![],
        },
    ]
}
fn preserve_streams(
    source: &mut dyn TreeSource,
    sink: &mut dyn transfer::NamedStreamSink,
    enabled: bool,
) -> Result<(), CliError> {
    transfer::preserve_named_streams(
        source,
        Path::new("file"),
        sink,
        Path::new("destination"),
        XattrOptions {
            preserve_xattrs: enabled,
        },
    )
}
#[test]
fn a_capable_sink_accepts_binary_and_empty_named_streams() {
    let expected = streams();
    let mut sink = StreamSink::default();
    preserve_streams(&mut Streams(Some(expected.clone())), &mut sink, true).unwrap();
    assert_eq!(
        sink.0,
        expected
            .into_iter()
            .map(|s| (PathBuf::from("destination"), s))
            .collect::<Vec<_>>()
    );
}
#[test]
fn an_incapable_sink_refuses_and_names_the_stream() {
    let error = preserve_streams(&mut Streams(Some(streams())), &mut NoStreams, true).unwrap_err();
    assert_eq!(error.code, CliError::refused("").code);
    assert!(error.message.contains("secret"), "{error:?}");
    assert!(error.message.contains("not supported"), "{error:?}");
}
#[test]
fn disabled_stream_preservation_queries_neither_capability() {
    preserve_streams(&mut Streams(None), &mut NoStreams, false).unwrap();
}
#[test]
fn an_empty_stream_list_needs_no_sink_capability() {
    preserve_streams(&mut Streams(Some(vec![])), &mut NoStreams, true).unwrap();
}
#[test]
fn a_stream_enumeration_refusal_reaches_the_caller() {
    let error = preserve_streams(&mut Streams(None), &mut StreamSink::default(), true).unwrap_err();
    assert!(error.message.contains("stream enumeration unavailable"));
}
#[test]
fn a_legacy_source_explicitly_refuses_stream_enumeration() {
    let error = preserve_streams(
        &mut Legacy(Source { attributes: None }),
        &mut StreamSink::default(),
        true,
    )
    .unwrap_err();
    assert_eq!(error.code, CliError::refused("").code);
    assert!(error.message.contains("named streams"));
}
#[test]
fn host_export_names_the_refused_stream_without_creating_side_files() {
    let root = destination("streams-no-side-files");
    std::fs::create_dir(&root).unwrap();
    let dest = root.join("file");
    let result = transfer::export_tree_with_xattrs(
        &ExportRequest {
            path: "file".into(),
            output: Output::File(dest.clone()),
            replace: false,
        },
        &mut Streams(Some(streams())),
        TreeOptions::default(),
        LinkOptions::default(),
        TimeOptions::default(),
        XattrOptions {
            preserve_xattrs: true,
        },
    )
    .unwrap();
    assert_eq!(count(&result, "refused"), 1);
    assert_ne!(result.code, 0);
    let report = format!("{:?}", result.report);
    assert!(
        report.contains("secret") && report.contains("named stream"),
        "{report}"
    );
    assert_eq!(std::fs::read(&dest).unwrap(), b"data");
    let names: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [std::ffi::OsString::from("file")]);
    std::fs::remove_dir_all(root).unwrap();
}
