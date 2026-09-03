//! Asset source: Studio's own tool icons, then everything gpui-component ships.
//!
//! The Studio rail draws the glyph each direct API tool has inside Studio, so a
//! person who learns the tool in one place recognises it in the other.
//!
//! The files are Hugeicons, MIT, taken from the same catalogue Studio renders
//! from — `EXPECTED_TOOL_GLYPHS` in the Studio repository names the mapping.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

/// Path to bytes, resolved before the bundled set. `include_bytes!` keeps them
/// in the binary, which is what every other build target already assumes about
/// icons: no file beside the executable, nothing to install.
macro_rules! studio_icons {
    ($($slug:literal),* $(,)?) => {
        &[$((
            concat!("icons/studio/", $slug, ".svg"),
            include_bytes!(concat!("../assets/icons/studio/", $slug, ".svg")).as_slice(),
        )),*]
    };
}

const STUDIO_ICONS: &[(&str, &[u8])] = studio_icons![
    "image-to-image",
    "background-removal",
    "background-replace",
    "upscale",
    "product-lifestyle",
];

/// The icon path for a Studio tool slug, or `None` when this build ships no
/// glyph for it. A missing icon draws nothing, so the caller decides.
pub fn studio_icon(slug: &str) -> Option<&'static str> {
    let wanted = format!("icons/studio/{slug}.svg");
    STUDIO_ICONS
        .iter()
        .map(|(path, _)| *path)
        .find(|path| *path == wanted)
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = STUDIO_ICONS.iter().find(|(name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut names: Vec<SharedString> = STUDIO_ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect();
        names.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_studio_tool_has_its_studio_glyph() {
        for tool in crate::studio::TOOLS {
            let slug = tool.slug();
            assert!(
                studio_icon(slug).is_some(),
                "{slug} is offered in the rail with no icon"
            );
        }
    }

    #[test]
    fn studio_icons_load_before_the_bundled_set_and_do_not_hide_it() {
        assert!(
            Assets
                .load("icons/studio/background-removal.svg")
                .expect("loads")
                .is_some()
        );
        assert!(Assets.load("icons/close.svg").expect("loads").is_some());
    }
}
