//! DATA-WORKSPACE: the page list, its revision counter and undo/redo.

use crate::consts::{MAX_UNDO, MAX_WORKSPACE_PAGES};
use crate::error::{AppError, Result};
use crate::model::{OcrState, Page, PageContent, PageId, Source, SourceId};
use crate::op::{NewPage, Op};
use imbl::Vector;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Range;

/// Page sizes accepted for blank pages, in points (about 1 cm to 5 m).
const BLANK_SIDE_PT: std::ops::RangeInclusive<f32> = 28.0..=14_400.0;

#[derive(Debug, Clone, Default)]
pub struct Workspace {
    /// Structure-sharing vector, so each undo snapshot costs O(log n).
    pages: Vector<Page>,
    rev: u64,
    undo: VecDeque<Vector<Page>>,
    redo: Vec<Vector<Page>>,
    next_page_id: u64,
    next_source_id: u32,
    sources: BTreeMap<SourceId, Source>,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }
    pub fn len(&self) -> usize {
        self.pages.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Pages in `range`, clamped to the list.
    pub fn pages(&self, range: Range<usize>) -> Vec<Page> {
        let end = range.end.min(self.pages.len());
        let start = range.start.min(end);
        self.pages
            .iter()
            .skip(start)
            .take(end - start)
            .cloned()
            .collect()
    }
    pub fn all_pages(&self) -> impl Iterator<Item = &Page> {
        self.pages.iter()
    }
    pub fn find(&self, id: PageId) -> Option<&Page> {
        self.pages.iter().find(|p| p.id == id)
    }
    pub fn index_of(&self, id: PageId) -> Option<usize> {
        self.pages.iter().position(|p| p.id == id)
    }

    /// Register a source; pages can then point into it. Not undoable: sources
    /// outlive the pages that reference them so undo can bring pages back.
    pub fn add_source(&mut self, make: impl FnOnce(SourceId) -> Source) -> SourceId {
        let id = SourceId(self.next_source_id);
        self.next_source_id += 1;
        let mut source = make(id);
        source.id = id;
        self.sources.insert(id, source);
        id
    }
    pub fn source(&self, id: SourceId) -> Option<&Source> {
        self.sources.get(&id)
    }
    pub fn source_mut(&mut self, id: SourceId) -> Option<&mut Source> {
        self.sources.get_mut(&id)
    }

    /// Apply an op. On error the workspace is unchanged.
    pub fn apply(&mut self, op: Op) -> Result<u64> {
        let next = self.applied(&op)?;
        self.undo
            .push_back(std::mem::replace(&mut self.pages, next));
        if self.undo.len() > MAX_UNDO {
            self.undo.pop_front();
        }
        self.redo.clear();
        self.rev += 1;
        Ok(self.rev)
    }

    pub fn undo(&mut self) -> Option<u64> {
        let prev = self.undo.pop_back()?;
        self.redo.push(std::mem::replace(&mut self.pages, prev));
        self.rev += 1;
        Some(self.rev)
    }

    pub fn redo(&mut self) -> Option<u64> {
        let next = self.redo.pop()?;
        self.undo
            .push_back(std::mem::replace(&mut self.pages, next));
        self.rev += 1;
        Some(self.rev)
    }

    /// Record a job result on a page. Not an [`Op`] and not undoable: it
    /// describes the pixels, it is not an edit. Snapshots taken earlier keep
    /// the older state; the OCR cache makes that harmless (OCR-CACHE).
    pub fn set_ocr_state(&mut self, id: PageId, state: OcrState) -> Result<u64> {
        self.update_page(id, |p| p.ocr = state)
    }
    pub fn set_has_text(&mut self, id: PageId, has_text: bool) -> Result<u64> {
        self.update_page(id, |p| p.has_text = Some(has_text))
    }

    fn update_page(&mut self, id: PageId, f: impl FnOnce(&mut Page)) -> Result<u64> {
        let i = self
            .index_of(id)
            .ok_or_else(|| AppError::not_found("page"))?;
        if let Some(p) = self.pages.get_mut(i) {
            f(p);
        }
        self.rev += 1;
        Ok(self.rev)
    }

    fn fresh_id(&mut self) -> PageId {
        let id = PageId(self.next_page_id);
        self.next_page_id += 1;
        id
    }

    /// Positions of `ids` in list order. Rejects empty lists, duplicates and unknown ids.
    fn positions(&self, ids: &[PageId]) -> Result<Vec<usize>> {
        if ids.is_empty() {
            return Err(AppError::invalid("no pages given"));
        }
        let wanted: BTreeSet<PageId> = ids.iter().copied().collect();
        if wanted.len() != ids.len() {
            return Err(AppError::invalid("page listed twice"));
        }
        let found: Vec<usize> = self
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| wanted.contains(&p.id))
            .map(|(i, _)| i)
            .collect();
        if found.len() != wanted.len() {
            return Err(AppError::not_found("page"));
        }
        Ok(found)
    }

    fn check_room(&self, extra: usize) -> Result<()> {
        if self.pages.len() + extra > MAX_WORKSPACE_PAGES {
            return Err(AppError::limit("too many pages in the workspace"));
        }
        Ok(())
    }

    fn check_content(&self, content: &PageContent) -> Result<()> {
        match *content {
            PageContent::Source { src, index } => {
                let source = self
                    .sources
                    .get(&src)
                    .ok_or_else(|| AppError::not_found("source"))?;
                if index >= source.page_count {
                    return Err(AppError::invalid("page index past the end of its source"));
                }
            }
            PageContent::Blank {
                width_pt,
                height_pt,
            } => {
                if !BLANK_SIDE_PT.contains(&width_pt) || !BLANK_SIDE_PT.contains(&height_pt) {
                    return Err(AppError::invalid("blank page size out of range"));
                }
            }
        }
        Ok(())
    }

    fn make_page(&mut self, new: NewPage) -> Page {
        Page {
            id: self.fresh_id(),
            content: new.content,
            rot: Default::default(),
            crop: None,
            filter: Default::default(),
            ocr: OcrState::None,
            has_text: new.has_text,
        }
    }

    /// The page list after `op`, validated up front so a failure changes
    /// nothing (page ids already handed out are simply skipped).
    fn applied(&mut self, op: &Op) -> Result<Vector<Page>> {
        let mut pages = self.pages.clone();
        match op {
            Op::Insert { at, pages: new } => {
                if new.is_empty() {
                    return Err(AppError::invalid("no pages given"));
                }
                if *at > pages.len() {
                    return Err(AppError::invalid("insert position past the end"));
                }
                self.check_room(new.len())?;
                for n in new {
                    self.check_content(&n.content)?;
                }
                let tail = pages.split_off(*at);
                for n in new {
                    let page = self.make_page(*n);
                    pages.push_back(page);
                }
                pages.append(tail);
            }
            Op::InsertBlank { at, size } => {
                if *at > pages.len() {
                    return Err(AppError::invalid("insert position past the end"));
                }
                self.check_room(1)?;
                let content = PageContent::Blank {
                    width_pt: size.0,
                    height_pt: size.1,
                };
                self.check_content(&content)?;
                let page = self.make_page(NewPage {
                    content,
                    has_text: Some(false),
                });
                pages.insert(*at, page);
            }
            Op::Delete { ids } => {
                for i in self.positions(ids)?.into_iter().rev() {
                    pages.remove(i);
                }
            }
            Op::Move { ids, to } => {
                let pos = self.positions(ids)?;
                if *to > pages.len() - pos.len() {
                    return Err(AppError::invalid("move target past the end"));
                }
                let mut lifted = Vec::with_capacity(pos.len());
                for i in pos.into_iter().rev() {
                    lifted.push(pages.remove(i));
                }
                let tail = pages.split_off(*to);
                pages.extend(lifted.into_iter().rev());
                pages.append(tail);
            }
            Op::Rotate { ids, delta } => {
                for i in self.positions(ids)? {
                    if let Some(p) = pages.get_mut(i) {
                        p.rot = p.rot.plus(*delta);
                    }
                }
            }
            Op::Crop { ids, rect } => {
                if let Some(r) = rect {
                    r.validate()?;
                }
                for i in self.positions(ids)? {
                    if let Some(p) = pages.get_mut(i) {
                        p.crop = *rect;
                    }
                }
            }
            Op::SetFilter { ids, filter } => {
                filter.validate()?;
                for i in self.positions(ids)? {
                    if let Some(p) = pages.get_mut(i) {
                        p.filter = *filter;
                    }
                }
            }
            Op::Duplicate { ids } => {
                let pos = self.positions(ids)?;
                self.check_room(pos.len())?;
                for i in pos.into_iter().rev() {
                    if let Some(original) = pages.get(i).cloned() {
                        let copy = Page {
                            id: self.fresh_id(),
                            ..original
                        };
                        pages.insert(i + 1, copy);
                    }
                }
            }
        }
        Ok(pages)
    }
}
