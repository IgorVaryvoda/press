//! Publish converted images to Sirv and copy responsive embed markup.

use super::*;

pub(super) fn image_embed(url: &str) -> String {
    format!(
        "<img src=\"{url}?w=1280\" srcset=\"{url}?w=640 640w, {url}?w=1280 1280w, {url}?w=1920 1920w\" sizes=\"100vw\" loading=\"lazy\" alt=\"\">"
    )
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
        self.run_upload_plan(plan, SirvJobKind::Publish, Some(urls), cx);
    }

    pub(super) fn copy_result_embeds(&mut self, cx: &mut Context<Self>) {
        let text = self
            .published_results
            .iter()
            .map(|url| image_embed(url))
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_embeds_are_responsive_and_lazy_loaded() {
        let image = image_embed("https://demo.sirv.com/a.jpg");
        assert!(image.contains("srcset="));
        assert!(image.contains("loading=\"lazy\""));
    }
}
