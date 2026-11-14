//! TEST-OPS and TEST-UNDO: random ops on random workspaces keep the
//! invariants, and undo/redo are exact inverses.
//!
//! A small deterministic generator is used instead of a property-testing
//! crate to keep the dependency set minimal (SEC-SUPPLY).

use sheaf_core::consts::MAX_UNDO;
use sheaf_core::*;
use std::collections::BTreeSet;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

const SOURCE_PAGES: u32 = 40;

fn workspace_with_source() -> (Workspace, SourceId) {
    let mut ws = Workspace::new();
    let src = ws.add_source(|id| Source {
        id,
        path: "a.pdf".into(),
        kind: SourceKind::Pdf,
        sha256: [0; 32],
        page_count: SOURCE_PAGES,
        encrypted: false,
        unlocked: true,
    });
    (ws, src)
}

fn pick_ids(rng: &mut Rng, ws: &Workspace) -> Vec<PageId> {
    let all: Vec<PageId> = ws.all_pages().map(|p| p.id).collect();
    let want = 1 + rng.below(all.len().min(5));
    let mut set = BTreeSet::new();
    while set.len() < want {
        set.insert(all[rng.below(all.len())]);
    }
    // Hand them over in arbitrary order: ops must not depend on it.
    let mut ids: Vec<PageId> = set.into_iter().collect();
    if rng.coin() {
        ids.reverse();
    }
    ids
}

fn random_op(rng: &mut Rng, ws: &Workspace, src: SourceId) -> Op {
    let kind = if ws.is_empty() {
        rng.below(2)
    } else {
        rng.below(8)
    };
    match kind {
        0 => Op::Insert {
            at: rng.below(ws.len() + 1),
            pages: (0..1 + rng.below(4))
                .map(|_| NewPage {
                    content: PageContent::Source {
                        src,
                        index: rng.below(SOURCE_PAGES as usize) as u32,
                    },
                    has_text: Some(rng.coin()),
                })
                .collect(),
        },
        1 => Op::InsertBlank {
            at: rng.below(ws.len() + 1),
            size: (595.0, 842.0),
        },
        2 => Op::Delete {
            ids: pick_ids(rng, ws),
        },
        3 => {
            let ids = pick_ids(rng, ws);
            let to = rng.below(ws.len() - ids.len() + 1);
            Op::Move { ids, to }
        }
        4 => Op::Rotate {
            ids: pick_ids(rng, ws),
            delta: [Rot::R90, Rot::R180, Rot::R270][rng.below(3)],
        },
        5 => Op::Crop {
            ids: pick_ids(rng, ws),
            rect: rng.coin().then_some(NormRect {
                x0: 0.1,
                y0: 0.2,
                x1: 0.9,
                y1: 0.8,
            }),
        },
        6 => Op::SetFilter {
            ids: pick_ids(rng, ws),
            filter: Filter {
                mode: ColorMode::BW,
                despeckle: rng.coin(),
                ..Default::default()
            },
        },
        _ => Op::Duplicate {
            ids: pick_ids(rng, ws),
        },
    }
}

fn snapshot(ws: &Workspace) -> Vec<Page> {
    ws.all_pages().cloned().collect()
}

fn check_invariants(ws: &Workspace, src: SourceId) {
    let pages = snapshot(ws);
    let ids: BTreeSet<PageId> = pages.iter().map(|p| p.id).collect();
    assert_eq!(ids.len(), pages.len(), "page ids must be unique");
    for (i, p) in pages.iter().enumerate() {
        assert_eq!(ws.index_of(p.id), Some(i));
        if let PageContent::Source { src: s, index } = p.content {
            assert_eq!(s, src);
            assert!(index < SOURCE_PAGES, "source index must stay valid");
        }
        if let Some(r) = p.crop {
            r.validate().expect("stored crop is valid");
        }
    }
    assert_eq!(ws.pages(0..usize::MAX).len(), pages.len());
}

