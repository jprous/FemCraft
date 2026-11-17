//! Dockable layout built on `egui_tiles`: every pane sits in a tab bar; tabs can be dragged
//! onto any edge or into another tab bar, splits can be resized, and the panels can be closed
//! (hidden) and reopened from the ribbon / View menu.
//!
//! Each open model is a `Document` pane titled with its file name. New models open as tabs next
//! to the active model, or next to the `Start` tab, which marks the model space and stays there
//! (welcome page with Open and recent files).
//!
//! Default arrangement: Display left, Navigator right, Tables below the model space.
//! (`egui_tiles` has no floating windows; `egui_dock` could add those once it supports the
//! egui version in use.)

use eframe::egui;
use egui_tiles::{Container, Tile, TileId, Tiles, Tree};

/// The panes of the main window.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, serde::Serialize, serde::Deserialize)]
pub enum Pane {
    /// Model space placeholder: welcome page when no model is open.
    Start,
    /// One open model (document id).
    Document(u64),
    /// Model summary, display toggles, grid and view settings.
    Display,
    /// Tree explorer of the model database.
    Navigator,
    /// Object tables of the model database.
    Tables,
}

impl Pane {
    /// Panels that can be closed and reopened.
    pub const CLOSABLE: [Pane; 3] = [Pane::Display, Pane::Navigator, Pane::Tables];
    /// Panes every layout must contain.
    pub const FIXED: [Pane; 4] = [Pane::Start, Pane::Display, Pane::Navigator, Pane::Tables];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Start => "Start",
            Pane::Document(_) => "Model",
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
    let start = tiles.insert_pane(Pane::Start);
    let display = tiles.insert_pane(Pane::Display);
    let navigator = tiles.insert_pane(Pane::Navigator);
    let tables = tiles.insert_pane(Pane::Tables);

    let model_space = tiles.insert_tab_tile(vec![start]);
    let left = tiles.insert_tab_tile(vec![display]);
    let right = tiles.insert_tab_tile(vec![navigator]);
    let bottom = tiles.insert_tab_tile(vec![tables]);

    let centre = tiles.insert_vertical_tile(vec![model_space, bottom]);
    set_share(&mut tiles, centre, model_space, 0.72);
    set_share(&mut tiles, centre, bottom, 0.28);

    let root = tiles.insert_horizontal_tile(vec![left, centre, right]);
    set_share(&mut tiles, root, left, 0.19);
    set_share(&mut tiles, root, centre, 0.61);
    set_share(&mut tiles, root, right, 0.20);

    Tree::new(egui::Id::unique("femcraft_dock"), root, tiles)
}

/// A layout restored from a previous session is usable if it still holds every fixed pane
/// (a newer FemCraft may have added panes).
pub fn is_complete(tree: &Tree<Pane>) -> bool {
    Pane::FIXED
        .iter()
        .all(|p| tree.tiles.find_pane(p).is_some())
}

/// Is the pane shown (present and not closed)?
pub fn is_open(tree: &Tree<Pane>, pane: Pane) -> bool {
    tree.tiles
        .find_pane(&pane)
        .is_some_and(|id| tree.tiles.is_visible(id))
}

/// Shows or hides a pane; a reopened pane comes back where it was, as the active tab.
pub fn set_open(tree: &mut Tree<Pane>, pane: Pane, open: bool) {
    if let Some(id) = tree.tiles.find_pane(&pane) {
        tree.tiles.set_visible(id, open);
        if open {
            focus(tree, pane);
        }
    }
}

/// Brings a pane's tab to the front (in its tab bar and any enclosing tab bars).
pub fn focus(tree: &mut Tree<Pane>, pane: Pane) {
    if let Some(id) = tree.tiles.find_pane(&pane) {
        tree.make_active(|tile_id, _| tile_id == id);
    }
}

