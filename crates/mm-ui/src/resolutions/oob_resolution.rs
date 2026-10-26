//! OOB Resolution — types and state for out-of-band tag resolution.
//!
//! Unified module replacing the former oob_sync + oob_conflict split.
//! All OOB signal files are classified into `ConflictBucket` categories
//! and resolved through a single tabbed interface.
//!
//! Route: `/resolve/oob-resolution`
//! Query: `GetOobSummary` (bucket totals + one bounded page)

use std::borrow::Cow;
use std::collections::BTreeSet;

use ratatui::style::{Color, Style};

use mm_meta::decisions::DecisionKey;
use mm_meta::views::{ConflictBucket, OobFile, OobSummary, TagMismatchEntry};

use crate::geometry::FocusPane;
use crate::input::InputAction;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrameCore};
use crate::protocol_binding::ProtocolBinding;
use crate::rich_text::{RichBlock, RichSpan};
use crate::standard_list::{ListEntry, StandardListConfig, StandardListState};
use crate::wizard::{WizardItem, WizardOffer};

// ============================================================================
// ListEntry + WizardItem for OobFile
// ============================================================================

impl ListEntry for OobFile {
    type Action = OobAction;

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<OobAction> {
        None // buttons handle resolution, not list items
    }
}

impl WizardItem for OobFile {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        if self.mismatches.is_empty() {
            return None;
        }

