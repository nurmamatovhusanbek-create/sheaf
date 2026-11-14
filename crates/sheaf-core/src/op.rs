//! DATA-OP: the only way to change a workspace.

use crate::model::{Filter, NormRect, PageContent, PageId, Rot};
use serde::{Deserialize, Serialize};

/// A page about to be inserted. The workspace assigns its [`PageId`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NewPage {
    pub content: PageContent,
    pub has_text: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    Insert {
        at: usize,
        pages: Vec<NewPage>,
    },
    Delete {
        ids: Vec<PageId>,
    },
    /// Moved pages keep their relative order. `to` is the index they start at
    /// in the list that remains once they have been lifted out.
    Move {
        ids: Vec<PageId>,
        to: usize,
    },
    Rotate {
        ids: Vec<PageId>,
        delta: Rot,
    },
    Crop {
        ids: Vec<PageId>,
        rect: Option<NormRect>,
    },
    SetFilter {
        ids: Vec<PageId>,
        filter: Filter,
    },
    /// Each copy lands right after its original and gets a fresh id.
    Duplicate {
        ids: Vec<PageId>,
    },
    /// `size` is width and height in PDF points.
    InsertBlank {
        at: usize,
        size: (f32, f32),
    },
}
