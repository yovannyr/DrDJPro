//! Persisted track-table presentation, independent of track identities.
use serde::{Deserialize, Serialize};

pub const COLUMN_COUNT: usize = 13;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableColumns {
    pub version: u8,
    pub order: Vec<i32>,
    pub hidden: Vec<i32>,
    #[serde(default = "default_cover_overlay_enabled")]
    pub cover_overlay_enabled: bool,
}

fn default_cover_overlay_enabled() -> bool {
    true
}

impl Default for TableColumns {
    fn default() -> Self {
        Self {
            version: 1,
            order: vec![6, 11, 0, 3, 7, 8, 12, 4, 5, 9, 10, 2, 1],
            hidden: vec![],
            cover_overlay_enabled: default_cover_overlay_enabled(),
        }
    }
}

impl TableColumns {
    /// Preserve custom order and visibility when adding cover and energy columns.
    pub fn upgrade(mut self) -> Result<Self, String> {
        for (new_id, after_id) in [(11, 6), (12, 8)] {
            if self.order.len() != new_id as usize {
                continue;
            }
            let mut old = self.order.clone();
            old.sort_unstable();
            if self.version != 1
                || old != (0..new_id).collect::<Vec<_>>()
                || self.hidden.len() >= new_id as usize
                || self.hidden.iter().any(|id| !self.order.contains(id))
                || self
                    .hidden
                    .iter()
                    .enumerate()
                    .any(|(i, id)| self.hidden[..i].contains(id))
            {
                return Err("Invalid legacy track-table column preferences".into());
            }
            let position = self
                .order
                .iter()
                .position(|id| *id == after_id)
                .unwrap_or(0)
                + 1;
            self.order.insert(position, new_id);
        }
        self.validate()?;
        Ok(self)
    }
    pub fn validate(&self) -> Result<(), String> {
        let mut sorted = self.order.clone();
        sorted.sort_unstable();
        if self.version != 1
            || sorted != (0..COLUMN_COUNT as i32).collect::<Vec<_>>()
            || self.hidden.len() >= COLUMN_COUNT
            || self.hidden.iter().any(|id| !self.order.contains(id))
            || self
                .hidden
                .iter()
                .enumerate()
                .any(|(i, id)| self.hidden[..i].contains(id))
        {
            return Err("Invalid track-table column preferences".into());
        }
        Ok(())
    }

    pub fn change(&mut self, id: i32, action: i32) -> Result<(), String> {
        let index = self
            .order
            .iter()
            .position(|&column| column == id)
            .ok_or("Unknown table column")?;
        match action {
            0 => {
                if let Some(index) = self.hidden.iter().position(|&column| column == id) {
                    self.hidden.remove(index);
                } else if self.hidden.len() + 1 < COLUMN_COUNT {
                    self.hidden.push(id);
                } else {
                    return Err("At least one table column must remain visible".into());
                }
            }
            -1 if index > 0 => self.order.swap(index, index - 1),
            1 if index + 1 < self.order.len() => self.order.swap(index, index + 1),
            -1 | 1 => return Err("Column is already at the table edge".into()),
            _ => return Err("Unknown table column action".into()),
        }
        self.validate()
    }
}

pub fn label(id: i32) -> &'static str {
    match id {
        0 => "Song",
        1 => "Type",
        2 => "Path",
        3 => "Artist",
        4 => "Album",
        5 => "Length",
        6 => "#",
        7 => "BPM",
        8 => "Key",
        9 => "Gain dB",
        10 => "Analysis",
        11 => "Cover",
        12 => "Energy",
        _ => "Unknown",
    }
}

pub fn sort_label(id: i32) -> &'static str {
    match id {
        0 => "name",
        1 => "type",
        2 => "path",
        3 => "artist",
        4 => "album",
        5 => "duration",
        6 => "id",
        7 => "BPM",
        8 => "key",
        9 => "gain",
        10 => "analysis",
        11 => "cover",
        12 => "energy",
        _ => "unknown",
    }
}

pub fn width(id: i32) -> f32 {
    match id {
        0 => 180.,
        2 => 240.,
        3 | 4 => 140.,
        5 | 9 => 90.,
        6 => 60.,
        7 | 8 | 1 => 70.,
        10 => 100.,
        11 => 36.,
        12 => 110.,
        _ => 70.,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_validate_visibility_order_and_last_visible_column() {
        let mut columns = TableColumns::default();
        columns.change(7, -1).unwrap();
        assert_eq!(&columns.order[..5], &[6, 11, 0, 7, 3]);
        columns.change(7, 0).unwrap();
        assert!(columns.hidden.contains(&7));
        columns.change(7, 0).unwrap();
        for id in 0..12 {
            columns.change(id, 0).unwrap();
        }
        assert!(columns.change(12, 0).is_err());
        assert!(columns.change(99, 0).is_err());
        columns.order[0] = columns.order[1];
        assert!(columns.validate().is_err());
    }

    #[test]
    fn legacy_columns_preserve_custom_order_and_hidden_state() {
        let old: TableColumns = serde_json::from_str(
            r#"{"version":1,"order":[0,6,3,1,2,4,5,7,8,9,10],"hidden":[3,7]}"#,
        )
        .unwrap();
        let upgraded = old.clone().upgrade().unwrap();
        assert!(upgraded.cover_overlay_enabled);
        assert_eq!(
            upgraded
                .order
                .iter()
                .copied()
                .filter(|id| *id < 11)
                .collect::<Vec<_>>(),
            old.order
        );
        assert_eq!(upgraded.hidden, old.hidden);
        assert_eq!(upgraded.clone().upgrade().unwrap(), upgraded);
        let mut bad = old;
        bad.order[0] = 99;
        assert!(bad.upgrade().is_err());
    }

    #[test]
    fn cover_overlay_defaults_for_existing_twelve_columns_and_roundtrips_when_disabled() {
        let old = r#"{"version":1,"order":[11,6,3,0,1,2,4,5,7,8,9,10],"hidden":[0,11]}"#;
        let mut columns = serde_json::from_str::<TableColumns>(old)
            .unwrap()
            .upgrade()
            .unwrap();
        assert!(columns.cover_overlay_enabled);
        assert_eq!(columns.order[0], 11);
        assert_eq!(columns.hidden, [0, 11]);
        columns.cover_overlay_enabled = false;
        let raw = serde_json::to_vec(&columns).unwrap();
        let restored = serde_json::from_slice::<TableColumns>(&raw)
            .unwrap()
            .upgrade()
            .unwrap();
        assert_eq!(restored, columns);
    }
}
