//! Publish converted images to Sirv and copy responsive embed markup.

use super::*;

pub(super) fn image_embed(url: &str, alt: &str) -> String {
    let alt = html_attribute(alt);
    format!(
        "<img src=\"{url}?w=1280\" srcset=\"{url}?w=640 640w, {url}?w=1280 1280w, {url}?w=1920 1920w\" sizes=\"100vw\" loading=\"lazy\" alt=\"{alt}\">"
    )
}

/// Alt text is model output: it must not be able to close the attribute.
fn html_attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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

/// What publishing the originals does with each converted row. Sirv encodes
/// on delivery, so a master published as-is avoids a second lossy encode.
/// A remote copy of the same size is linked, not uploaded again; a copy of a
/// different size is someone's other master and is never overwritten here.
#[derive(Debug, PartialEq)]
enum OriginalPlan {
    Upload(String),
    Link(String),
    Differs(String),
}

fn original_plan(
    root: &Path,
    entry: &Entry,
    files: &HashMap<String, sirv::Node>,
    deep: bool,
) -> Option<OriginalPlan> {
    let key = sirv::paired_key(root, &entry.path, deep)?;
    Some(match sirv::classify(entry.bytes, files.get(&key)) {
        sirv::SyncState::OnlyLocal => OriginalPlan::Upload(key),
        sirv::SyncState::SameSize => OriginalPlan::Link(key),
        sirv::SyncState::DifferentSize => OriginalPlan::Differs(key),
    })
}

impl Audit {
    /// Originals unless replace mode moved them into the backup folder: there
    /// the file at the source path is the converted one.
    pub(super) fn publish_uploads_originals(&self) -> bool {
        self.publish_originals && !matches!(self.conversion_destination, Some((Output::Replace, _)))
    }

    pub(super) fn publish_waiting(&self) -> Option<String> {
        let pairing = self.sirv_pairing.as_ref()?;
        match &pairing.cdn_host {
            CdnHost::Loading => return Some("Finding the Sirv CDN host…".into()),
            CdnHost::Failed(message) => {
                return Some(format!("Could not find the Sirv CDN host: {message}"));
            }
            CdnHost::Ready(_) => {}
        }
        (self.publish_uploads_originals() && !matches!(pairing.files, Listing::Ready(_)))
            .then(|| "Listing the paired Sirv folder…".into())
    }

    pub(super) fn publish_results(&mut self, cx: &mut Context<Self>) {
        if self.scan_blocks_delivery() {
            return;
        }
        if self.sirv_pairing.is_none() {
            self.open_sirv_browser(cx);
            return;
        }
        let originals = self.publish_uploads_originals();
        let Some(pairing) = &self.sirv_pairing else {
            return;
        };
        let CdnHost::Ready(host) = &pairing.cdn_host else {
            return;
        };
        let files = match &pairing.files {
            Listing::Ready(files) => Some(files),
            _ if originals => return,
            _ => None,
        };
        let mut rows = self.result_paths.iter().collect::<Vec<_>>();
        rows.sort_by_key(|(index, _)| **index);
        let mut plan = Vec::new();
        let mut urls = Vec::new();
        let mut differs = Vec::new();
        let mut nested = Vec::new();
        for (index, output) in rows {
            let Some(entry) = self.entries.get(*index) else {
                continue;
            };
            let (key, upload) = match files.filter(|_| originals) {
                Some(files) => match original_plan(&self.root, entry, files, pairing.deep) {
                    Some(OriginalPlan::Upload(key)) => (key, Some(entry.path.clone())),
                    Some(OriginalPlan::Link(key)) => (key, None),
                    Some(OriginalPlan::Differs(key)) => {
                        differs.push(key);
                        continue;
                    }
                    None => {
                        if let Some(key) = sirv::relative_key(&self.root, &entry.path) {
                            nested.push(key);
                        }
                        continue;
                    }
                },
                None => {
                    let Some(key) = result_key(&self.root, entry, output) else {
                        continue;
                    };
                    (key, Some(output.clone()))
                }
            };
            let Ok(url) = sirv::public_url(host, &remote_path(&pairing.dir, &key)) else {
                continue;
            };
            if let Some(path) = upload {
                plan.push((key, path));
            }
            urls.push(url);
        }
        if !differs.is_empty() {
            self.notify_error(
                "sirv-publish",
                "Some originals were not published",
                format!(
                    "Sirv holds a different file under the same name: {}. Compare them in the Sirv bar first.",
                    differs.join(", ")
                ),
                cx,
            );
        }
        if !nested.is_empty() {
            self.notify_error(
                "sirv-publish-nested",
                "Some originals were not published",
                format!(
                    "The pairing compares one folder level, so files in subfolders stay put: {}.",
                    named(nested.into_iter())
                ),
                cx,
            );
        }
        if plan.is_empty() {
            // Everything is already on Sirv: nothing to upload, only links.
            if !urls.is_empty() {
                self.published_results = urls;
                cx.notify();
            }
            return;
        }
        // Originals promise never to replace a file on Sirv; converted
        // results under optimized/ are meant to be published over.
        self.run_upload_plan(plan, SirvJobKind::Publish, Some(urls), originals, cx);
    }

    pub(super) fn copy_result_embeds(&mut self, cx: &mut Context<Self>) {
        let text = self
            .published_results
            .iter()
            .map(|url| image_embed(url, self.published_alts.get(url).map_or("", String::as_str)))
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
    }

    /// Published URLs that still need alt text.
    pub(super) fn alt_text_pending(&self) -> usize {
        self.published_results
            .iter()
            .filter(|url| !self.published_alts.contains_key(*url))
            .count()
    }

    pub(super) fn alt_text_running(&self) -> bool {
        self.alt_job.as_ref().is_some_and(|job| !job.finished)
    }