#[test]
fn random_ops_keep_invariants_and_undo_is_exact() {
    for seed in 1..=200u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let (mut ws, src) = workspace_with_source();
        let mut seen_ids = BTreeSet::new();
        for _ in 0..60 {
            let before = snapshot(&ws);
            let rev = ws.rev();
            let op = random_op(&mut rng, &ws, src);
            let new_rev = ws
                .apply(op.clone())
                .unwrap_or_else(|e| panic!("seed {seed}: {op:?}: {e}"));
            assert_eq!(new_rev, rev + 1, "rev increments on every change");
            check_invariants(&ws, src);
            let after = snapshot(&ws);

            // Ids are never reused, even for pages that were deleted earlier.
            let before_ids: BTreeSet<PageId> = before.iter().map(|p| p.id).collect();
            for p in &after {
                if !before_ids.contains(&p.id) {
                    assert!(seen_ids.insert(p.id), "seed {seed}: id {:?} reused", p.id);
                }
            }

            assert!(ws.undo().is_some());
            assert_eq!(snapshot(&ws), before, "seed {seed}: undo of {op:?}");
            assert!(ws.redo().is_some());
            assert_eq!(snapshot(&ws), after, "seed {seed}: redo of {op:?}");
        }
    }
}

#[test]
fn undoing_everything_returns_to_empty_and_redo_replays() {
    let mut rng = Rng(42);
    let (mut ws, src) = workspace_with_source();
    let mut states = vec![snapshot(&ws)];
    for _ in 0..50 {
        let op = random_op(&mut rng, &ws, src);
        ws.apply(op).expect("valid op");
        states.push(snapshot(&ws));
    }
    for expected in states.iter().rev().skip(1) {
        ws.undo().expect("history available");
        assert_eq!(&snapshot(&ws), expected);
    }
    assert!(ws.undo().is_none());
    for expected in states.iter().skip(1) {
        ws.redo().expect("redo available");
        assert_eq!(&snapshot(&ws), expected);
    }
    assert!(ws.redo().is_none());
}

#[test]
fn undo_depth_is_capped() {
    let (mut ws, _) = workspace_with_source();
    for _ in 0..MAX_UNDO + 25 {
        ws.apply(Op::InsertBlank {
            at: 0,
            size: (595.0, 842.0),
        })
        .expect("valid op");
    }
    let mut undone = 0;
    while ws.undo().is_some() {
        undone += 1;
    }
    assert_eq!(undone, MAX_UNDO);
    assert_eq!(ws.len(), 25, "state older than the cap is kept, not lost");
}

#[test]
fn a_new_edit_clears_redo() {
    let (mut ws, _) = workspace_with_source();
    ws.apply(Op::InsertBlank {
        at: 0,
        size: (595.0, 842.0),
    })
    .expect("valid op");
    ws.undo().expect("history available");
    assert!(ws.can_redo());
    ws.apply(Op::InsertBlank {
        at: 0,
        size: (300.0, 300.0),
    })
    .expect("valid op");
    assert!(!ws.can_redo());
}

#[test]
fn page_ops_produce_the_expected_sequence() {
    // TEST-PAGEOPS at the model level: remove, reorder, duplicate, rotate.
    let (mut ws, src) = workspace_with_source();
    let pages = (0..5).map(|index| NewPage {
        content: PageContent::Source { src, index },
        has_text: None,
    });
    ws.apply(Op::Insert {
        at: 0,
        pages: pages.collect(),
    })
    .expect("insert");
    let id = |ws: &Workspace, i: usize| ws.pages(i..i + 1)[0].id;
    let order = |ws: &Workspace| -> Vec<u32> {
        ws.all_pages()
            .map(|p| match p.content {
                PageContent::Source { index, .. } => index,
                PageContent::Blank { .. } => 99,
            })
            .collect()
    };

    ws.apply(Op::Delete {
        ids: vec![id(&ws, 1)],
    })
    .expect("delete");
    assert_eq!(order(&ws), [0, 2, 3, 4]);

    // Lift pages "4" and "0" (given out of order) and drop them after "2".
    ws.apply(Op::Move {
        ids: vec![id(&ws, 3), id(&ws, 0)],
        to: 1,
    })
    .expect("move");
    assert_eq!(order(&ws), [2, 0, 4, 3]);

    ws.apply(Op::Duplicate {
        ids: vec![id(&ws, 0)],
    })
    .expect("duplicate");
    assert_eq!(order(&ws), [2, 2, 0, 4, 3]);
    assert_ne!(id(&ws, 0), id(&ws, 1));

    ws.apply(Op::InsertBlank {
        at: 5,
        size: (595.0, 842.0),
    })
    .expect("blank");
    assert_eq!(order(&ws), [2, 2, 0, 4, 3, 99]);

    let target = id(&ws, 2);
    ws.apply(Op::Rotate {
        ids: vec![target],
        delta: Rot::R270,
    })
    .expect("rotate");
    ws.apply(Op::Rotate {
        ids: vec![target],
        delta: Rot::R180,
    })
    .expect("rotate");
    assert_eq!(ws.find(target).map(|p| p.rot), Some(Rot::R90));
}

