//! Containerd reports the selected OCI manifest and its config as different
//! identities. Release pins identify manifests; container `Image` identifies
//! the config. The daemon's manifest descriptor binds these observations.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ContainerImage<'a> {
    pub manifest_digest: &'a str,
    pub config_digest: &'a str,
}

fn digest<'a>(value: &'a Value, pointer: &str, label: &str) -> Result<&'a str, String> {
    let observed = value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{label} lacks image identity {pointer}"))?;
    if observed.len() != 71
        || !observed.starts_with("sha256:")
        || !observed[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(format!(
            "{label} has invalid image identity {pointer}: {observed:?}"
        ));
    }
    Ok(observed)
}

pub fn read_container_image<'a>(
    value: &'a Value,
    label: &str,
) -> Result<ContainerImage<'a>, String> {
    let media_type = value
        .pointer("/ImageManifestDescriptor/mediaType")
        .and_then(Value::as_str);
    if media_type != Some("application/vnd.oci.image.manifest.v1+json") {
        return Err(format!(
            "{label} lacks the supported OCI container manifest descriptor: {media_type:?}"
        ));
    }
    let manifest_digest = digest(value, "/ImageManifestDescriptor/digest", label)?;
    let config_digest = digest(value, "/Image", label)?;
    let configured = digest(value, "/Config/Image", label)?;
    if configured != manifest_digest {
        return Err(format!("{label} was not launched using its immutable manifest: configured={configured:?}, manifest={manifest_digest:?}"));
    }
    Ok(ContainerImage {
        manifest_digest,
        config_digest,
    })
}

pub fn require_container_image<'a>(
    value: &'a Value,
    expected: &str,
    label: &str,
) -> Result<ContainerImage<'a>, String> {
    let observed = read_container_image(value, label)?;
    if observed.manifest_digest != expected {
        return Err(format!("{label} manifest differs from its pin: expected={expected:?}, observed={:?}, config={:?}",
                           observed.manifest_digest, observed.config_digest));
    }
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::json!({
            "Image": format!("sha256:{}", "2".repeat(64)),
            "ImageManifestDescriptor": {"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": format!("sha256:{}", "1".repeat(64))},
            "Config": {"Image": format!("sha256:{}", "1".repeat(64))}
        })
    }

    #[test]
    fn container_manifest_and_config_are_distinct_observations() {
        let value = fixture();
        let manifest = format!("sha256:{}", "1".repeat(64));
        let observed = require_container_image(&value, &manifest, "fixture").unwrap();
        assert_eq!(observed.manifest_digest, manifest);
        assert_eq!(observed.config_digest, format!("sha256:{}", "2".repeat(64)));
        assert!(
            require_container_image(&value, observed.config_digest, "fixture")
                .unwrap_err()
                .contains("differs from its pin")
        );
    }

    #[test]
    fn absent_or_inconsistent_manifest_evidence_never_uses_config_as_a_pin() {
        for pointer in [
            "/ImageManifestDescriptor/digest",
            "/ImageManifestDescriptor/mediaType",
            "/Image",
            "/Config/Image",
        ] {
            let mut value = fixture();
            *value.pointer_mut(pointer).unwrap() = Value::Null;
            assert!(
                read_container_image(&value, "fixture").is_err(),
                "{pointer}"
            );
        }
        let mut value = fixture();
        value["Config"]["Image"] = value["Image"].clone();
        assert!(read_container_image(&value, "fixture")
            .unwrap_err()
            .contains("not launched using its immutable manifest"));
    }
}
