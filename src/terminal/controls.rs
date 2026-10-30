//! Shared pure geometry and semantic styling for terminal controls.

use crate::pane_layout::Rect;

/// Semantic visual state of a control. These states describe presentation only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlState {
    Normal,
    Focus,
    Pressed,
    Disabled,
    Pending,
}

/// Stable style role, independent of terminal color capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlRole {
    Secondary,
    Primary,
    Destructive,
}

/// Pure clipped layout result shared by painting and pointer hit testing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlLayout {
    pub bounds: Rect,
    pub label: Rect,
}

/// Lay out a single row filled control with one cell of side padding.
pub fn layout_filled(bounds: Rect, label_width: u16) -> Option<ControlLayout> {
    if bounds.rows != 1 || bounds.columns < label_width.checked_add(2)? {
        return None;
    }
    bounds.x.checked_add(bounds.columns)?;
    bounds.y.checked_add(bounds.rows)?;
    Some(ControlLayout {
        bounds,
        label: Rect {
            x: bounds.x.checked_add(1)?,
            y: bounds.y,
            rows: 1,
            columns: label_width,
        },
    })
}

/// Terminal-ready style values for a filled control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlStyle {
    pub foreground: u8,
    pub background: u8,
    pub attributes: &'static str,
    pub marker: &'static str,
}

pub const fn style(role: ControlRole, state: ControlState, reduced_color: bool) -> ControlStyle {
    if reduced_color {
        return ControlStyle {
            attributes: match state {
                ControlState::Normal => match role {
                    ControlRole::Primary => "7;1",
                    ControlRole::Destructive => "7;4",
                    ControlRole::Secondary => "7",
                },
                ControlState::Focus => "7;1;4",
                ControlState::Pressed => "7;1;3;4",
                ControlState::Disabled => "7;3",
                ControlState::Pending => "7;3;4",
            },
            foreground: 39,
            background: 49,
            marker: match state {
                ControlState::Focus => ">",
                ControlState::Pressed => "*",
                ControlState::Disabled => "-",
                ControlState::Pending => "~",
                ControlState::Normal => " ",
            },
        };
    }
    match (role, state) {
        (_, ControlState::Disabled) => ControlStyle {
            attributes: "",
            foreground: 37,
            background: 40,
            marker: "-",
        },
        (_, ControlState::Pending) => ControlStyle {
            attributes: "",
            foreground: 30,
            background: 43,
            marker: "~",
        },
        (_, ControlState::Pressed) => ControlStyle {
            attributes: "",
            foreground: 97,
            background: 44,
            marker: "*",
        },
        (_, ControlState::Focus) => ControlStyle {
            attributes: "",
            foreground: 30,
            background: 46,
            marker: ">",
        },
        (ControlRole::Primary, ControlState::Normal) => ControlStyle {
            attributes: "",
            foreground: 97,
            background: 104,
            marker: " ",
        },
        (ControlRole::Destructive, ControlState::Normal) => ControlStyle {
            attributes: "",
            foreground: 97,
            background: 41,
            marker: " ",
        },
        (ControlRole::Secondary, ControlState::Normal) => ControlStyle {
            attributes: "",
            foreground: 97,
            background: 100,
            marker: " ",
        },
    }
}

impl ControlStyle {
    /// Emit a complete style, resetting prior child or chrome attributes.
    pub fn write(self, output: &mut impl std::io::Write) -> std::io::Result<()> {
        write!(output, "\x1b[0;{};{}m", self.foreground, self.background)?;
        if !self.attributes.is_empty() {
            write!(output, "\x1b[{}m", self.attributes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(columns: u16) -> Rect {
        Rect {
            x: 3,
            y: 2,
            rows: 1,
            columns,
        }
    }

    #[test]
    fn filled_layout_includes_padding_and_rejects_clipped_labels() {
        let layout = layout_filled(rect(8), 6).unwrap();
        assert_eq!(layout.bounds, rect(8));
        assert_eq!(
            layout.label,
            Rect {
                x: 4,
                y: 2,
                rows: 1,
                columns: 6
            }
        );
        assert!(layout_filled(rect(7), 6).is_none());
        assert!(layout_filled(rect(8), u16::MAX).is_none());
    }

    #[test]
    fn layout_rejects_multiple_rows_and_coordinate_overflow() {
        for invalid in [
            Rect { rows: 0, ..rect(8) },
            Rect { rows: 2, ..rect(8) },
            Rect {
                x: u16::MAX - 3,
                ..rect(8)
            },
            Rect {
                y: u16::MAX,
                ..rect(8)
            },
        ] {
            assert!(layout_filled(invalid, 4).is_none());
        }
        let valid = layout_filled(
            Rect {
                x: u16::MAX - 8,
                ..rect(8)
            },
            6,
        )
        .unwrap();
        assert!(valid.bounds.contains(valid.label.x, valid.label.y));
        assert!(!valid.bounds.contains(valid.bounds.x + 8, valid.bounds.y));
    }

    #[test]
    fn semantic_states_have_reduced_color_markers() {
        assert_eq!(
            style(ControlRole::Primary, ControlState::Focus, true).marker,
            ">"
        );
        assert_eq!(
            style(ControlRole::Secondary, ControlState::Pressed, true).marker,
            "*"
        );
        assert_eq!(
            style(ControlRole::Destructive, ControlState::Disabled, true).marker,
            "-"
        );
        assert_eq!(
            style(ControlRole::Primary, ControlState::Pending, true).marker,
            "~"
        );
    }
}