    /// Stop the running alt text job at the next image.
    pub(super) fn stop_alt_text(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = self.alt_job.as_ref() {
            job.cancelled
                .store(true, std::sync::atomic::Ordering::Release);
        }
        cx.notify();
    }

    /// Ask Studio for alt text for every published URL that lacks it, one paid
    /// request at a time. Studio fetches the public Sirv URL, so no image
    /// leaves this computer here. The first failure stops the job: a missing
    /// credit or a refused key would fail every later request the same way,
    /// and the next click retries only what is still missing.
    pub(super) fn write_alt_text(&mut self, cx: &mut Context<Self>) {
        if self.alt_text_running() {
            return;
        }
        let Some(key) = self.studio_key.clone() else {
            return;
        };
        let urls = self
            .published_results
            .iter()
            .filter(|url| !self.published_alts.contains_key(*url))
            .cloned()
            .collect::<Vec<_>>();
        if urls.is_empty() {
            return;
        }
        self.clear_error("alt-text", cx);
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.alt_job = Some(AltJob {
            done: 0,
            total: urls.len(),
            credits_used: 0.,
            failure: None,
            finished: false,
            cancelled: cancelled.clone(),
        });
        cx.notify();

        cx.spawn(async move |this, cx| {
            let owns = |audit: &Audit| {
                audit
                    .alt_job
                    .as_ref()
                    .is_some_and(|job| Arc::ptr_eq(&job.cancelled, &cancelled))
            };
            for url in urls {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                let result = cx
                    .background_executor()
                    .spawn({
                        let key = key.clone();
                        let url = url.clone();
                        // A published original can be a 10 MB PNG. The model
                        // needs a picture, not the master, so Sirv sends a
                        // small JPEG any fetcher can decode.
                        async move { studio::alt_text(&key, &format!("{url}?w=1024&format=jpg")) }
                    })
                    .await;
                let failed = this
                    .update(cx, |audit, cx| {
                        if !owns(audit) {
                            return true;
                        }
                        let Some(job) = audit.alt_job.as_mut() else {
                            return true;
                        };
                        job.done += 1;
                        let failed = match result {
                            Ok((alt, used)) => {
                                job.credits_used += used.unwrap_or(0.);
                                audit.published_alts.insert(url, alt);
                                false
                            }
                            Err(message) => {
                                let name = url.rsplit('/').next().unwrap_or(&url).to_string();
                                job.failure = Some(format!("{name}: {message}"));
                                true
                            }
                        };
                        cx.notify();
                        failed
                    })
                    .unwrap_or(true);
                if failed {
                    break;
                }
            }
            let _ = this.update(cx, |audit, cx| {
                if !owns(audit) {
                    return;
                }
                let Some(job) = audit.alt_job.as_mut() else {
                    return;
                };
                job.finished = true;
                if let Some(failure) = job.failure.clone() {
                    audit.notify_error("alt-text", "Alt text stopped", failure, cx);
                }
                audit.refresh_studio_credits(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// One line for under the alt text button while or after a job runs.
    pub(super) fn alt_text_status(&self) -> Option<String> {
        let job = self.alt_job.as_ref()?;
        let spent = if job.credits_used > 0. {
            format!(" · {} used", studio::format_credits(job.credits_used))
        } else {
            String::new()
        };
        Some(match (&job.failure, job.finished) {
            (Some(failure), _) => {
                format!("Alt text {} of {}{spent} · {failure}", job.done, job.total)
            }
            (None, false) => format!("Writing alt text {} of {}…{spent}", job.done, job.total),
            (None, true) => format!("Alt text written for {} of {}{spent}", job.done, job.total),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_embeds_are_responsive_and_lazy_loaded() {
        let image = image_embed("https://demo.sirv.com/a.jpg", "");
        assert!(image.contains("srcset="));
        assert!(image.contains("loading=\"lazy\""));
        assert!(image.contains("alt=\"\""));
    }

    #[test]
    fn model_alt_text_cannot_break_out_of_the_attribute() {
        let image = image_embed(
            "https://demo.sirv.com/a.jpg",
            r#"A "red" chair <b> & a lamp"#,
        );
        assert!(image.ends_with(r#"alt="A &quot;red&quot; chair &lt;b&gt; &amp; a lamp">"#));
    }

    #[test]
    fn originals_upload_when_missing_link_when_equal_and_never_overwrite() {
        let root = Path::new("/shop");
        let entry = |name: &str, bytes: u64| Entry {
            path: root.join(name),
            format: image::ImageFormat::Jpeg.into(),
            width: 1,
            height: 1,
            bytes,
        };
        let node = |size: u64| {
            serde_json::from_str::<sirv::Node>(&format!(r#"{{"filename":"x","size":{size}}}"#))
                .unwrap()
        };
        let files: HashMap<String, sirv::Node> = [
            ("same.jpg".to_string(), node(100)),
            ("differs.jpg".to_string(), node(7)),
        ]
        .into();
        assert_eq!(
            original_plan(root, &entry("new.jpg", 100), &files, false),
            Some(OriginalPlan::Upload("new.jpg".into()))
        );
        assert_eq!(
            original_plan(root, &entry("same.jpg", 100), &files, false),
            Some(OriginalPlan::Link("same.jpg".into()))
        );
        assert_eq!(
            original_plan(root, &entry("differs.jpg", 100), &files, false),
            Some(OriginalPlan::Differs("differs.jpg".into()))
        );
        // A subfolder is outside the one-level listing, so its file is never
        // planned: "missing" there only means "not looked for".
        assert_eq!(
            original_plan(root, &entry("sub/new.jpg", 100), &files, false),
            None
        );
    }
}
