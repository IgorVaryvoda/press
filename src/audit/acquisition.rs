//! Acquisition actions built from facts the audit already owns.
//!
//! These stay deliberately file-based: Press can prove dimensions, weight,
//! names, conversion output and Sirv delivery without decoding another image
//! or inventing a hosted-report service.

use super::*;
use std::collections::BTreeMap;

pub(super) const MARKETPLACE_EDGE: u32 = 1400;
pub(super) const MARKETPLACE_MAX_BYTES: u64 = 250 * 1024;
pub(super) const SHOW_ACQUISITION_EXTRAS: bool = false;

type SpinGroupKey = (String, String, String, String);
type NumberedFrames = Vec<(u32, usize)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SpinSet {
    pub name: String,
    pub indices: Vec<usize>,
    pub remote_folder: String,
    pub issue: Option<String>,
}

impl SpinSet {
    pub fn ready(&self) -> bool {
        self.issue.is_none()
    }
}

pub(super) fn marketplace_fails(entry: &Entry) -> bool {
    entry.width != MARKETPLACE_EDGE
        || entry.height != MARKETPLACE_EDGE
        || entry.bytes > MARKETPLACE_MAX_BYTES
        || entry.extension_lies()
}

fn numbered_stem(path: &Path) -> Option<(String, String, u32)> {
    let stem = path.file_stem()?.to_string_lossy();
    let mut run = None;
    let mut start = None;
    for (ix, ch) in stem.char_indices() {
        if ch.is_ascii_digit() {
            start.get_or_insert(ix);
        } else if let Some(from) = start.take() {
            run = Some((from, ix));
        }
    }
    if let Some(from) = start {
        run = Some((from, stem.len()));
    }
    let (from, to) = run?;
    let number = stem[from..to].parse().ok()?;
    Some((stem[..from].to_string(), stem[to..].to_string(), number))
}

