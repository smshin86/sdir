use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneArrangement {
    Rows,
    Columns,
    MainLeft,
    MainRight,
    MainTop,
    MainBottom,
    #[serde(other)]
    #[default]
    Standard,
}

#[derive(Clone, Copy)]
pub struct PaneLayout {
    pub count: usize,
    pub arrangement: PaneArrangement,
    pub name: &'static str,
    pub rows: &'static [&'static str],
}

use PaneArrangement::*;
pub const PANE_LAYOUTS: &[PaneLayout] = &[
    PaneLayout {
        count: 1,
        arrangement: Standard,
        name: "한 패널",
        rows: &["a"],
    },
    PaneLayout {
        count: 2,
        arrangement: Standard,
        name: "좌우",
        rows: &["ab"],
    },
    PaneLayout {
        count: 2,
        arrangement: Rows,
        name: "상하",
        rows: &["a", "b"],
    },
    PaneLayout {
        count: 3,
        arrangement: Standard,
        name: "왼쪽 크게",
        rows: &["ab", "ac"],
    },
    PaneLayout {
        count: 3,
        arrangement: MainRight,
        name: "오른쪽 크게",
        rows: &["ab", "cb"],
    },
    PaneLayout {
        count: 3,
        arrangement: MainTop,
        name: "위쪽 크게",
        rows: &["aa", "bc"],
    },
    PaneLayout {
        count: 3,
        arrangement: MainBottom,
        name: "아래쪽 크게",
        rows: &["ab", "cc"],
    },
    PaneLayout {
        count: 3,
        arrangement: Columns,
        name: "가로로 나란히",
        rows: &["abc"],
    },
    PaneLayout {
        count: 3,
        arrangement: Rows,
        name: "세로로 나란히",
        rows: &["a", "b", "c"],
    },
    PaneLayout {
        count: 4,
        arrangement: Standard,
        name: "격자",
        rows: &["ab", "cd"],
    },
    PaneLayout {
        count: 4,
        arrangement: Columns,
        name: "가로로 나란히",
        rows: &["abcd"],
    },
    PaneLayout {
        count: 4,
        arrangement: Rows,
        name: "세로로 나란히",
        rows: &["a", "b", "c", "d"],
    },
    PaneLayout {
        count: 4,
        arrangement: MainLeft,
        name: "왼쪽 크게",
        rows: &["ab", "ac", "ad"],
    },
    PaneLayout {
        count: 4,
        arrangement: MainRight,
        name: "오른쪽 크게",
        rows: &["ab", "cb", "db"],
    },
    PaneLayout {
        count: 4,
        arrangement: MainTop,
        name: "위쪽 크게",
        rows: &["aaa", "bcd"],
    },
    PaneLayout {
        count: 4,
        arrangement: MainBottom,
        name: "아래쪽 크게",
        rows: &["abc", "ddd"],
    },
];

impl PaneArrangement {
    pub fn layout(self, count: usize) -> &'static PaneLayout {
        PANE_LAYOUTS
            .iter()
            .find(|option| option.count == count && option.arrangement == self)
            .or_else(|| PANE_LAYOUTS.iter().find(|option| option.count == count))
            .unwrap_or(&PANE_LAYOUTS[1])
    }
}
