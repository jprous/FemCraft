//! A session without a window: open models kept in memory (used by `femcraft --mcp`).

use std::path::{Path, PathBuf};

use femcraft_rfdb::RfDocument;

use crate::{ModelInfo, Workspace};

pub struct Headless {
    /// The blank model behind File > New.
    pub blank_model: Option<PathBuf>,
    /// RFEM files new objects are copied from (see [`RfDocument::prototype_files`]).
    pub prototype_files: Vec<PathBuf>,
    docs: Vec<(PathBuf, RfDocument)>,
    active: Option<usize>,
}

impl Headless {
    pub fn new(blank_model: Option<PathBuf>, prototype_files: Vec<PathBuf>) -> Self {
        Self {
            blank_model,
            prototype_files,
            docs: Vec::new(),
            active: None,
        }
    }

    fn push(&mut self, path: PathBuf, mut doc: RfDocument) -> String {
        doc.prototype_files = self.prototype_files.clone();
        let name = title(&path);
        self.docs.push((path, doc));
        self.active = Some(self.docs.len() - 1);
        name
    }
}

fn title(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl Workspace for Headless {
    fn new_model(&mut self, name: Option<&str>) -> Result<String, String> {
        let blank = self
            .blank_model
            .clone()
            .filter(|p| p.is_file())
            .ok_or("no blank model: run File > New once in FemCraft to choose the template")?;
        let doc = RfDocument::open(&blank).map_err(|e| e.to_string())?;
        let name = match name {
            Some(n) => n.to_owned(),
            None => (1..)
                .map(|n| format!("Model {n}"))
                .find(|n| !self.docs.iter().any(|d| title(&d.0) == *n))
                .unwrap_or_default(),
        };
        // No extension: the model is untitled until save_as.
        Ok(self.push(PathBuf::from(name), doc))
    }

    fn open_model(&mut self, path: &Path) -> Result<String, String> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        if let Some(i) = self.docs.iter().position(|d| d.0 == abs) {
            self.active = Some(i);
            return Ok(title(&abs));
        }
        let doc = RfDocument::open(&abs).map_err(|e| format!("{}: {e}", abs.display()))?;
        Ok(self.push(abs, doc))
    }

    fn save_model(&mut self, path: Option<&Path>) -> Result<PathBuf, String> {
        let (own, doc) = self
            .active
            .and_then(|i| self.docs.get_mut(i))
            .ok_or("no model is open")?;
        let target = match path {
            Some(p) => std::path::absolute(p).unwrap_or_else(|_| p.to_owned()),
            None if own
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("rf6")) =>
            {
                own.clone()
            }
            None => return Err("the model is untitled; use save_as".into()),
        };
        doc.save(&target).map_err(|e| e.to_string())?;
        *own = target.clone();
        Ok(target)
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.docs
            .iter()
            .enumerate()
            .map(|(i, (p, d))| ModelInfo {
                name: title(p),
                path: p.clone(),
                active: self.active == Some(i),
                modified: d.model.is_modified(),
            })
            .collect()
    }

    fn active(&mut self) -> Option<&mut RfDocument> {
        self.docs.get_mut(self.active?).map(|d| &mut d.1)
    }
}