        let mut content = Vec::new();
        for m in &self.mismatches {
            let db = m.db_value.as_deref().unwrap_or("\u{2014}");
            let disk = m.disk_value.as_deref().unwrap_or("\u{2014}");
            content.push(RichBlock::Paragraph(vec![
                RichSpan::new(&m.field, Style::default().fg(Color::White)),
                RichSpan::new(": ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(db, Style::default().fg(Color::Green)),
                RichSpan::new(" \u{2192} ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(disk, Style::default().fg(Color::Cyan)),
            ]));
        }

        Some(WizardOffer::Pane {
            title: format!("Tag Diff: {}", self.path),
            content,
        })
    }
}

// ============================================================================
// Button types
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub struct OobButtonCtx {
    pub is_resolvable: bool,
    pub is_acknowledgeable: bool,
    pub has_files: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OobButton {
    #[default]
    ApplyDb,
    AssimilateDisk,
    Acknowledge,
    Cancel,
}

impl ModalButtons for OobButton {
    type Context = OobButtonCtx;
    type Action = OobAction;

    fn all() -> &'static [Self] {
        &[Self::ApplyDb, Self::AssimilateDisk, Self::Acknowledge, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::ApplyDb => "Apply DB -> Files".into(),
            Self::AssimilateDisk => "Assimilate Files -> DB".into(),
            Self::Acknowledge => "Acknowledge Mtime".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::ApplyDb => Color::Green,
            Self::AssimilateDisk => Color::Cyan,
            Self::Acknowledge => Color::Green,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::ApplyDb => ctx.is_resolvable && ctx.has_files,
            Self::AssimilateDisk => ctx.is_resolvable && ctx.has_files,
            Self::Acknowledge => ctx.is_acknowledgeable && ctx.has_files,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> OobAction {
        match self {
            Self::ApplyDb => OobAction::ApplyDb,
            Self::AssimilateDisk => OobAction::AssimilateDisk,
            Self::Acknowledge => OobAction::Acknowledge,
            Self::Cancel => OobAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::ApplyDb => ProtocolBinding::Transaction {
                decision_key: DecisionKey::OobResolution { bucket: ConflictBucket::Conflict },
                label: "Apply DB tags \u{2192} files".into(),
            },
            Self::AssimilateDisk => ProtocolBinding::Transaction {
                decision_key: DecisionKey::OobResolution { bucket: ConflictBucket::Conflict },
                label: "Assimilate file tags \u{2192} DB".into(),
            },
            Self::Acknowledge => ProtocolBinding::Transaction {
                decision_key: DecisionKey::OobResolution { bucket: ConflictBucket::MtimeOnly },
                label: "Acknowledge mtime changes".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OobAction {
    None,
    Navigate,
    /// The active tab changed — the held page belongs to the previous bucket
    /// and the caller must re-query before rendering.
    BucketChanged,
    ApplyDb,
    AssimilateDisk,
    Acknowledge,
    Cancel,
}

// ============================================================================
// Per-bucket file state
// ============================================================================

pub struct OobBucketState {
    pub files: Vec<OobFile>,
    pub list: StandardListState,
}

impl OobBucketState {
    /// Build a page's list state with nothing selected.
    ///
    /// An empty selection means "the whole bucket", which is both the default
    /// action and the only one that scales — `files` is a bounded window, so
    /// pre-selecting it would silently mean "the first thousand" on a bucket
    /// holding half a million rows.
    pub fn new(files: Vec<OobFile>) -> Self {
        Self {
            files,
            list: StandardListState::new(StandardListConfig {
                multi_select: true,
                ..Default::default()
            }),
        }
    }

    pub fn current_file(&self) -> Option<&OobFile> {
        self.files.get(self.list.cursor)
    }

    fn navigate_up(&mut self) -> bool {
        if self.list.cursor > 0 {
            self.list.cursor -= 1;
            true
        } else {
            false
        }
    }

    fn navigate_down(&mut self) -> bool {
        if self.list.cursor + 1 < self.files.len() {
            self.list.cursor += 1;
            true
        } else {
            false
        }
    }

    fn page_up(&mut self) -> bool {
        let old = self.list.cursor;
        self.list.cursor = self.list.cursor.saturating_sub(20);
        self.list.cursor != old
    }

    fn page_down(&mut self) -> bool {
        let old = self.list.cursor;
        self.list.cursor = (self.list.cursor + 20).min(self.files.len().saturating_sub(1));
        self.list.cursor != old
    }
}

// ============================================================================
// OOB Resolution State
// ============================================================================

/// One bucket's totals plus a window into the active one.
///
/// Only the active bucket's page is held. The other three contribute their
/// counts and nothing else — switching tabs re-queries, because holding every
/// bucket's rows is exactly what made this modal unopenable during a bulk tag
/// rewrite.
pub struct OobResolutionState {
    pub active_bucket: ConflictBucket,
    /// Totals for every bucket, indexed by [`ConflictBucket::index`].
    pub bucket_counts: [usize; 4],
    /// Window into the active bucket, ordered by path.
    page: OobBucketState,
    /// Index of the window's first entry within the active bucket.
    page_offset: usize,
    pub frame: FrameState<OobButton>,
}

impl OobResolutionState {
    pub fn new(summary: OobSummary) -> Self {
        let mut frame = FrameState::new();
        if summary.bucket.is_acknowledgeable() {
            frame.buttons.selected = OobButton::Acknowledge;
        }

        Self {
            active_bucket: summary.bucket,
            bucket_counts: summary.counts,
            page_offset: summary.offset,
            page: OobBucketState::new(summary.files),
            frame,
        }
    }

    /// Swap in a freshly queried page after a bucket switch.
    ///
    /// Cursor and selection reset with the page: both index into the window,
    /// and the window has been replaced.
    pub fn set_page(&mut self, summary: OobSummary) {
        self.active_bucket = summary.bucket;
        self.bucket_counts = summary.counts;
        self.page_offset = summary.offset;
        self.page = OobBucketState::new(summary.files);
        let ctx = self.button_ctx();
        self.frame.buttons.clamp(&ctx);
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.active_bucket_state()
            .current_file()
            .map(|f| f.path.as_str())
    }

    pub fn active_bucket_state(&self) -> &OobBucketState {
        &self.page
    }

    pub fn active_bucket_state_mut(&mut self) -> &mut OobBucketState {
        &mut self.page
    }

    /// Rows in the active bucket — the count, not the page size.
    pub fn active_bucket_total(&self) -> usize {
        self.bucket_counts[self.active_bucket.index()]
    }

    /// Whether the active bucket holds more rows than the page shows.
    pub fn is_truncated(&self) -> bool {
        self.page_offset + self.page.files.len() < self.active_bucket_total()
    }

    pub fn total_files(&self) -> usize {
        self.bucket_counts.iter().sum()
    }

    /// Whether the operator has narrowed the action to specific visible rows.
    ///
    /// No selection means the whole bucket, matching the button labels.
    pub fn has_explicit_selection(&self) -> bool {
        !self.page.list.selected.is_empty()
    }

    pub fn button_ctx(&self) -> OobButtonCtx {
        OobButtonCtx {
            is_resolvable: self.active_bucket.is_resolvable(),
            is_acknowledgeable: self.active_bucket.is_acknowledgeable(),
            has_files: self.active_bucket_total() > 0,
        }
    }

    pub fn current_mismatches(&self) -> &[TagMismatchEntry] {
        self.active_bucket_state()
            .current_file()
            .map(|f| f.mismatches.as_slice())
            .unwrap_or(&[])
    }

    pub fn handle_click(&mut self, x: u16, y: u16) -> Option<OobAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            Some(action)
        } else {
            None
        }
    }

    pub fn handle_input(&mut self, action: &InputAction) -> OobAction {
        match action {
            // Select all / deselect all
            InputAction::TextHome => {
                let bucket = self.active_bucket_state_mut();
                let all_count = bucket.files.len();
                let all_selected = bucket.list.selected.len() == all_count;
                if all_selected {
                    bucket.list.selected.clear();
                } else {
                    for i in 0..all_count { bucket.list.selected.insert(i); }
                }
                return OobAction::None;
            }
            InputAction::Toggle => {
                let bucket = self.active_bucket_state_mut();
                if !bucket.files.is_empty() {
                    let cursor = bucket.list.cursor;
                    if bucket.list.selected.contains(&cursor) {
                        bucket.list.selected.remove(&cursor);
                    } else {
                        bucket.list.selected.insert(cursor);
                    }
                }
                return OobAction::None;
            }
            InputAction::CycleNext => {
                self.active_bucket = self.active_bucket.next();
                let ctx = self.button_ctx();
                self.frame.buttons.clamp(&ctx);
                return OobAction::BucketChanged;
            }
            InputAction::CyclePrev => {
                self.active_bucket = self.active_bucket.prev();
                let ctx = self.button_ctx();
                self.frame.buttons.clamp(&ctx);
                return OobAction::BucketChanged;
            }
            InputAction::NavUp => {
                return if self.active_bucket_state_mut().navigate_up() {
                    OobAction::Navigate
                } else { OobAction::None };
            }
            InputAction::NavDown => {
                return if self.active_bucket_state_mut().navigate_down() {
                    OobAction::Navigate
                } else { OobAction::None };
            }
            InputAction::PageUp => {
                return if self.active_bucket_state_mut().page_up() {
                    OobAction::Navigate
                } else { OobAction::None };
            }
            InputAction::PageDown => {
                return if self.active_bucket_state_mut().page_down() {
                    OobAction::Navigate
                } else { OobAction::None };
            }
            _ => {}
        }

        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            _ => OobAction::None,
        }
    }
}

// ============================================================================
// Dispatchable
// ============================================================================

impl super::dispatch::Dispatchable for OobResolutionState {
    type Action = OobAction;

    fn dispatch(
        &self,
        action: OobAction,
        resolver: &mm_meta::paths::PathResolver,
    ) -> super::dispatch::DispatchResult {
        use mm_meta::db_types::Zone;
        use mm_meta::mutations::indexing::{
            AcknowledgeMtimeOnlyMutation, ApplyDbTagsToDiskMutation,
            AssimilateDiskTagsToDbMutation,
        };
        use mm_meta::mutations::Mutation;
        use super::dispatch::DispatchResult;

        use mm_meta::mutations::indexing::{OobBucketAction, ResolveOobBucketMutation};

        // The bucket action for each button, and the bucket it acts on.
        let bucket_action = match action {
            OobAction::None | OobAction::Navigate | OobAction::BucketChanged => {
                return DispatchResult::Handled
            }
            OobAction::Cancel => return DispatchResult::Cancel,
            OobAction::ApplyDb => OobBucketAction::ApplyDbToDisk,
            OobAction::AssimilateDisk => OobBucketAction::AssimilateDiskToDb,
            OobAction::Acknowledge => OobBucketAction::AcknowledgeMtime,
        };
        let bucket = match action {
            OobAction::Acknowledge => ConflictBucket::MtimeOnly,
            _ => self.active_bucket,
        };
        let key = DecisionKey::OobResolution { bucket };

        // No selection means the whole bucket, which is staged as a single
        // mutation naming the bucket. Enumerating it here would put a mutation
        // per file into the decision — half a million of them during a bulk tag
        // rewrite — and the modal only ever holds a window of the bucket
        // anyway, so the enumeration would silently act on the window alone.
        if !self.has_explicit_selection() {
            let observed_count = self.bucket_counts[bucket.index()];
            if observed_count == 0 {
                return DispatchResult::Handled;
            }
            return DispatchResult::Stage {
                key,
                label: bucket_action.label().into(),
                mutations: vec![Mutation::ResolveOobBucket(ResolveOobBucketMutation {
                    bucket,
                    action: bucket_action,
                    observed_count,
                })],
            };
        }

        // An explicit selection is per-file by construction: it can only name
        // rows the operator can see, so it is bounded by the page.
        let page = self.active_bucket_state();
        let tracks: Vec<(i64, std::path::PathBuf)> = page
            .list
            .selected
            .iter()
            .filter_map(|&idx| page.files.get(idx))
            .map(|f| {
                (
                    f.inode,
                    resolver.resolve_for_zone(Zone::Corpus, std::path::Path::new(&f.path)),
                )
            })
            .collect();

        if tracks.is_empty() {
            return DispatchResult::Handled;
        }

        let mutations: Vec<Mutation> = match bucket_action {
            OobBucketAction::ApplyDbToDisk => tracks
                .into_iter()
                .map(|(inode, path)| {
                    Mutation::ApplyDbTagsToDisk(ApplyDbTagsToDiskMutation {
                        inode,
                        path,
                        zone: Zone::Corpus,
                    })
                })
                .collect(),
            OobBucketAction::AssimilateDiskToDb => tracks
                .into_iter()
                .map(|(inode, path)| {
                    Mutation::AssimilateDiskTagsToDb(AssimilateDiskTagsToDbMutation {
                        inode,
                        path,
                        zone: None,
                    })
                })
                .collect(),
            OobBucketAction::AcknowledgeMtime => {
                vec![Mutation::AcknowledgeMtimeOnly(
                    AcknowledgeMtimeOnlyMutation { tracks },
                )]
            }
        };

        DispatchResult::Stage {
            key,
            label: bucket_action.label().into(),
            mutations,
        }
    }

    fn cancel_message(&self) -> &'static str {
        "OOB resolution closed"
    }
}

impl ModalFrameCore for OobResolutionState {
    type Button = OobButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::HorizontalSplit { list_percent: 33, info_height: 4 }
    }

    fn list_title(&self) -> String { "Files".into() }

    fn frame_state(&self) -> &FrameState<OobButton> { &self.frame }
    fn frame_state_mut(&mut self) -> &mut FrameState<OobButton> { &mut self.frame }
    fn cursor(&self) -> usize { self.active_bucket_state().list.cursor }
    fn cursor_mut(&mut self) -> &mut usize { &mut self.active_bucket_state_mut().list.cursor }
    fn list_len(&self) -> usize { self.active_bucket_state().files.len() }
    fn button_ctx(&self) -> OobButtonCtx { OobResolutionState::button_ctx(self) }
    fn escape_action(&self) -> OobAction { OobAction::Cancel }
}
