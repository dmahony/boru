use serde::{Deserialize, Serialize};

pub const SETTINGS_KEY: &str = "desktop_preferences_v1";
const MAX_PAYLOAD_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DesktopPreferences {
    pub theme: Theme,
    pub font_size: u8,
    pub scale: u8,
    pub accent: Option<String>,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self { theme: Theme::Dark, font_size: 16, scale: 100, accent: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme { Light, Dark, Amoled }

impl DesktopPreferences {
    pub fn decode(raw: Option<&str>) -> Result<Self, String> {
        let Some(raw) = raw else { return Ok(Self::default()); };
        if raw.len() > MAX_PAYLOAD_BYTES { return Err("settings payload too large".into()); }
        let value: Self = serde_json::from_str(raw).map_err(|e| format!("invalid desktop preferences: {e}"))?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(12..=24).contains(&self.font_size) { return Err("font_size must be 12..=24".into()); }
        if !(80..=140).contains(&self.scale) { return Err("scale must be 80..=140".into()); }
        if let Some(color) = &self.accent {
            if color.len() != 7 || !color.starts_with('#') || !color[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("accent must be a #RRGGBB color".into());
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string(self).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_legacy_theme_values_are_compatible() {
        assert_eq!(DesktopPreferences::decode(None).unwrap(), DesktopPreferences::default());
        assert_eq!(DesktopPreferences::decode(Some(r#"{"theme":"light"}"#)).unwrap().theme, Theme::Light);
        assert_eq!(DesktopPreferences::decode(Some(r#"{"theme":"amoled"}"#)).unwrap().theme, Theme::Amoled);
    }

    #[test]
    fn preferences_round_trip_and_reject_corruption() {
        let value = DesktopPreferences { theme: Theme::Amoled, font_size: 18, scale: 115, accent: Some("#abcdef".into()) };
        assert_eq!(DesktopPreferences::decode(Some(&value.encode().unwrap())).unwrap(), value);
        assert!(DesktopPreferences::decode(Some("{bad")).is_err());
        assert!(DesktopPreferences::decode(Some(&"x".repeat(MAX_PAYLOAD_BYTES + 1))).is_err());
        assert!(DesktopPreferences { font_size: 2, ..value }.validate().is_err());
    }
}
