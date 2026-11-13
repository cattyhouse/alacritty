use serde::{Deserialize, Deserializer, Serialize, de};
use toml::Value;

use alacritty_config_derive::{ConfigDeserialize, SerdeReplace};
use alacritty_terminal::term::Osc52;

use crate::config::ui_config::{Program, StringVisitor};

#[derive(ConfigDeserialize, Serialize, Default, Clone, Debug, PartialEq)]
pub struct Terminal {
    /// OSC52 support mode.
    pub osc52: SerdeOsc52,
    /// Report modifiers of keys which the legacy encoding would drop.
    pub extended_key_encoding: SerdeExtendedKeyEncoding,
    /// Path to a shell program to run on startup.
    pub shell: Option<Program>,
}

/// Whether modifiers which the legacy encoding cannot express are reported explicitly.
///
/// Terminals only use the Kitty keyboard protocol when the application asked for it, but a
/// multiplexer like tmux does not forward that request to the outer terminal. Keys which
/// would silently lose their modifiers are therefore encoded explicitly by default.
#[derive(SerdeReplace, Serialize, Clone, Copy, Debug, PartialEq)]
pub struct SerdeExtendedKeyEncoding(pub bool);

impl Default for SerdeExtendedKeyEncoding {
    fn default() -> Self {
        Self(true)
    }
}

impl<'de> Deserialize<'de> for SerdeExtendedKeyEncoding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        bool::deserialize(deserializer).map(Self)
    }
}

#[derive(SerdeReplace, Serialize, Default, Copy, Clone, Debug, PartialEq)]
pub struct SerdeOsc52(pub Osc52);

impl<'de> Deserialize<'de> for SerdeOsc52 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = deserializer.deserialize_str(StringVisitor)?;
        Osc52::deserialize(Value::String(value)).map(SerdeOsc52).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_key_encoding_defaults_to_enabled() {
        let config: Terminal = toml::from_str("").unwrap();
        assert!(config.extended_key_encoding.0);
    }

    #[test]
    fn extended_key_encoding_can_be_disabled() {
        let config: Terminal = toml::from_str("extended_key_encoding = false").unwrap();
        assert!(!config.extended_key_encoding.0);
    }
}
