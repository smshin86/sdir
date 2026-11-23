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
    pub const MAX_WIDTH: u16 = 800;

    pub const fn minimum_width(index: usize) -> u16 {
        if index == 0 { 120 } else { 50 }
    }

    pub fn normalize(&mut self) {
        self.visible[0] = true;
        for (index, width) in self.widths.iter_mut().enumerate() {
            *width = (*width).clamp(Self::minimum_width(index), Self::MAX_WIDTH);
        }
    }

    pub fn resize(&mut self, index: usize, width: u16) -> bool {
        if index >= self.widths.len() || !self.visible[index] {
            return false;
        }
        let width = width.clamp(Self::minimum_width(index), Self::MAX_WIDTH);
        if self.widths[index] == width {
            return false;
        }
        self.widths[index] = width;
        self.custom = true;
        true
    }

    pub fn details_style(&self) -> String {
        let variables = self
            .widths
            .iter()
            .enumerate()
            .map(|(index, width)| format!("--detail-column-{index}:{width}px;"))
            .collect::<String>();
        let tracks = self
            .visible
            .iter()
            .enumerate()
            .filter(|(_, visible)| **visible)
            .map(|(index, _)| format!("var(--detail-column-{index})"))
            .collect::<Vec<_>>();
        // Shared by the row and header grids: 13px padding and 7px column gaps.
        let overhead = 26 + 7 * tracks.len().saturating_sub(1);
        format!(
            "{variables}--detail-columns:{};--detail-width:calc({overhead}px + {});",
            tracks.join(" "),
            tracks.join(" + ")
        )
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
