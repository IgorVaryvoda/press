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
    pub(super) rows: Vec<ReviewRow>,
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
                    Ok((root, resolutions))
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
                    Ok((root, resolutions)) => {
                        review.root = Some(root);
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
        cx.notify();
        let id = id.to_string();
        Self::read_handoff_source(root, path, id, generation, request, None, cx);
    }

    /// Read a confirmed row's file again and compare it byte for byte. The
    /// same size and timestamp are not an answer: only the bytes are.
    pub(super) fn recheck_handoff_row(&mut self, id: &str, cx: &mut Context<Self>) {
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

/// A preserved unknown field as one readable line. Producers add advisory
/// metadata Press does not read, and dropping it from the card would hide what
/// the file actually says. It is shown as text and stays text: no value here
/// selects a format, a size, a folder or an action.
pub(super) fn advisory_line(key: &str, value: &serde_json::Value) -> String {
    const SHOWN: usize = 160;
    let rendered = match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let mut line: String = rendered
        .chars()
        .filter(|character| !character.is_control())
        .take(SHOWN)
        .collect();
    if rendered.chars().filter(|c| !c.is_control()).count() > SHOWN {
        line.push('…');
    }
    format!("{key}: {line}")
}

/// A path as text for a label only. The path itself stays a path.
pub(super) fn label(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_advisory_value_is_shown_as_text_and_bounded() {
        let long = serde_json::Value::String("x".repeat(400));
        let line = advisory_line("scope", &long);
        assert!(line.starts_with("scope: xxx"));
        assert!(line.ends_with('…'));
        assert_eq!(line.chars().count(), "scope: ".len() + 161);
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

    #[test]
    fn a_row_starts_unconfirmed() {
        let row = ReviewRow::new("r1".into());
        assert!(matches!(row.state, RowState::Unconfirmed));
        assert_eq!(row.state.word(), "unconfirmed");
        assert!(row.chosen.is_none());
        assert!(row.verdict.is_none());
    }
}
