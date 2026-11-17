//! Docking layout around the model space: left, right and bottom docks, each holding one or
//! more panes as tabs. A pane can be moved to another dock or closed from its tab's context
//! menu, and reopened from the ribbon.
//!
//! Kept deliberately small: `egui_dock` (drag-and-drop docking) targets egui 0.36 at the
//! time of writing; this can be swapped for it once it supports the egui version in use.

use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Dock {
    Left,
    Right,
    Bottom,
}

impl Dock {
    pub const ALL: [Dock; 3] = [Dock::Left, Dock::Right, Dock::Bottom];

    pub fn label(self) -> &'static str {
        match self {
            Dock::Left => "left",
            Dock::Right => "right",
            Dock::Bottom => "bottom",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The dockable panes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Pane {
    /// Model summary, display toggles, grid and view settings.
    Display,
    /// Tree explorer of the model database.
    Navigator,
    /// Object tables of the model database.
    Tables,
}

impl Pane {
    pub const ALL: [Pane; 3] = [Pane::Display, Pane::Navigator, Pane::Tables];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Display => "Display",
            Pane::Navigator => "Navigator",
            Pane::Tables => "Tables",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    pane: Pane,
    dock: Dock,
    open: bool,
}

pub struct DockLayout {
    slots: Vec<Slot>,
    active: [Option<Pane>; 3],
}

impl Default for DockLayout {
    fn default() -> Self {
        Self {
            slots: vec![
                Slot {
                    pane: Pane::Display,
                    dock: Dock::Left,
                    open: true,
                },
                Slot {
                    pane: Pane::Navigator,
                    dock: Dock::Right,
                    open: true,
                },
                Slot {
                    pane: Pane::Tables,
                    dock: Dock::Bottom,
                    open: true,
                },
            ],
            active: [None; 3],
        }
    }
}

impl DockLayout {
    /// Open panes in a dock, in tab order.
    pub fn panes_in(&self, dock: Dock) -> Vec<Pane> {
        self.slots
            .iter()
            .filter(|s| s.open && s.dock == dock)
            .map(|s| s.pane)
            .collect()
    }

    pub fn is_open(&self, pane: Pane) -> bool {
        self.slots.iter().any(|s| s.pane == pane && s.open)
    }

    pub fn set_open(&mut self, pane: Pane, open: bool) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.pane == pane) {
            s.open = open;
            if open {
                self.active[s.dock.index()] = Some(pane);
            }
        }
    }

    pub fn dock_of(&self, pane: Pane) -> Dock {
        self.slots
            .iter()
            .find(|s| s.pane == pane)
            .map_or(Dock::Left, |s| s.dock)
    }

    /// Moves a pane to another dock (at the end of its tabs) and shows it there.
    pub fn move_to(&mut self, pane: Pane, dock: Dock) {
        if let Some(i) = self.slots.iter().position(|s| s.pane == pane) {
            let mut slot = self.slots.remove(i);
            slot.dock = dock;
            slot.open = true;
            self.slots.push(slot);
            self.active[dock.index()] = Some(pane);
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Active pane of a dock (the first open one if none was chosen).
    pub fn active(&mut self, dock: Dock) -> Option<Pane> {
        let panes = self.panes_in(dock);
        let a = &mut self.active[dock.index()];
        if a.is_none_or(|p| !panes.contains(&p)) {
            *a = panes.first().copied();
        }
        *a
    }

    /// Tab strip of a dock: click selects, right-click offers move / close.
    /// Returns the pane to show below it.
    pub fn tab_strip(&mut self, ui: &mut egui::Ui, dock: Dock) -> Option<Pane> {
        let active = self.active(dock);
        let mut action: Option<(Pane, Option<Dock>)> = None; // (pane, Some(dock) = move, None = close)
        ui.horizontal(|ui| {
            for pane in self.panes_in(dock) {
                let tab = ui.selectable_label(active == Some(pane), pane.title());
                if tab.clicked() {
                    self.active[dock.index()] = Some(pane);
                }
                tab.context_menu(|ui| {
                    for target in Dock::ALL {
                        if target != dock && ui.button(format!("Dock {}", target.label())).clicked()
                        {
                            action = Some((pane, Some(target)));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Close").clicked() {
                        action = Some((pane, None));
                        ui.close();
                    }
                });
            }
        });
        match action {
            Some((pane, Some(target))) => self.move_to(pane, target),
            Some((pane, None)) => self.set_open(pane, false),
            None => {}
        }
        self.active(dock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_close_reopen() {
        let mut l = DockLayout::default();
        assert_eq!(l.panes_in(Dock::Left), vec![Pane::Display]);
        l.move_to(Pane::Navigator, Dock::Left);
        assert_eq!(l.panes_in(Dock::Left), vec![Pane::Display, Pane::Navigator]);
        assert!(l.panes_in(Dock::Right).is_empty());
        assert_eq!(l.active(Dock::Left), Some(Pane::Navigator));
        l.set_open(Pane::Navigator, false);
        assert_eq!(l.active(Dock::Left), Some(Pane::Display));
        l.set_open(Pane::Navigator, true);
        assert_eq!(l.dock_of(Pane::Navigator), Dock::Left);
    }
}
