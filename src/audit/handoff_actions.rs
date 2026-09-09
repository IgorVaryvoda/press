//! Reviewing an imported ImageGuide report in the window.
//!
//! The review is session state and nothing else. It never replaces the
//! dataset, writes a job, a recipe or a setting, creates an output, or starts
//! a conversion; the report's text is data the user reads, never instructions
//! Press follows. Every effect this file has is a filesystem read of a file
//! the user chose, inside a root the user chose.

use std::path::{Path, PathBuf};

use gpui_kit::{Context, Window};

use super::Audit;
use crate::handoff::{ConfirmedSource, PendingHandoff, Recheck, Verdict};

/// Attempts the review's keyboard makes to reach its first action. The review
/// only holds tab stops once a frame has laid it out, and a review the rail
/// stopped showing must not ask for frames forever.
const FOCUS_REVIEW_FRAMES: u8 = 4;

/// One imported report under review, with the local files each of its
/// resources might be.
pub(super) struct HandoffReview {
    /// The request generation this review belongs to. A background result
    /// carrying another one describes an import, a root or a dataset that has
    /// already been replaced, and is dropped rather than applied.
    pub(super) generation: u64,
    /// The report file, for provenance in the card. Never re-read.
    pub(super) file: PathBuf,
    pub(super) pending: PendingHandoff,
    /// The one root the user selected, as the kernel resolves it. `None`
    /// until they select one: a report alone points at no folder.
    pub(super) root: Option<PathBuf>,
    pub(super) scanning: bool,
    /// What the walk of that root could and could not read. Matching is only
    /// as complete as the walk behind it, so this travels with the rows.
    pub(super) scan: Option<ScanDiagnostics>,
    pub(super) rows: Vec<ReviewRow>,
}

/// What one walk of the chosen root found, and what it could not look at.
/// A resource nothing answered to is "nothing was found" — which means
/// "absent" only when the folder was read completely.
pub(super) struct ScanDiagnostics {
    /// Images the walk actually probed and offered to matching.
    pub(super) images: usize,
    /// Files under the root that would not decode, and folders the walk could
    /// not enter. Both make matching's answers a lower bound.
    pub(super) unreadable: Vec<PathBuf>,
    pub(super) unreadable_total: usize,
    pub(super) unentered: Vec<PathBuf>,
    pub(super) unentered_total: usize,
    /// Files this Press does not decode at all. They were seen and excluded,
    /// which is a different thing from a file it failed to read.
    pub(super) skipped_raw: usize,
    pub(super) skipped_heic: usize,
    pub(super) skipped_packages: usize,
}

impl ScanDiagnostics {
    fn of(scan: &crate::scan::Scan) -> Self {
        Self {
            images: scan.entries.len(),
            unreadable: scan.unreadable.iter().take(MAX_LINES).cloned().collect(),
            unreadable_total: scan.unreadable.len(),
            unentered: scan.walk_errors.iter().take(MAX_LINES).cloned().collect(),
            unentered_total: scan.walk_errors.len(),
            skipped_raw: scan.skipped_raw,
            skipped_heic: scan.skipped_heic,
            skipped_packages: scan.skipped_packages,
        }
    }

    /// Whether the walk left part of the folder unread. While this holds, a
    /// resource matching nothing has not been shown to be absent.
    pub(super) fn incomplete(&self) -> bool {
        self.unreadable_total > 0 || self.unentered_total > 0
    }

    /// Files that were seen and excluded because Press does not decode them.
    pub(super) fn unsupported(&self) -> usize {
        self.skipped_raw + self.skipped_heic + self.skipped_packages
    }
}

impl HandoffReview {
    pub(super) fn row(&self, id: &str) -> Option<&ReviewRow> {
        self.rows.iter().find(|row| row.id == id)
    }

    fn row_mut(&mut self, id: &str) -> Option<&mut ReviewRow> {
        self.rows.iter_mut().find(|row| row.id == id)
    }

    /// Start one request on a row and hand back the token it must come back
    /// with. Anything the row had in flight is superseded by it.
    fn start_row_request(&mut self, id: &str) -> Option<u64> {
        let row = self.row_mut(id)?;
        row.request = row.request.wrapping_add(1);
        Some(row.request)
    }

    pub(super) fn confirmed_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row.state, RowState::Confirmed(_)))
            .count()
    }
}

/// One reported resource in the review. The verdict is what matching found;
/// the state is what a person decided, and the two never merge.
pub(super) struct ReviewRow {
    pub(super) id: String,
    /// What automatic matching made of this resource under the current root,
    /// absent until a root has been scanned.
    pub(super) verdict: Option<Verdict>,
    /// Real files the row offers, at most `handoff::MAX_SHOWN_CHOICES`.
    pub(super) choices: Vec<PathBuf>,
    /// Further files the list does not show. They stay reachable through the
    /// row's own file picker, never through a rendered summary line.
    pub(super) hidden: usize,
    pub(super) notes: Vec<String>,
    /// The file this row would confirm. It comes from `choices` by index or
    /// from a file picker, never from parsing a path back out of a label.
    pub(super) chosen: Option<PathBuf>,
    pub(super) state: RowState,
    /// This row's own request counter, bumped by every choice, file dialog
    /// and read it starts. The review's generation covers the report, the
    /// root and the dataset; it cannot tell one row's second thought from its
    /// first. Without this, picking A, then B, then A again lets the first
    /// read land and confirm bytes nobody confirmed, and a file dialog opened
    /// before a confirmation can overwrite a newer choice when it returns.
    request: u64,
}

impl ReviewRow {
    fn new(id: String) -> Self {
        Self {
            id,
            verdict: None,
            choices: Vec::new(),
            hidden: 0,
            notes: Vec::new(),
            chosen: None,
            state: RowState::Unconfirmed,
            request: 0,
        }
    }
}

