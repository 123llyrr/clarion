//! Compatibility at the process boundary, before any threads or device backends start.

use std::ffi::OsString;

/// Preserve existing renderer, debug and startup overrides while making Clarion authoritative.
/// This imports environment values only; there is still one compositor and one backend path.
pub fn inherit_legacy_environment() {
    for (key, value) in
        legacy_environment_aliases(std::env::vars_os(), |name| std::env::var_os(name))
    {
        std::env::set_var(key, value);
    }
}

fn legacy_environment_aliases(
    variables: impl IntoIterator<Item = (OsString, OsString)>,
    mut current: impl FnMut(&str) -> Option<OsString>,
) -> Vec<(OsString, OsString)> {
    variables
        .into_iter()
        .filter_map(|(name, value)| {
            let suffix = name.to_str()?.strip_prefix("REALAND_")?;
            let canonical = format!("CLARION_{suffix}");
            current(&canonical)
                .is_none()
                .then(|| (OsString::from(canonical), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_overrides_win_and_other_environment_is_not_copied() {
        let variables = [
            ("REALAND_RENDERER", "vulkan"),
            ("REALAND_DISABLE_FEATURE", "1"),
            ("REALAND_EMPTY_OVERRIDE", "legacy"),
            ("UNRELATED_SECRET", "not an alias"),
        ]
        .map(|(key, value)| (OsString::from(key), OsString::from(value)));
        let aliases = legacy_environment_aliases(variables, |name| match name {
            "CLARION_RENDERER" => Some(OsString::from("gles")),
            "CLARION_EMPTY_OVERRIDE" => Some(OsString::new()),
            _ => None,
        });
        assert_eq!(
            aliases,
            vec![(
                OsString::from("CLARION_DISABLE_FEATURE"),
                OsString::from("1")
            )]
        );
    }
}
