//! Opt-in features are the only way in to this crate's dependencies.
//!
//! Every driver's static library links this crate. Its default build has
//! no dependency at all, and that is a property a consumer relies on
//! without checking: a dependency made non-optional here, or a `cli`
//! folded into `default`, would reach every static library in the family
//! with no change in any of them. So `Cargo.toml` is read, with a real TOML
//! parser, and each half is refused.
//!
//! Two features may turn dependencies on: `cli`, which may turn on any, and
//! `layers`, the shared driver API layers, which may turn on exactly serde
//! and sha2 for its serializable evidence and plan digests. Any other
//! feature that turns one on is refused, as is any other dependency under
//! `layers`.

use std::path::Path;

fn manifest() -> toml::Table {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    text.parse().unwrap()
}

#[test]
fn every_dependency_is_optional_and_only_cli_turns_one_on() {
    let manifest = manifest();
    let deps = manifest["dependencies"].as_table().expect("[dependencies]");
    // The parse is asserted before the loop it feeds: an empty table would
    // pass the loop for the wrong reason.
    for name in ["clap", "clap_complete", "clap_mangen"] {
        assert!(deps.contains_key(name), "{name} is not a dependency");
    }
    for (name, spec) in deps {
        let optional = spec
            .as_table()
            .and_then(|t| t.get("optional"))
            .and_then(toml::Value::as_bool);
        assert_eq!(optional, Some(true), "{name} is not optional");
    }

    let features = manifest["features"].as_table().expect("[features]");
    let cli: Vec<&str> = features["cli"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for name in deps.keys() {
        assert!(
            cli.contains(&format!("dep:{name}").as_str()),
            "cli does not turn on {name}"
        );
    }
    // `layers` turns on these two and nothing else.
    let layers: Vec<&str> = features["layers"]
        .as_array()
        .expect("[features] layers")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        layers,
        ["dep:serde", "dep:sha2"],
        "layers turns on more than serde and sha2"
    );
    for (feature, enables) in features {
        if feature == "cli" || feature == "layers" {
            continue;
        }
        for enabled in enables.as_array().unwrap() {
            let enabled = enabled.as_str().unwrap();
            assert!(
                !enabled.starts_with("dep:") && enabled != "cli",
                "feature {feature} turns on {enabled}"
            );
        }
    }
    assert!(
        !features.contains_key("default"),
        "a default feature set would reach every consumer's static library"
    );
}