/// Where one row stands with its human. Every row starts unconfirmed,
/// including one whose supplied path hint named exactly one file: an exact
/// hint is evidence about a name, not somebody saying this is the image.
pub(super) enum RowState {
    Unconfirmed,
    /// A read is in flight. The row shows it rather than looking decided.
    Busy,
    Confirmed(ConfirmedSource),
    /// A recheck found other bytes under the same name. The confirmation is
    /// revoked, not amended: the user confirms the new file or picks another.
    Changed,
    /// The file could not be read as a source at all, with the reason kept.
    Refused(String),
}

impl RowState {
    pub(super) fn word(&self) -> &'static str {
        match self {
            RowState::Unconfirmed => "unconfirmed",
            RowState::Busy => "reading…",
            RowState::Confirmed(_) => "confirmed",
            RowState::Changed => "changed",
            RowState::Refused(_) => "refused",
        }
    }
}

impl Audit {
    /// Retire whatever was under review, and everything in flight for it. A
    /// new import, another root, a cancel and a replaced dataset all arrive
    /// here, so a late picker, scan or confirmation from any of them fails
    /// its ownership check instead of reviving a review nobody is looking at.
    pub(super) fn bump_handoff_generation(&mut self) {
        self.handoff_generation = self.handoff_generation.wrapping_add(1);
    }

    /// A background result is this review's only when the generation it was
    /// started under is still the current one and still owns the open review.
    fn owns_handoff_request(&self, generation: u64) -> bool {
        self.handoff_generation == generation
            && self
                .handoff_review
                .as_ref()
                .is_some_and(|review| review.generation == generation)
    }

