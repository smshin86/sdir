use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchPreferences {
    pub instant: bool,
    pub select_on_focus: bool,
    pub focus_on_open: bool,
    pub memory_mib: u16,
}
impl Default for SearchPreferences {
    fn default() -> Self {
        Self {
            instant: true,
            select_on_focus: true,
            focus_on_open: true,
            memory_mib: 1024,
        }
    }
}
impl SearchPreferences {
    pub fn normalize(&mut self) {
        if ![256, 512, 1024, 2048].contains(&self.memory_mib) {
            self.memory_mib = 1024;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Columns {
    pub custom: bool,
    pub visible: [bool; 4],
    pub widths: [u16; 4],
}
impl Default for Columns {
    fn default() -> Self {
        Self {
            custom: false,
            visible: [true; 4],
            widths: [240, 90, 100, 140],
        }
    }
}
impl Columns {
    pub fn normalize(&mut self) {
        self.visible[0] = true;
        for (i, w) in self.widths.iter_mut().enumerate() {
            *w = (*w).clamp(if i == 0 { 120 } else { 50 }, 800);
        }
    }
    pub fn template(&self) -> String {
        self.widths
            .iter()
            .enumerate()
            .filter(|(i, _)| self.visible[*i])
            .map(|(_, w)| format!("{w}px"))
            .collect::<Vec<_>>()
            .join(" ")
    }
    pub fn total(&self) -> u32 {
        self.widths
            .iter()
            .enumerate()
            .filter(|(i, _)| self.visible[*i])
            .map(|(_, w)| u32::from(*w))
            .sum::<u32>()
            + 50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_search_preferences_keep_legacy_defaults_and_round_trip() {
        let legacy: SearchPreferences = serde_json::from_str(r#"{"instant":false}"#).unwrap();
        assert!(!legacy.instant);
        assert!(legacy.select_on_focus);
        assert!(legacy.focus_on_open);
        assert_eq!(legacy.memory_mib, 1024);

        let current = SearchPreferences {
            instant: false,
            select_on_focus: false,
            focus_on_open: true,
            memory_mib: 512,
        };
        let encoded = serde_json::to_value(&current).unwrap();
        let keys = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            ["focus_on_open", "instant", "memory_mib", "select_on_focus"]
                .into_iter()
                .collect()
        );
        let decoded: SearchPreferences = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, current);
        assert_eq!(SearchPreferences::default().memory_mib, 1024);
    }
}