fn path_key(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Find likely single-row spins. Two matching numbered files are enough to report
/// an incomplete set; publishing still requires Sirv's 8–1000 frame range plus
/// contiguous numbers and consistent pixels.
pub(super) fn detect_spins(root: &Path, entries: &[Entry]) -> Vec<SpinSet> {
    let mut groups: BTreeMap<SpinGroupKey, NumberedFrames> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some((prefix, suffix, number)) = numbered_stem(&entry.path) else {
            continue;
        };
        let relative = entry.path.strip_prefix(root).unwrap_or(&entry.path);
        let parent = relative.parent().map(path_key).unwrap_or_default();
        let extension = entry
            .path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        groups
            .entry((parent, prefix, suffix, extension))
            .or_default()
            .push((number, index));
    }

    groups
        .into_iter()
        .filter_map(|((parent, prefix, suffix, _), mut frames)| {
            if frames.len() < 2 {
                return None;
            }
            frames.sort_by_key(|(number, _)| *number);
            let name = format!("{prefix}{suffix}")
                .trim_matches(['-', '_', ' '])
                .to_string();
            let name = if name.is_empty() { "spin".into() } else { name };
            let remote_folder = ["press-spins", parent.as_str(), name.as_str()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("/");
            let indices = frames.iter().map(|(_, index)| *index).collect::<Vec<_>>();
            let mut issues = Vec::new();
            if frames.len() < 8 {
                issues.push(format!("needs 8 frames (found {})", frames.len()));
            } else if frames.len() > 1000 {
                issues.push(format!("exceeds 1000 frames (found {})", frames.len()));
            }
            if frames.iter().any(|(number, _)| *number > 1024) {
                issues.push("frame number exceeds 1024".into());
            }
            if let Some(pair) = frames
                .windows(2)
                .find(|pair| pair[1].0 != pair[0].0.saturating_add(1))
            {
                issues.push(if pair[1].0 == pair[0].0 {
                    format!("duplicate frame {}", pair[0].0)
                } else {
                    format!("missing frames after {}", pair[0].0)
                });
            }
            let first = &entries[indices[0]];
            if indices.iter().skip(1).any(|index| {
                let entry = &entries[*index];
                entry.width != first.width || entry.height != first.height
            }) {
                issues.push("inconsistent dimensions".into());
            }
            if indices
                .iter()
                .skip(1)
                .any(|index| entries[*index].format != first.format)
            {
                issues.push("inconsistent image formats".into());
            }
            Some(SpinSet {
                name,
                indices,
                remote_folder,
                issue: (!issues.is_empty()).then(|| issues.join(", ")),
            })
        })
        .collect()
}

pub(super) fn image_embed(url: &str) -> String {
    format!(
        "<img src=\"{url}?w=1280\" srcset=\"{url}?w=640 640w, {url}?w=1280 1280w, {url}?w=1920 1920w\" sizes=\"100vw\" loading=\"lazy\" alt=\"\">"
    )
}

pub(super) fn spin_embed(url: &str) -> String {
    format!(
        "<script src=\"https://scripts.sirv.com/sirvjs/v3/sirv.js\"></script>\n<div class=\"Sirv\" data-src=\"{url}\"></div>"
    )
}

pub(super) fn audit_report(
    root: &Path,
    entries: &[Entry],
    show_parent: bool,
    skipped_raw: usize,
    findings: (usize, usize, usize),
    conversion: (usize, (u64, u64)),
) -> String {
    let (heavy, mislabelled, marketplace) = findings;
    let (converted, converted_totals) = conversion;
    let folder = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "selected folder".into());
    let bytes = entries.iter().map(|entry| entry.bytes).sum::<u64>();
    let marketplace_passes = entries.len().saturating_sub(marketplace);
    let mut report = format!(
        "# Press image audit — {folder}\n\n- {} images · {}\n- {heavy} heavy · {mislabelled} mislabelled\n- Marketplace file preflight: {marketplace_passes} pass · {marketplace} need work (1400×1400, ≤250 KB, truthful extension; review the background visually)\n",
        entries.len(),
        format_bytes(bytes),
    );
    if skipped_raw > 0 {
        report.push_str(&format!(
            "- {skipped_raw} camera RAW sources present (not decoded)\n"
        ));
    }
    if converted > 0 {
        let (before, after) = converted_totals;
        report.push_str(&format!(
            "- Converted {converted}: {} → {} ({})\n",
            format_bytes(before),
            format_bytes(after),
            if after <= before {
                format!("{} saved", format_bytes(before - after))
            } else {
                format!("{} larger", format_bytes(after - before))
            }
        ));
    }
    let mut heaviest = entries.iter().collect::<Vec<_>>();
    heaviest.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    if !heaviest.is_empty() {
        report.push_str("\nHeaviest files:\n");
        for entry in heaviest.into_iter().take(5) {
            report.push_str(&format!(
                "- {} — {}\n",
                entry_label(root, show_parent, entry),
                format_bytes(entry.bytes)
            ));
        }
    }
    report.push_str(
        "\nDeliver faster images with Sirv: https://sirv.com/?utm_source=press&utm_medium=desktop&utm_campaign=audit-report\nCreate product visuals in Sirv Studio: https://www.sirv.studio/?utm_source=press&utm_medium=desktop&utm_campaign=audit-report\n",
    );
    report
}

fn remote_path(dir: &str, key: &str) -> String {
    format!(
        "{}/{}",
        dir.trim_end_matches('/'),
        key.trim_start_matches('/')
    )
}

fn result_key(root: &Path, entry: &Entry, output: &Path) -> Option<String> {
    let source = sirv::relative_key(root, &entry.path)?;
    let parent = source.rsplit_once('/').map(|(parent, _)| parent);
    let name = output.file_name()?.to_string_lossy();
    Some(match parent {
        Some(parent) => format!("optimized/{parent}/{name}"),
        None => format!("optimized/{name}"),
    })
}