    /// Pick an ImageGuide report and validate it into a pending review. The
    /// dialog and the read both run off the update path; the bytes are bounded
    /// before the parser sees them, and nothing on disk is touched besides the
    /// file the user chose. A product job is not involved: reviewing a report
    /// is not editing a job, so no job has to be chosen first.
    pub(super) fn import_handoff_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        self.bump_handoff_generation();
        self.handoff_review = None;
        cx.notify();
        let generation = self.handoff_generation;
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move {
                    rfd::FileDialog::new()
                        .add_filter("ImageGuide report", &["json"])
                        .pick_file()
                })
                .await;
            let Some(path) = picked else { return };
            let read = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        crate::job::read_bounded(
                            &path,
                            crate::handoff::MAX_FILE_BYTES,
                            "handoff report",
                        )
                        .and_then(|bytes| crate::handoff::parse_bytes(&bytes))
                    }
                })
                .await;
            let _ = this.update_in(cx, |audit, window, cx| {
                // The dialog outlives the click that opened it. A newer
                // import, a cancel or a new folder while it was open all moved
                // the generation on, and this file is no longer wanted.
                if audit.handoff_generation != generation {
                    return;
                }
                match read {
                    Ok(pending) => audit.open_handoff_review(path, pending, generation, window, cx),
                    Err(message) => {
                        audit.notify_error("handoff", "Couldn’t import the report", message, cx)
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn open_handoff_review(
        &mut self,
        file: PathBuf,
        pending: PendingHandoff,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = pending
            .handoff
            .resources
            .iter()
            .map(|resource| ReviewRow::new(resource.id.clone()))
            .collect();
        self.handoff_review = Some(HandoffReview {
            generation,
            file,
            pending,
            root: None,
            scanning: false,
            scan: None,
            rows,
        });
        // The card is the last thing in the rail's scrolling settings; bring
        // it to the top of them, so opening a review always shows one.
        let item = 3 + usize::from(!self.recipes_skipped.is_empty());
        self.rail_scroll.scroll_to_top_of_item(item);
        cx.on_next_frame(window, |audit, window, cx| {
            audit.focus_handoff_review(FOCUS_REVIEW_FRAMES, window, cx);
        });
        cx.notify();
    }

    /// Put the keyboard on the review's first action. The wrapper is a tab
    /// group that is not itself a stop, so anchoring there and stepping once
    /// lands on a real button, which keeps its focus ring and its native Enter
    /// and Space — gpui-component's `Button` renders its own keyed focus
    /// handle and discards one a caller supplies.
    ///
    /// The step only works off a frame whose tab stops already carry the card,
    /// so a window that has not painted it yet puts the anchor back and tries
    /// again, a bounded number of times.
    fn focus_handoff_review(&mut self, attempts: u8, window: &mut Window, cx: &mut Context<Self>) {
        if self.handoff_review.is_none() {
            return;
        }
        window.focus(&self.handoff_review_focus, cx);
        window.focus_next(cx);
        let landed = !self.handoff_review_focus.is_focused(window)
            && self.handoff_review_focus.contains_focused(window, cx);
        if landed {
            return;
        }
        window.focus(&self.handoff_review_focus, cx);
        if let Some(left) = attempts.checked_sub(1).filter(|left| *left > 0) {
            cx.on_next_frame(window, move |audit, window, cx| {
                audit.focus_handoff_review(left, window, cx);
            });
        }
    }

    /// Close the review and give the keyboard back to the list. Everything in
    /// flight for it is disowned first, so a scan or a read that finishes
    /// afterwards cannot put the card back up.
    pub(super) fn cancel_handoff_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bump_handoff_generation();
        self.handoff_review = None;
        self.clear_error("handoff", cx);
        Self::restore_audit_focus(window, cx);
    }

    /// Choose the one local folder this report is matched against. Selecting
    /// another root retires the previous one's scan and every confirmation
    /// made under it: bytes confirmed in one tree say nothing about another.
    pub(super) fn choose_handoff_root(&mut self, cx: &mut Context<Self>) {
        if self.converting || self.handoff_review.is_none() {
            return;
        }
        let generation = self.retarget_handoff_review();
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move { rfd::FileDialog::new().pick_folder() })
                .await;
            let Some(root) = picked else { return };
            let _ = this.update(cx, |audit, cx| {
                // The folder dialog outlives the click. A newer import, root
                // or cancel while it was open owns the review now.
                if audit.owns_handoff_request(generation) {
                    audit.set_handoff_root(root, cx);
                }
            });
        })
        .detach();
    }

    /// Point the open review at a root the caller already has. The chooser
    /// above is the same thing with a dialog in front of it.
    pub(super) fn set_handoff_root(&mut self, root: std::path::PathBuf, cx: &mut Context<Self>) {
        let generation = self.retarget_handoff_review();
        self.apply_handoff_root(root, generation, cx);
    }

    /// Retire the review's detached work and hand its successor generation
    /// back. The review itself stays: it is the same report, being pointed at
    /// another folder.
    pub(super) fn retarget_handoff_review(&mut self) -> u64 {
        self.bump_handoff_generation();
        let generation = self.handoff_generation;
        if let Some(review) = self.handoff_review.as_mut() {
            review.generation = generation;
            for row in &mut review.rows {
                // The reads this just disowned will never answer. A cancelled
                // folder dialog leaves the same rows on screen, so a row still
                // saying "reading…" would say it for the rest of the session.
                // It goes back to unconfirmed rather than to a confirmation
                // whose recheck never finished: an unfinished check is not a
                // pass.
                if matches!(row.state, RowState::Busy) {
                    row.request = row.request.wrapping_add(1);
                    row.state = RowState::Unconfirmed;
                }
            }
        }
        generation
    }

    /// Resolve and walk the chosen root, then match every resource under it.
    /// Every confirmation made under the previous root goes: bytes confirmed
    /// in one tree say nothing about a file in another.
    fn apply_handoff_root(
        &mut self,
        root: std::path::PathBuf,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(review) = self.handoff_review.as_mut() else {
            return;
        };
        review.scanning = true;
        // The report travels into the background with the walk. Matching
        // stats, resolves and reads directory entries for every resource, and
        // doing that between two UI updates would freeze the window on a large
        // folder; the update thread only ever receives finished resolutions.
        let handoff = review.pending.handoff.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            // Resolving and walking the root are filesystem work, so they
            // happen off the main thread. `canonicalize` is the kernel's own
            // answer about what folder this is: a lexical normalization would
            // let a typed `<root>/link/..` name a tree the root does not hold.
            let scanned = cx
                .background_executor()
                .spawn(async move {
                    let root = std::fs::canonicalize(&root)
                        .map_err(|error| format!("{} cannot be opened: {error}", root.display()))?;
                    if !root.is_dir() {
                        return Err(format!("{} is not a folder", root.display()));
                    }
                    let scan = crate::scan::scan(&root, &root.join(crate::scan::OUTPUT_DIR));
                    let resolutions =
                        crate::handoff::resolve_to_root(&handoff, &root, &scan.entries);
                    Ok((root, ScanDiagnostics::of(&scan), resolutions))
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if !audit.owns_handoff_request(generation) {
                    return;
                }
                let Some(review) = audit.handoff_review.as_mut() else {
                    return;
                };
                review.scanning = false;
                match scanned {
                    Ok((root, diagnostics, resolutions)) => {
                        review.root = Some(root);
                        review.scan = Some(diagnostics);
                        review.rows = resolutions
                            .into_iter()
                            .map(|resolution| ReviewRow {
                                // Matching proposes files; it never chooses
                                // one. Even a single exact path hint waits for
                                // the click that says this is the image, so
                                // nothing is preselected out of a resemblance.
                                chosen: None,
                                state: RowState::Unconfirmed,
                                id: resolution.id,
                                verdict: Some(resolution.verdict),
                                choices: resolution.paths,
                                hidden: resolution.hidden,
                                notes: resolution.notes,
                                request: 0,
                            })
                            .collect();
                        audit.clear_error("handoff", cx);
                    }
                    Err(message) => {
                        audit.notify_error("handoff", "Couldn’t read that folder", message, cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Take this row's next request token. The file dialog does this before
    /// it opens, so a dialog that returns after the row moved on is stale.
    pub(super) fn start_handoff_row_request(&mut self, id: &str) -> Option<u64> {
        self.handoff_review
            .as_mut()
            .and_then(|review| review.start_row_request(id))
    }

    /// Choose one of the files the row already offers, by position in its own
    /// list. A rendered label is never parsed back into a path, so the
    /// truncation count and a lossy display of a non-UTF-8 name cannot become
    /// a file somebody confirms.
    pub(super) fn select_handoff_choice(&mut self, id: &str, index: usize, cx: &mut Context<Self>) {
        let Some(review) = self.handoff_review.as_mut() else {
            return;
        };
        let Some(row) = review.row_mut(id) else {
            return;
        };
        let Some(path) = row.choices.get(index).cloned() else {
            return;
        };
        // Choosing another file withdraws whatever the previous one proved,
        // and supersedes any read or dialog this row still has out: a result
        // for the file they just moved off must not land as a confirmation.
        row.request = row.request.wrapping_add(1);
        row.chosen = Some(path);
        row.state = RowState::Unconfirmed;
        cx.notify();
    }

    /// Pick this row's source by hand. Hidden choices, a hashed build name and
    /// a file nothing matched all reach their source this way, and the pick is
    /// resolved inside the chosen root before it becomes the row's file.
    pub(super) fn choose_handoff_source(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let generation = self.handoff_generation;
        let Some(root) = self
            .handoff_review
            .as_ref()
            .and_then(|review| review.root.clone())
        else {
            return;
        };
        // These dialogs are not modal to this window, so the row stays live
        // while one is open. The token is taken now, and a dialog that returns
        // after the row was chosen, confirmed or asked again is stale.
        let Some(request) = self.start_handoff_row_request(id) else {
            return;
        };
        let id = id.to_string();

        cx.spawn(async move |this, cx| {
            let start = root.clone();
            let picked = cx
                .background_executor()
                .spawn(async move { rfd::FileDialog::new().set_directory(&start).pick_file() })
                .await;
            let Some(path) = picked else { return };
            let _ = this.update(cx, |audit, cx| {
                if audit.owns_handoff_request(generation) {
                    audit.apply_handoff_choice(&id, path, request, cx);
                }
            });
        })
        .detach();
    }

    /// Take a file somebody picked by hand as this row's source, once the
    /// kernel agrees the chosen root holds it. A pick outside the root is
    /// refused here, before any of its bytes are read.
    pub(super) fn apply_handoff_choice(
        &mut self,
        id: &str,
        path: std::path::PathBuf,
        request: u64,
        cx: &mut Context<Self>,
    ) {
        let generation = self.handoff_generation;
        let Some(review) = self.handoff_review.as_ref() else {
            return;
        };
        let Some(root) = review.root.clone() else {
            return;
        };
        if review.row(id).is_none_or(|row| row.request != request) {
            return;
        }
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let resolved = cx
                .background_executor()
                .spawn({
                    let root = root.clone();
                    async move { crate::handoff::resolve_choice(&root, &path) }
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if !audit.owns_handoff_request(generation) {
                    return;
                }
                let Some(review) = audit.handoff_review.as_mut() else {
                    return;
                };
                // The root can only be the one this pick was resolved against.
                if review.root.as_deref() != Some(root.as_path()) {
                    return;
                }
                let Some(row) = review.row_mut(&id) else {
                    return;
                };
                if row.request != request {
                    return;
                }
                match resolved {
                    Ok(path) => {
                        row.chosen = Some(path);
                        row.state = RowState::Unconfirmed;
                    }
                    Err(message) => {
                        row.chosen = None;
                        row.state = RowState::Refused(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Confirm this row's chosen file as the resource's local source: resolve
    /// it inside the chosen root, read its bytes under the conversion read
    /// bound, and keep their identity. One row at a time, by hand — there is
    /// no confirm-all, because a person confirming a list has confirmed
    /// nothing in particular.
    pub(super) fn confirm_handoff_row(&mut self, id: &str, cx: &mut Context<Self>) {
        // Guarded before anything moves: every source read is bounded by the
        // conversion limit, so a report's five hundred rows could otherwise
        // put five hundred of them in flight at once.
        if self.handoff_reading {
            return;
        }
        let generation = self.handoff_generation;
        let Some(review) = self.handoff_review.as_mut() else {
            return;
        };
        let Some(root) = review.root.clone() else {
            return;
        };
        let Some(row) = review.row_mut(id) else {
            return;
        };
        let Some(path) = row.chosen.clone() else {
            return;
        };
        row.state = RowState::Busy;
        row.request = row.request.wrapping_add(1);
        let request = row.request;
        self.handoff_reading = true;
        cx.notify();
        let id = id.to_string();
        Self::read_handoff_source(root, path, id, generation, request, None, cx);
    }

    /// Read a confirmed row's file again and compare it byte for byte. The
    /// same size and timestamp are not an answer: only the bytes are.
    pub(super) fn recheck_handoff_row(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.handoff_reading {
            return;
        }
        let generation = self.handoff_generation;
        let Some(review) = self.handoff_review.as_mut() else {
            return;
        };
        let Some(root) = review.root.clone() else {
            return;
        };
        let Some(row) = review.row_mut(id) else {
            return;
        };
        let RowState::Confirmed(confirmed) = &row.state else {
            return;
        };
        let expected = confirmed.clone();
        let path = expected.source.path.clone();
        row.state = RowState::Busy;
        row.request = row.request.wrapping_add(1);
        let request = row.request;
        self.handoff_reading = true;
        cx.notify();
        let id = id.to_string();
        Self::read_handoff_source(root, path, id, generation, request, Some(expected), cx);
    }

    /// The one place a review reads image bytes. The review's generation, the
    /// root, the row and the row's own request token all travel with it and
    /// are all checked again on the way back, so a read that finishes after
    /// the review closed, the root changed, or the row was pointed at another
    /// file lands nowhere.
    #[allow(clippy::too_many_arguments)]
    fn read_handoff_source(
        root: PathBuf,
        path: PathBuf,
        id: String,
        generation: u64,
        request: u64,
        expected: Option<ConfirmedSource>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn({
                    let root = root.clone();
                    let path = path.clone();
                    let expected = expected.clone();
                    async move {
                        match expected {
                            None => match crate::handoff::confirm_source(&root, &path) {
                                Ok(source) => SourceOutcome::Confirmed(source),
                                Err(message) => SourceOutcome::Refused(message),
                            },
                            Some(expected) => {
                                match crate::handoff::recheck_source(&root, &expected) {
                                    Recheck::Same => SourceOutcome::Unchanged,
                                    Recheck::Changed => SourceOutcome::Changed,
                                    Recheck::Refused(message) => SourceOutcome::Refused(message),
                                }
                            }
                        }
                    }
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                // The slot is released first and unconditionally. This read
                // finished whether or not anybody still wants its answer, and
                // every check below can decline it; releasing after one of
                // them would leave the review unable to read anything again.
                audit.handoff_reading = false;
                cx.notify();
                if !audit.owns_handoff_request(generation) {
                    return;
                }
                let Some(review) = audit.handoff_review.as_mut() else {
                    return;
                };
                if review.root.as_deref() != Some(root.as_path()) {
                    return;
                }
                let Some(row) = review.row_mut(&id) else {
                    return;
                };
                // The row's own token, not the path: a path hint that named
                // the file through an in-root link resolves to the file's real
                // name, and comparing the two would drop the answer and leave
                // the row reading forever.
                if row.request != request {
                    return;
                }
                row.state = match outcome {
                    SourceOutcome::Confirmed(source) => {
                        // The row now names the file whose bytes were read,
                        // so a later recheck asks about that same file.
                        row.chosen = Some(source.source.path.clone());
                        RowState::Confirmed(source)
                    }
                    // The bytes are the ones that were confirmed, so the
                    // confirmation that named them stands unchanged.
                    SourceOutcome::Unchanged => match expected {
                        Some(expected) => RowState::Confirmed(expected),
                        None => RowState::Unconfirmed,
                    },
                    SourceOutcome::Changed => RowState::Changed,
                    SourceOutcome::Refused(message) => RowState::Refused(message),
                };
                cx.notify();
            });
        })
        .detach();
    }
}

/// What one read of a chosen file established.
enum SourceOutcome {
    Confirmed(ConfirmedSource),
    Unchanged,
    Changed,
    Refused(String),
}

/// How many entries of one list the card draws before it says how many it is
/// not showing. A valid bounded report can still carry tens of thousands of
/// preserved unknown fields, and one warning for each of them; the card is a
/// review, not a rendering of the file.
pub(super) const MAX_LINES: usize = 8;

/// How long one drawn line may be. Every string in a report is bounded at
/// `handoff::MAX_STRING_CHARS`, which is four thousand characters — a size
/// meant for a parser, not for a line of a card.
const MAX_LINE_CHARS: usize = 160;

/// How much of a field name is drawn. A name is an identifier, not prose.
const MAX_KEY_CHARS: usize = 48;

/// One line of at most `limit` characters, with control characters removed so
/// a value cannot break out of its line and read like something Press wrote.
/// A line that was cut ends in an ellipsis, so a preview never passes for the
/// whole value.
fn clip(text: &str, limit: usize) -> String {
    let mut readable = text.chars().filter(|character| !character.is_control());
    let mut line: String = readable.by_ref().take(limit).collect();
    if readable.next().is_some() {
        line.push('…');
    }
    line
}

/// A preserved unknown value as text. The point of showing it is that the
/// reader can see what the producer actually said — the export's scope and its
/// recommended formats live in exactly these fields — so a compound value is
/// previewed as JSON rather than summarized into its shape, and the clip marks
/// where the preview stops. A whole report is bounded at one mebibyte and only
/// `MAX_LINES` fields are drawn per section, which bounds this.
fn advisory_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => clip(text, MAX_LINE_CHARS),
        other => clip(
            &serde_json::to_string(other).unwrap_or_else(|_| "unreadable value".into()),
            MAX_LINE_CHARS,
        ),
    }
}

/// A preserved unknown field as one readable line. Producers add advisory
/// metadata Press does not read, and dropping it from the card would hide what
/// the file actually says. It is shown as text and stays text: no value here
/// selects a format, a size, a folder or an action.
pub(super) fn advisory_line(key: &str, value: &serde_json::Value) -> String {
    format!("{}: {}", clip(key, MAX_KEY_CHARS), advisory_value(value))
}

/// One drawn line of ordinary report text, clipped to fit the card.
pub(super) fn text_line(text: &str) -> String {
    clip(text, MAX_LINE_CHARS)
}

/// At most `MAX_LINES` of `items`, followed by one line naming how many were
/// left out. `total` is counted, never collected: the caller passes an
/// iterator, so fifty thousand fields cost eight strings to draw.
///
/// `tail` is the caller's own phrase because where the rest went differs.
/// Report content is all still in the pending report; a walk's diagnostics
/// are not in any report and are simply out of view.
pub(super) fn bounded_lines(
    items: impl Iterator<Item = String>,
    total: usize,
    tail: &str,
) -> Vec<String> {
    let mut lines: Vec<String> = items.take(MAX_LINES).collect();
    if let Some(hidden) = total.checked_sub(MAX_LINES).filter(|hidden| *hidden > 0) {
        lines.push(format!("…and {hidden} more {tail}"));
    }
    lines
}

/// What the walk of the chosen root could and could not read, as lines. A
/// folder that was not fully read cannot support the word "absent", so it says
/// so here and every unmatched row repeats it.
pub(super) fn scan_lines(scan: Option<&ScanDiagnostics>) -> Vec<String> {
    let Some(scan) = scan else {
        return Vec::new();
    };
    let mut lines = vec![format!("Folder: {} images searched", scan.images)];
    if scan.unsupported() > 0 {
        // Seen and excluded, which is not the same as failed to read: these
        // formats have no decoder here, and saying otherwise would invent
        // support the app does not have.
        let mut parts = Vec::new();
        if scan.skipped_raw > 0 {
            parts.push(format!("{} camera raw", scan.skipped_raw));
        }
        if scan.skipped_heic > 0 {
            parts.push(format!("{} HEIC", scan.skipped_heic));
        }
        if scan.skipped_packages > 0 {
            parts.push(format!("{} image package", scan.skipped_packages));
        }
        lines.push(format!(
            "Not searched, unsupported here: {}",
            parts.join(", ")
        ));
    }
    if !scan.incomplete() {
        return lines;
    }
    lines.push("This folder was not read completely, so a row below matching nothing has not been shown to be absent.".into());
    if scan.unreadable_total > 0 {
        lines.push(format!("Would not decode ({}):", scan.unreadable_total));
        lines.extend(bounded_lines(
            scan.unreadable
                .iter()
                .map(|path| format!("  {}", named(path))),
            scan.unreadable_total,
            "files, not listed here",
        ));
    }
    if scan.unentered_total > 0 {
        lines.push(format!("Could not enter ({}):", scan.unentered_total));
        lines.extend(bounded_lines(
            scan.unentered
                .iter()
                .map(|path| format!("  {}", named(path))),
            scan.unentered_total,
            "folders, not listed here",
        ));
    }
    lines
}

/// Who produced the report, when, what it removed, what the folder search
/// found, and what this Press kept without reading. Every list is bounded and
/// says how much it left out; the pending report keeps all of it.
pub(super) fn provenance_lines(review: &HandoffReview) -> Vec<String> {
    let handoff = &review.pending.handoff;
    let mut lines = vec![
        text_line(&format!(
            "Producer: {} {}",
            handoff.producer, handoff.producer_revision
        )),
        text_line(&format!("Task: {}", handoff.task)),
        text_line(&format!("Observed: {}", handoff.observed)),
        text_line(&format!("File: {}", label(&review.file))),
    ];
    if handoff.redactions.is_empty() {
        lines.push("Redactions: none declared".into());
    } else {
        lines.push(format!("Redactions ({}):", handoff.redactions.len()));
        lines.extend(bounded_lines(
            handoff
                .redactions
                .iter()
                .map(|redaction| format!("  {}", text_line(redaction))),
            handoff.redactions.len(),
            "redactions, kept in the report",
        ));
    }
    lines.extend(scan_lines(review.scan.as_ref()));
    // Scope, limits and every other field this schema does not define are
    // shown as the file holds them. Press has not read them, and saying so is
    // the only honest label for a value it cannot interpret. A valid report
    // can carry tens of thousands of them, with one warning each, so the card
    // draws a bounded head and names the remainder.
    if !handoff.unknown.is_empty() {
        lines.push(format!(
            "Advisory fields kept unread ({}):",
            handoff.unknown.len()
        ));
        lines.extend(bounded_lines(
            handoff
                .unknown
                .iter()
                .map(|(key, value)| format!("  {}", advisory_line(key, value))),
            handoff.unknown.len(),
            "advisory fields, kept in the report",
        ));
    }
    if !review.pending.warnings.is_empty() {
        lines.push(format!("Warnings ({}):", review.pending.warnings.len()));
        lines.extend(bounded_lines(
            review
                .pending
                .warnings
                .iter()
                .map(|warning| format!("  {}", text_line(warning))),
            review.pending.warnings.len(),
            "warnings, kept in the report",
        ));
    }
    lines
}

/// What one resource says about itself, what matching made of it, and what
/// its row has decided. Same bounds as the provenance: a resource may carry
/// sixty-four findings of four thousand characters each.
pub(super) fn resource_lines(review: &HandoffReview, row: &ReviewRow) -> Vec<String> {
    let Some(resource) = review
        .pending
        .handoff
        .resources
        .iter()
        .find(|resource| resource.id == row.id)
    else {
        return Vec::new();
    };
    let scan = review.scan.as_ref();
    let root = review.root.as_deref();
    let mut lines = Vec::new();
    lines.push(match row.verdict {
        None => "Match: choose a source folder first".to_string(),
        Some(verdict) => {
            let mut line = format!(
                "Match: {} (automatic; not a confirmation)",
                crate::handoff::verdict_word(verdict)
            );
            // An incomplete walk cannot support "nothing is here".
            if scan.is_some_and(ScanDiagnostics::incomplete)
                && matches!(verdict, Verdict::Unmatched | Verdict::OutOfScope)
            {
                line.push_str(" — from a folder that was not read completely");
            }
            line
        }
    });
    lines.push(match row.chosen.as_deref() {
        // Under the root, so a duplicate basename reads as the file it is.
        Some(path) => text_line(&format!("Chosen: {}", under_root(root, path))),
        None => "Chosen: no file".to_string(),
    });
    if row.hidden > 0 {
        lines.push(format!(
            "{} more files match; reach them with Choose file…",
            row.hidden
        ));
    }
    lines.push(match (resource.width, resource.height) {
        (Some(width), Some(height)) => format!("Observed: {width}×{height}"),
        // An absent dimension is unknown, not zero and not a default.
        _ => "Observed: dimensions unknown".to_string(),
    });
    lines.push(match resource.bytes {
        Some(bytes) if resource.bytes_measured => {
            format!("Bytes: {} measured", crate::scan::format_bytes(bytes))
        }
        Some(bytes) => format!("Bytes: {} estimated", crate::scan::format_bytes(bytes)),
        None => "Bytes: unknown".to_string(),
    });
    if !resource.findings.is_empty() {
        lines.push(format!("Findings ({}):", resource.findings.len()));
        lines.extend(bounded_lines(
            resource
                .findings
                .iter()
                .map(|finding| format!("  {}", text_line(finding))),
            resource.findings.len(),
            "findings, kept in the report",
        ));
    }
    if !resource.formats.is_empty() {
        lines.push(format!("Requested formats ({}):", resource.formats.len()));
        lines.extend(bounded_lines(
            resource
                .formats
                .iter()
                .map(|format| format!("  {}", text_line(format))),
            resource.formats.len(),
            "formats, kept in the report",
        ));
    }
    lines.push(match resource.max_edge {
        Some(edge) => format!("Requested max edge: {edge}px"),
        None => "Requested max edge: none".to_string(),
    });
    // Advisory producer metadata, including any observed or recommended
    // format. It is shown, never applied: this review changes no recipe.
    if !resource.unknown.is_empty() {
        lines.push(format!(
            "Advisory fields kept unread ({}):",
            resource.unknown.len()
        ));
        lines.extend(bounded_lines(
            resource
                .unknown
                .iter()
                .map(|(key, value)| format!("  {}", advisory_line(key, value))),
            resource.unknown.len(),
            "advisory fields, kept in the report",
        ));
    }
    if !row.notes.is_empty() {
        lines.extend(bounded_lines(
            row.notes
                .iter()
                .map(|note| text_line(&format!("Note: {note}"))),
            row.notes.len(),
            "notes, kept in the report",
        ));
    }
    if let RowState::Refused(reason) = &row.state {
        lines.push(text_line(&format!("Refused: {reason}")));
    }
    if matches!(row.state, RowState::Changed) {
        lines.push("The file's bytes changed since it was confirmed.".into());
    }
    lines
}

/// A path's own name for a diagnostic line, falling back to the whole path
/// when it has none.
fn named(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| label(path));
    text_line(&name)
}

/// A path as it reads under the chosen root: `a/icon.webp` rather than just
/// `icon.webp`, so two files that share a basename can be told apart by the
/// person choosing between them. Display only — the row keeps the real path
/// and confirms that. A path that is somehow not under the root shows whole,
/// rather than silently shortening to something that is not where it is.
pub(super) fn under_root(root: Option<&Path>, path: &Path) -> String {
    let shown = root
        .and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path);
    label(shown)
}

/// A path as text for a label only. The path itself stays a path.
pub(super) fn label(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_advisory_key_and_value_are_both_bounded() {
        let line = advisory_line(&"k".repeat(200), &serde_json::json!("x".repeat(4096)));
        assert_eq!(
            line.chars().count(),
            MAX_KEY_CHARS + 1 + ": ".len() + MAX_LINE_CHARS + 1,
            "both halves are clipped, each with its own ellipsis: {line}"
        );
    }

    #[test]
    fn advisory_control_characters_never_reach_a_label() {
        let value = serde_json::Value::String("full\u{7}-audit\nrm -rf".into());
        assert_eq!(
            advisory_line("kind", &value),
            "kind: full-auditrm -rf",
            "an advisory value is one line of text, never a second line that reads like a command"
        );
    }

    /// The count comes from the caller, so nothing has to be collected to
    /// learn how much was left out.
    #[test]
    fn a_long_list_draws_a_bounded_head_and_says_what_it_left_out() {
        let lines = bounded_lines(
            (0..50_000).map(|index| format!("f{index}")),
            50_000,
            "fields",
        );
        assert_eq!(lines.len(), MAX_LINES + 1);
        assert_eq!(lines[0], "f0");
        assert_eq!(
            lines[MAX_LINES],
            format!("…and {} more fields", 50_000 - MAX_LINES)
        );
        let short = bounded_lines(["one".to_string()].into_iter(), 1, "fields");
        assert_eq!(
            short,
            vec!["one".to_string()],
            "a short list says nothing extra"
        );
    }

    /// The shape of the accepted adversarial export: a valid, in-bounds report
    /// whose tens of thousands of preserved unknown fields each produce a
    /// warning. Built here rather than checking in the 940 KB file, so the
    /// fixture lives with the behaviour it describes.
    fn many_advisory_fields(extra: usize) -> PendingHandoff {
        let mut envelope = serde_json::json!({
            "schema": crate::handoff::SCHEMA_VERSION,
            "producer": "imageguide-extension",
            "producer_revision": "press-export-1",
            "task": "full-audit",
            "observed": "2026-09-09T06:47:22.361Z",
            "redactions": ["page-url-and-title-omitted"],
            "scope": {"kind": "full-audit", "filter": "all", "search_applied": false},
            "resources": [{
                "id": "r1",
                "urls": [],
                "path_hints": ["catalog-item.png"],
                "width": 1800,
                "height": 1200,
                "bytes": 88945,
                "bytes_measured": true,
                "findings": ["oversized"],
                "max_edge": null,
                "formats": [],
                "format_recommendations": ["webp", "avif"]
            }]
        });
        let fields = envelope.as_object_mut().expect("the envelope is an object");
        for index in 0..extra {
            fields.insert(format!("extra_{index}"), serde_json::json!(index));
        }
        crate::handoff::parse_bytes(&serde_json::to_vec(&envelope).expect("the report serializes"))
            .expect("a report of preserved unknown fields is still a valid report")
    }

    fn review_of(pending: PendingHandoff) -> HandoffReview {
        let rows = pending
            .handoff
            .resources
            .iter()
            .map(|resource| ReviewRow::new(resource.id.clone()))
            .collect();
        HandoffReview {
            generation: 0,
            file: PathBuf::from("many-advisory-fields.json"),
            pending,
            root: None,
            scanning: false,
            scan: None,
            rows,
        }
    }

    /// The reason advisory fields are drawn at all is that the reader can see
    /// what the producer said. A nested scope and a recommendation list must
    /// therefore show their contents, marked where the preview stops — not a
    /// count of how many things were inside them.
    #[test]
    fn a_nested_advisory_value_shows_its_content_in_the_preview() {
        let review = review_of(many_advisory_fields(0));
        let scope = provenance_lines(&review)
            .into_iter()
            .find(|line| line.trim_start().starts_with("scope: "))
            .expect("the export's scope is drawn");
        assert!(
            scope.contains("full-audit") && scope.contains("\"filter\":\"all\""),
            "the scope's own restriction is legible: {scope}"
        );
        let recommendation = resource_lines(&review, &review.rows[0])
            .into_iter()
            .find(|line| line.trim_start().starts_with("format_recommendations: "))
            .expect("the resource's recommendation is drawn");
        assert!(
            recommendation.contains("webp") && recommendation.contains("avif"),
            "an advisory format list reads as formats: {recommendation}"
        );
    }

    /// A long value is previewed, not summarized away, and the ellipsis says
    /// the preview stopped.
    #[test]
    fn a_long_advisory_value_is_previewed_and_marked() {
        let long = serde_json::json!({"kind": "x".repeat(4000)});
        let line = advisory_line("scope", &long);
        assert!(line.starts_with("scope: {\"kind\":\"xxx"), "{line}");
        assert!(
            line.ends_with('…'),
            "the preview says where it stops: {line}"
        );
        assert_eq!(line.chars().count(), "scope: ".len() + MAX_LINE_CHARS + 1);
    }

    /// Tens of thousands of advisory fields, and one warning each, must not
    /// become tens of thousands of lines. The report keeps every one of them;
    /// the card draws a bounded head and says how much it is not drawing.
    #[test]
    fn a_report_of_many_advisory_fields_draws_a_bounded_card() {
        let pending = many_advisory_fields(50_000);
        let fields = pending.handoff.unknown.len();
        let warnings = pending.warnings.len();
        assert_eq!(
            fields, 50_001,
            "scope plus every injected field is preserved"
        );
        // One warning for each of those, plus one for the resource's own
        // preserved `format_recommendations`.
        assert_eq!(
            warnings,
            fields + 1,
            "and each one is still named as unread"
        );

        let review = review_of(pending);
        let lines = provenance_lines(&review);
        assert!(
            lines.len() <= 4 + 2 + 2 * (MAX_LINES + 2),
            "the card stays a card: {} lines",
            lines.len()
        );
        assert!(
            lines.contains(&format!("Advisory fields kept unread ({fields}):")),
            "the full count is disclosed: {lines:#?}"
        );
        assert!(
            lines.contains(&format!(
                "…and {} more advisory fields, kept in the report",
                fields - MAX_LINES
            )),
            "and so is how much is not drawn: {lines:#?}"
        );
        assert!(
            lines.contains(&format!(
                "…and {} more warnings, kept in the report",
                warnings - MAX_LINES
            )),
            "warnings are bounded the same way: {lines:#?}"
        );
        assert!(
            review.pending.handoff.unknown.len() == fields
                && review.pending.warnings.len() == warnings,
            "and drawing the card changed neither"
        );
    }

    /// A resource's own lists are bounded on the same terms: the schema allows
    /// sixty-four findings of four thousand characters each.
    #[test]
    fn a_resources_long_lists_are_bounded_with_their_counts() {
        let mut pending = many_advisory_fields(0);
        pending.handoff.resources[0].findings = (0..crate::handoff::MAX_LIST_ENTRIES)
            .map(|index| format!("{index}-{}", "f".repeat(4096)))
            .collect();
        let review = review_of(pending);
        let lines = resource_lines(&review, &review.rows[0]);
        assert!(
            lines.contains(&format!("Findings ({}):", crate::handoff::MAX_LIST_ENTRIES)),
            "{lines:#?}"
        );
        assert!(
            lines.contains(&format!(
                "…and {} more findings, kept in the report",
                crate::handoff::MAX_LIST_ENTRIES - MAX_LINES
            )),
            "{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .all(|line| line.chars().count() <= MAX_LINE_CHARS + 4),
            "no drawn line runs away with a four-thousand-character finding"
        );
    }

    fn walk(unreadable: usize, raw: usize) -> ScanDiagnostics {
        ScanDiagnostics {
            images: 3,
            unreadable: (0..unreadable.min(MAX_LINES))
                .map(|index| PathBuf::from(format!("/root/sub/broken-{index}.png")))
                .collect(),
            unreadable_total: unreadable,
            unentered: Vec::new(),
            unentered_total: 0,
            skipped_raw: raw,
            skipped_heic: 0,
            skipped_packages: 0,
        }
    }

    /// An incomplete walk cannot support the word "absent", and every row that
    /// found nothing has to say so. Its own diagnostics are not in the report,
    /// so they do not claim to be kept there.
    #[test]
    fn an_incomplete_walk_qualifies_every_row_that_found_nothing() {
        let mut review = review_of(many_advisory_fields(0));
        review.rows[0].verdict = Some(Verdict::Unmatched);
        review.scan = Some(walk(20, 2));
        assert!(
            review
                .scan
                .as_ref()
                .is_some_and(ScanDiagnostics::incomplete)
        );

        let lines = provenance_lines(&review);
        assert!(
            lines.contains(&"Folder: 3 images searched".to_string()),
            "{lines:#?}"
        );
        assert!(
            lines.contains(&"Not searched, unsupported here: 2 camera raw".to_string()),
            "an excluded format is named as unsupported, not as missing: {lines:#?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("not read completely")),
            "{lines:#?}"
        );
        assert!(
            lines.contains(&"Would not decode (20):".to_string()),
            "{lines:#?}"
        );
        assert!(
            lines.contains(&"  broken-0.png".to_string()),
            "a small named set survives: {lines:#?}"
        );
        assert!(
            lines.contains(&format!(
                "…and {} more files, not listed here",
                20 - MAX_LINES
            )),
            "the rest are out of view, not in a report that never held them: {lines:#?}"
        );

        let row = resource_lines(&review, &review.rows[0]);
        assert_eq!(
            row[0],
            "Match: unmatched (automatic; not a confirmation) — from a folder that was not read completely"
        );
    }

    /// A folder that was read completely says nothing about incompleteness.
    #[test]
    fn a_complete_walk_leaves_unmatched_unqualified() {
        let mut review = review_of(many_advisory_fields(0));
        review.rows[0].verdict = Some(Verdict::Unmatched);
        review.scan = Some(walk(0, 0));
        assert!(
            !review
                .scan
                .as_ref()
                .expect("a walk is recorded")
                .incomplete()
        );
        let lines = provenance_lines(&review);
        assert!(
            !lines
                .iter()
                .any(|line| line.contains("not read completely")),
            "{lines:#?}"
        );
        assert_eq!(
            resource_lines(&review, &review.rows[0])[0],
            "Match: unmatched (automatic; not a confirmation)"
        );
    }

    /// Two candidates called `icon.webp` must not both be drawn as
    /// `icon.webp`. Their labels differ by where they are, while the row keeps
    /// the paths themselves.
    #[test]
    fn duplicate_basenames_are_told_apart_by_where_they_are() {
        let root = PathBuf::from("/pictures/site");
        let first = root.join("a").join("icon.webp");
        let second = root.join("b").join("icon.webp");
        let one = under_root(Some(&root), &first);
        let other = under_root(Some(&root), &second);
        assert_ne!(one, other, "the two labels are not the same label");
        assert_eq!(
            one,
            PathBuf::from("a").join("icon.webp").display().to_string()
        );
        assert_eq!(
            under_root(Some(&root), Path::new("/elsewhere/icon.webp")),
            "/elsewhere/icon.webp",
            "a path that is not under the root shows whole rather than pretending"
        );

        let mut review = review_of(many_advisory_fields(0));
        review.root = Some(root);
        review.rows[0].choices = vec![first, second.clone()];
        review.rows[0].chosen = Some(second.clone());
        assert_eq!(
            review.rows[0].chosen.as_deref(),
            Some(second.as_path()),
            "the chosen file is still the path, not its label"
        );
        assert!(
            resource_lines(&review, &review.rows[0]).contains(&format!(
                "Chosen: {}",
                PathBuf::from("b").join("icon.webp").display()
            )),
            "and the line names which of the two it is"
        );
    }

    #[test]
    fn a_row_starts_unconfirmed() {
        let row = ReviewRow::new("r1".into());
        assert!(matches!(row.state, RowState::Unconfirmed));
        assert_eq!(row.state.word(), "unconfirmed");
        assert!(row.chosen.is_none());
        assert!(row.verdict.is_none());
    }
}