/// Adds a model tab next to `beside` (the active model) if it is still there, else next to the
/// Start tab, and brings it to the front.
pub fn add_document(tree: &mut Tree<Pane>, doc: u64, beside: Option<u64>) {
    let anchor = beside
        .and_then(|b| tree.tiles.find_pane(&Pane::Document(b)))
        .or_else(|| tree.tiles.find_pane(&Pane::Start));
    let tile = tree.tiles.insert_pane(Pane::Document(doc));
    let parent = anchor.and_then(|a| tree.tiles.parent_of(a));
    match parent.and_then(|p| tree.tiles.get_mut(p)) {
        Some(Tile::Container(container)) => container.add_child(tile),
        // No anchor (should not happen): put it in a new tab bar beside the root.
        _ => {
            let tabs = tree.tiles.insert_tab_tile(vec![tile]);
            if let Some(root) = tree.root()
                && let Some(Tile::Container(c)) = tree.tiles.get_mut(root)
            {
                c.add_child(tabs);
            }
        }
    }
    focus(tree, Pane::Document(doc));
}

/// Removes a model tab; brings the Start tab to the front when no model is left.
pub fn remove_document(tree: &mut Tree<Pane>, doc: u64) {
    if let Some(tile) = tree.tiles.find_pane(&Pane::Document(doc)) {
        tree.remove_recursively(tile);
    }
    let any_left = tree
        .tiles
        .iter()
        .any(|(_, t)| matches!(t, Tile::Pane(Pane::Document(_))));
    if !any_left {
        set_open(tree, Pane::Start, true);
    }
}

/// The layout to save: model tabs removed (models are not reopened automatically).
pub fn for_saving(tree: &Tree<Pane>) -> Tree<Pane> {
    let mut tree = tree.clone();
    let docs: Vec<u64> = tree
        .tiles
        .iter()
        .filter_map(|(_, t)| match t {
            Tile::Pane(Pane::Document(d)) => Some(*d),
            _ => None,
        })
        .collect();
    for d in docs {
        remove_document(&mut tree, d);
    }
    tree
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(tree: &Tree<Pane>) -> usize {
        tree.tiles
            .iter()
            .filter(|(_, t)| matches!(t, Tile::Pane(Pane::Document(_))))
            .count()
    }

    #[test]
    fn panels_close_and_reopen() {
        let mut tree = default_tree();
        assert!(is_complete(&tree));
        for pane in Pane::FIXED {
            assert!(is_open(&tree, pane), "{pane:?}");
        }
        set_open(&mut tree, Pane::Tables, false);
        assert!(!is_open(&tree, Pane::Tables));
        set_open(&mut tree, Pane::Tables, true);
        assert!(is_open(&tree, Pane::Tables));
    }

    #[test]
    fn documents_open_beside_each_other_and_close() {
        let mut tree = default_tree();
        let start = tree.tiles.find_pane(&Pane::Start).unwrap();
        let model_space = tree.tiles.parent_of(start).unwrap();

        add_document(&mut tree, 1, None);
        add_document(&mut tree, 2, Some(1));
        assert_eq!(docs(&tree), 2);
        assert!(is_open(&tree, Pane::Start), "the Start tab stays");
        for d in [1, 2] {
            let t = tree.tiles.find_pane(&Pane::Document(d)).unwrap();
            assert_eq!(
                tree.tiles.parent_of(t),
                Some(model_space),
                "doc {d} in the model space"
            );
        }

        remove_document(&mut tree, 1);
        assert_eq!(docs(&tree), 1);
        remove_document(&mut tree, 2);
        assert_eq!(docs(&tree), 0);
        assert!(is_open(&tree, Pane::Start));
    }

    #[test]
    fn saved_layout_has_no_documents_and_reads_back() {
        let mut tree = default_tree();
        add_document(&mut tree, 7, None);
        set_open(&mut tree, Pane::Navigator, false);
        let saved = for_saving(&tree);
        assert_eq!(docs(&saved), 0);
        let text = ron::to_string(&saved).expect("layout serialises");
        let back: Tree<Pane> = ron::from_str(&text).expect("layout reads back");
        assert!(is_complete(&back));
        assert!(is_open(&back, Pane::Start));
        assert!(!is_open(&back, Pane::Navigator));
        // The live layout is untouched.
        assert_eq!(docs(&tree), 1);
    }
}