impl Audit {
    pub(super) fn publish_waiting(&self) -> Option<String> {
        self.sirv_pairing
            .as_ref()
            .and_then(|pairing| match &pairing.cdn_host {
                CdnHost::Loading => Some("Finding the Sirv CDN host…".into()),
                CdnHost::Failed(message) => {
                    Some(format!("Could not find the Sirv CDN host: {message}"))
                }
                CdnHost::Ready(_) => None,
            })
    }

    pub(super) fn copy_audit_report(&mut self, cx: &mut Context<Self>) {
        if self.scan_blocks_delivery() {
            return;
        }
        let report = audit_report(
            &self.root,
            &self.entries,
            self.batch_folders.is_some(),
            self.skipped_raw,
            (self.heavy, self.mislabelled, self.marketplace),
            (self.results.len(), self.converted_totals),
        );
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(report));
        self.report_copied = true;
        cx.notify();
    }

    pub(super) fn publish_results(&mut self, cx: &mut Context<Self>) {
        if self.scan_blocks_delivery() {
            return;
        }
        if self.sirv_pairing.is_none() {
            self.open_sirv_browser(cx);
            return;
        }
        let Some(pairing) = &self.sirv_pairing else {
            return;
        };
        let CdnHost::Ready(host) = &pairing.cdn_host else {
            return;
        };
        let mut plan = Vec::new();
        let mut urls = Vec::new();
        for (index, output) in &self.result_paths {
            let Some(entry) = self.entries.get(*index) else {
                continue;
            };
            let Some(key) = result_key(&self.root, entry, output) else {
                continue;
            };
            let Ok(url) = sirv::public_url(host, &remote_path(&pairing.dir, &key)) else {
                continue;
            };
            plan.push((key, output.clone()));
            urls.push(url);
        }
        self.run_upload_plan(
            plan,
            SirvJobKind::Publish,
            UploadCompletion::Results(urls),
            cx,
        );
    }

    pub(super) fn publish_spins(&mut self, cx: &mut Context<Self>) {
        if self.scan_blocks_delivery() {
            return;
        }
        if self.sirv_pairing.is_none() {
            self.open_sirv_browser(cx);
            return;
        }
        let Some(pairing) = &self.sirv_pairing else {
            return;
        };
        let CdnHost::Ready(host) = &pairing.cdn_host else {
            return;
        };
        let mut plan = Vec::new();
        let mut urls = Vec::new();
        for spin in self.spins.iter().filter(|spin| spin.ready()) {
            for index in &spin.indices {
                let Some(entry) = self.entries.get(*index) else {
                    continue;
                };
                plan.push((
                    format!("{}/{}", spin.remote_folder, entry.name()),
                    entry.path.clone(),
                ));
            }
            let spin_name = spin.remote_folder.rsplit('/').next().unwrap_or(&spin.name);
            let filename = remote_path(
                &pairing.dir,
                &format!("{}/{}.spin", spin.remote_folder, spin_name),
            );
            if let Ok(url) = sirv::public_url(host, &filename) {
                urls.push(url);
            }
        }
        self.run_upload_plan(plan, SirvJobKind::Spin, UploadCompletion::Spins(urls), cx);
    }

    pub(super) fn copy_result_embeds(&mut self, cx: &mut Context<Self>) {
        let text = self
            .published_results
            .iter()
            .map(|url| image_embed(url))
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    pub(super) fn copy_spin_embeds(&mut self, cx: &mut Context<Self>) {
        let text = self
            .published_spins
            .iter()
            .map(|url| spin_embed(url))
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    pub(super) fn spin_notice(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.spins.is_empty() {
            return None;
        }
        let ready = self.spins.iter().filter(|spin| spin.ready()).count();
        let issues = self.spins.len() - ready;
        let frames = self
            .spins
            .iter()
            .filter(|spin| spin.ready())
            .map(|spin| spin.indices.len())
            .sum::<usize>();
        let message = if ready > 0 {
            format!(
                "{ready} {} ready · {frames} frames{}",
                if ready == 1 { "spin" } else { "spins" },
                if issues == 0 {
                    String::new()
                } else {
                    format!(" · {issues} need attention")
                }
            )
        } else {
            let spin = &self.spins[0];
            format!(
                "Spin preflight · {}: {}",
                spin.name,
                spin.issue.as_deref().unwrap_or("not ready")
            )
        };
        let alert = if ready > 0 {
            Alert::info("spin-preflight", message)
        } else {
            Alert::warning("spin-preflight", message)
        };
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .child(div().flex_1().min_w_0().child(alert))
                .children((ready > 0).then(|| {
                    if self.published_spins.is_empty() {
                        Button::new("publish-spins")
                            .small()
                            .outline()
                            .icon(IconName::ArrowUp)
                            .label(if self.sirv_pairing.is_none() {
                                "Connect & publish"
                            } else if ready == 1 {
                                "Publish spin"
                            } else {
                                "Publish spins"
                            })
                            .tooltip(self.publish_waiting().unwrap_or_else(|| {
                                "Upload complete numbered sets; Sirv creates the .spin files".into()
                            }))
                            .disabled(
                                self.scan_blocks_delivery()
                                    || self.sirv_busy()
                                    || self.publish_waiting().is_some(),
                            )
                            .on_click(cx.listener(|audit, _, _, cx| audit.publish_spins(cx)))
                    } else {
                        Button::new("copy-spin-embed")
                            .small()
                            .outline()
                            .icon(IconName::Copy)
                            .label("Copy spin embed")
                            .tooltip("Copy Sirv JS v3 embed code")
                            .on_click(cx.listener(|audit, _, _, cx| audit.copy_spin_embeds(cx)))
                    }
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageFormat;

    fn entry(name: &str, width: u32, height: u32) -> Entry {
        Entry {
            path: PathBuf::from(name),
            format: ImageFormat::Jpeg.into(),
            width,
            height,
            bytes: 100_000,
        }
    }

    #[test]
    fn spins_need_a_complete_consistent_sequence() {
        let mut entries = (1..=8)
            .map(|frame| entry(&format!("shoe-{frame:02}.jpg"), 1400, 1400))
            .collect::<Vec<_>>();
        let spins = detect_spins(Path::new(""), &entries);
        assert_eq!(spins.len(), 1);
        assert!(spins[0].ready());
        assert_eq!(spins[0].remote_folder, "press-spins/shoe");

        entries.remove(3);
        let spins = detect_spins(Path::new(""), &entries);
        assert!(spins[0].issue.as_deref().unwrap().contains("needs 8"));
        assert!(spins[0].issue.as_deref().unwrap().contains("missing"));
    }

    #[test]
    fn marketplace_and_report_only_claim_header_facts() {
        let ready = entry("/private/catalog/ready.jpg", 1400, 1400);
        assert!(!marketplace_fails(&ready));
        let mut large = entry("/private/catalog/large.jpg", 1400, 1400);
        large.bytes = MARKETPLACE_MAX_BYTES + 1;
        assert!(marketplace_fails(&large));

        let report = audit_report(
            Path::new("/private/catalog"),
            &[ready, large],
            false,
            3,
            (0, 0, 1),
            (0, (0, 0)),
        );
        assert!(report.contains("camera RAW sources present (not decoded)"));
        assert!(report.contains("review the background visually"));
        assert!(report.contains("Sirv Studio"));
        assert!(!report.contains("/private/catalog"));
    }

    #[test]
    fn embeds_use_responsive_images_and_sirv_js_v3() {
        let image = image_embed("https://demo.sirv.com/a.jpg");
        assert!(image.contains("srcset="));
        assert!(image.contains("loading=\"lazy\""));

        let spin = spin_embed("https://demo.sirv.com/a.spin");
        assert!(spin.contains("sirvjs/v3/sirv.js"));
        assert!(spin.contains("data-src=\"https://demo.sirv.com/a.spin\""));
    }
}
