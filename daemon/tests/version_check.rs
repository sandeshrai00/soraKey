/// manifest.json and Cargo.toml versions are bumped together; the freshness
/// check trusts the manifest. One fails-to-match and prebuilt matching breaks
/// silently, so pin them.
#[test]
fn manifest_and_cargo_versions_match() {
    let manifest_raw =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../manifest.json"))
            .expect("read manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_raw).expect("parse manifest.json");
    let manifest_version = manifest["version"]
        .as_str()
        .expect("manifest version is a string");

    let cargo_toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("read Cargo.toml");
    // Strict-ish parse without a TOML dependency: `version` key, optional
    // spacing, `=`, quoted value. Same strictness as the release workflow
    // and sora-build.sh tomllib reads (no bare-substring matching).
    let cargo_version = cargo_toml
        .lines()
        .map(str::trim)
        .find_map(|line| {
            let rest = line.strip_prefix("version")?;
            let rest = rest.trim_start().strip_prefix('=')?;
            let v = rest.trim().trim_matches('"');
            (!v.is_empty()).then_some(v.to_string())
        })
        .expect("Cargo.toml has a version line");

    assert_eq!(
        manifest_version, cargo_version,
        "manifest.json and daemon/Cargo.toml must be bumped together"
    );
}