#[test]
fn invalid_ops_are_rejected_and_change_nothing() {
    let (mut ws, src) = workspace_with_source();
    let page = NewPage {
        content: PageContent::Source { src, index: 0 },
        has_text: None,
    };
    ws.apply(Op::Insert {
        at: 0,
        pages: vec![page, page],
    })
    .expect("insert");
    let before = snapshot(&ws);
    let rev = ws.rev();
    let first = before[0].id;
    let bad = [
        Op::Insert {
            at: 3,
            pages: vec![page],
        },
        Op::Insert {
            at: 0,
            pages: vec![],
        },
        Op::Insert {
            at: 0,
            pages: vec![NewPage {
                content: PageContent::Source {
                    src,
                    index: SOURCE_PAGES,
                },
                has_text: None,
            }],
        },
        Op::Insert {
            at: 0,
            pages: vec![NewPage {
                content: PageContent::Source {
                    src: SourceId(77),
                    index: 0,
                },
                has_text: None,
            }],
        },
        Op::InsertBlank {
            at: 0,
            size: (0.0, 842.0),
        },
        Op::InsertBlank {
            at: 0,
            size: (f32::NAN, 842.0),
        },
        Op::Delete { ids: vec![] },
        Op::Delete {
            ids: vec![PageId(9_999)],
        },
        Op::Delete {
            ids: vec![first, first],
        },
        Op::Move {
            ids: vec![first],
            to: 2,
        },
        Op::Crop {
            ids: vec![first],
            rect: Some(NormRect {
                x0: 0.5,
                y0: 0.0,
                x1: 0.4,
                y1: 1.0,
            }),
        },
        Op::Crop {
            ids: vec![first],
            rect: Some(NormRect {
                x0: 0.0,
                y0: 0.0,
                x1: 1.5,
                y1: 1.0,
            }),
        },
        Op::SetFilter {
            ids: vec![first],
            filter: Filter {
                deskew: Deskew::Angle(f32::INFINITY),
                ..Default::default()
            },
        },
    ];
    for op in bad {
        assert!(ws.apply(op.clone()).is_err(), "{op:?} should be rejected");
        assert_eq!(snapshot(&ws), before);
        assert_eq!(ws.rev(), rev);
    }
    assert_eq!(
        ws.undo(),
        Some(rev + 1),
        "history is intact: only the insert is undone"
    );
    assert!(ws.is_empty());
}

#[test]
fn ocr_state_updates_bump_rev_without_touching_history() {
    let (mut ws, _) = workspace_with_source();
    ws.apply(Op::InsertBlank {
        at: 0,
        size: (595.0, 842.0),
    })
    .expect("valid op");
    let id = ws.pages(0..1)[0].id;
    let rev = ws.rev();
    assert_eq!(
        ws.set_ocr_state(id, OcrState::Queued).expect("page exists"),
        rev + 1
    );
    assert_eq!(ws.find(id).map(|p| p.ocr.clone()), Some(OcrState::Queued));
    assert!(ws.set_has_text(PageId(404), true).is_err());
    ws.undo()
        .expect("the insert is still the last undoable step");
    assert!(ws.is_empty());
}

#[test]
fn ops_and_protocol_round_trip_through_json() {
    let op = Op::Crop {
        ids: vec![PageId(3)],
        rect: Some(NormRect {
            x0: 0.0,
            y0: 0.0,
            x1: 0.5,
            y1: 0.5,
        }),
    };
    let json = serde_json::to_string(&op).expect("serialize");
    assert_eq!(serde_json::from_str::<Op>(&json).expect("deserialize"), op);

    use sheaf_core::proto::{Outcome, Response};
    let line = r#"{"id":17,"ok":false,"error":{"code":"ParseError","msg":"bad xref"}}"#;
    let resp: Response = serde_json::from_str(line).expect("parse");
    assert_eq!(
        resp.outcome(),
        Outcome::Failed(AppError::new(ErrorCode::ParseError, "bad xref"))
    );
    let done =
        serde_json::to_string(&Response::done(1, serde_json::json!({"k": 1}))).expect("serialize");
    assert_eq!(done, r#"{"id":1,"ok":true,"result":{"k":1}}"#);
    let progress: Response = serde_json::from_str(r#"{"id":17,"progress":0.4}"#).expect("parse");
    assert_eq!(progress.outcome(), Outcome::Progress(0.4));
}
