//! Dockable layout built on `egui_tiles`: every pane sits in a tab bar; tabs can be dragged
//! onto any edge or into another tab bar, splits can be resized, and panes other than the
//! model view can be closed (hidden) and reopened from the ribbon.
//!
//! Default arrangement: Display left, Navigator right, Tables below the model view.
//! (`egui_tiles` has no floating windows; `egui_dock` could add those once it supports the
//! egui version in use.)

use eframe::egui;
use egui_tiles::{Container, Tile, TileId, Tiles, Tree};

/// The panes of the main window.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, serde::Serialize, serde::Deserialize)]
pub enum Pane {
    /// The 3D model space.
    Viewport,
    /// Model summary, display toggles, grid and view settings.
    Display,
    /// Tree explorer of the model database.
    Navigator,
    /// Object tables of the model database.
    Tables,
}

impl Pane {
    pub const ALL: [Pane; 4] = [Pane::Viewport, Pane::Display, Pane::Navigator, Pane::Tables];

    /// Panes that can be closed and reopened (all but the model view).
    pub const CLOSABLE: [Pane; 3] = [Pane::Display, Pane::Navigator, Pane::Tables];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Viewport => "Model",
            Pane::Display => "Display",
            Pane::Navigator => "Navigator",
            Pane::Tables => "Tables",
        }
    }
}

fn set_share(tiles: &mut Tiles<Pane>, container: TileId, child: TileId, share: f32) {
    if let Some(Tile::Container(Container::Linear(linear))) = tiles.get_mut(container) {
        linear.shares.set_share(child, share);
    }
}

/// The default arrangement.
pub fn default_tree() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let viewport = tiles.insert_pane(Pane::Viewport);
    let display = tiles.insert_pane(Pane::Display);
    let navigator = tiles.insert_pane(Pane::Navigator);
    let tables = tiles.insert_pane(Pane::Tables);

    let viewport_tabs = tiles.insert_tab_tile(vec![viewport]);
    let left = tiles.insert_tab_tile(vec![display]);
    let right = tiles.insert_tab_tile(vec![navigator]);
    let bottom = tiles.insert_tab_tile(vec![tables]);

    let centre = tiles.insert_vertical_tile(vec![viewport_tabs, bottom]);
    set_share(&mut tiles, centre, viewport_tabs, 0.72);
    set_share(&mut tiles, centre, bottom, 0.28);

    let root = tiles.insert_horizontal_tile(vec![left, centre, right]);
    set_share(&mut tiles, root, left, 0.19);
    set_share(&mut tiles, root, centre, 0.61);
    set_share(&mut tiles, root, right, 0.20);

    Tree::new(egui::Id::unique("femcraft_dock"), root, tiles)
}

/// A layout restored from a previous session is usable if it still holds every pane
/// (a newer FemCraft may have added panes) and the model view is visible.
pub fn is_complete(tree: &Tree<Pane>) -> bool {
    Pane::ALL.iter().all(|p| tree.tiles.find_pane(p).is_some()) && is_open(tree, Pane::Viewport)
}

/// Is the pane shown (present and not closed)?
pub fn is_open(tree: &Tree<Pane>, pane: Pane) -> bool {
    tree.tiles
        .find_pane(&pane)
        .is_some_and(|id| tree.tiles.is_visible(id))
}

/// Shows or hides a pane; a reopened pane comes back where it was.
pub fn set_open(tree: &mut Tree<Pane>, pane: Pane, open: bool) {
    if let Some(id) = tree.tiles.find_pane(&pane) {
        tree.tiles.set_visible(id, open);
        if open {
            // Make it the active tab of its tab bar.
            if let Some(parent) = tree.tiles.parent_of(id)
                && let Some(Tile::Container(Container::Tabs(tabs))) = tree.tiles.get_mut(parent)
            {
                tabs.set_active(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_has_all_panes() {
        let mut tree = default_tree();
        for pane in [Pane::Viewport, Pane::Display, Pane::Navigator, Pane::Tables] {
            assert!(is_open(&tree, pane), "{pane:?}");
        }
        set_open(&mut tree, Pane::Tables, false);
        assert!(!is_open(&tree, Pane::Tables));
        set_open(&mut tree, Pane::Tables, true);
        assert!(is_open(&tree, Pane::Tables));
        assert!(is_complete(&tree));
    }

    #[test]
    fn layout_survives_a_save_and_load() {
        let mut tree = default_tree();
        set_open(&mut tree, Pane::Navigator, false);
        let text = to_ron(&tree);
        let back: Tree<Pane> = ron::from_str(&text).expect("layout reads back");
        assert!(is_complete(&back));
        assert!(!is_open(&back, Pane::Navigator));
    }

    fn to_ron(tree: &Tree<Pane>) -> String {
        ron::to_string(tree).expect("layout serialises")
    }
}
