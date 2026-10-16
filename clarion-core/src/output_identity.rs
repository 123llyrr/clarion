//! Owned display facts and stable workspace return affinity. No device handles
//! or configuration parser enter the core; adapters supply the current facts.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputIdentity {
    pub connector: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
}

impl OutputIdentity {
    pub fn connector_only(connector: String) -> Self {
        Self {
            connector,
            make: None,
            model: None,
            serial: None,
        }
    }

    pub fn canonical_name(&self) -> String {
        if self.make.is_none() && self.model.is_none() && self.serial.is_none() {
            self.connector.clone()
        } else {
            format!(
                "{} {} {}",
                self.make.as_deref().unwrap_or("Unknown"),
                self.model.as_deref().unwrap_or("Unknown"),
                self.serial.as_deref().unwrap_or("Unknown")
            )
        }
    }

    /// Preserve the public configuration's connector or make/model/serial
    /// aliases, including ASCII-insensitive comparison and missing facts.
    /// The matcher allocates nothing and safely rejects split UTF-8 prefixes.
    pub fn matches(&self, selector: &str) -> bool {
        if selector.eq_ignore_ascii_case(&self.connector) {
            return true;
        }
        if self.make.is_none() && self.model.is_none() && self.serial.is_none() {
            return false;
        }
        let make = self.make.as_deref().unwrap_or("Unknown");
        let model = self.model.as_deref().unwrap_or("Unknown");
        let serial = self.serial.as_deref().unwrap_or("Unknown");
        let Some(prefix) = selector.get(..make.len()) else {
            return false;
        };
        if !prefix.eq_ignore_ascii_case(make) {
            return false;
        }
        let Some(rest) = selector
            .get(make.len()..)
            .and_then(|rest| rest.strip_prefix(' '))
        else {
            return false;
        };
        let Some(prefix) = rest.get(..model.len()) else {
            return false;
        };
        if !prefix.eq_ignore_ascii_case(model) {
            return false;
        }
        rest.get(model.len()..)
            .and_then(|rest| rest.strip_prefix(' '))
            .is_some_and(|rest| rest.eq_ignore_ascii_case(serial))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_aliases_keep_connector_and_missing_fact_semantics_without_unicode_aliasing() {
        let connector = OutputIdentity::connector_only("DP-1".into());
        assert!(connector.matches("dp-1"));
        assert!(!connector.matches("Unknown Unknown Unknown"));
        assert_eq!(connector.canonical_name(), "DP-1");
        let display = OutputIdentity {
            connector: "DP-2".into(),
            make: Some("Display Maker".into()),
            model: Some("屏幕".into()),
            serial: None,
        };
        assert!(display.matches("DISPLAY MAKER 屏幕 unknown"));
        assert!(display.matches("dp-2"));
        assert!(!display.matches("DISPLAY MAKER 屏幕 unknown "));
        assert!(!display.matches("DISPLAY MAKER 屏 unknown"));
        assert!(!display.matches("显Maker 屏幕 Unknown"));
        assert_eq!(display.canonical_name(), "Display Maker 屏幕 Unknown");
    }
}
