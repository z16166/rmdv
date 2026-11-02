//! Fonts bundled into the binary, registered with Iced at startup.
//!
//! cosmic-text only accepts a face whose weight equals the requested weight
//! (monospace faces excepted), and fontdb reads a variable font as a single
//! face at its default weight. So `Inter-Variable.ttf` serves weight 400 only;
//! the static instances below (cut from it at wght 500/600/700, subset to
//! Latin, Greek, Cyrillic, punctuation, arrows and math) serve the others.
//! Without them a bold or medium "Inter" run falls through to JetBrains Mono.
//! Inter is licensed under the SIL OFL 1.1; see `assets/fonts/Inter-OFL.txt`.
//!
//! `NotoSansTC-{Regular,Bold}.ttf` carry CJK text (Traditional forms), since
//! the macOS system fallback resolves to Arial Unicode MS. They are static
//! 400/700 instances of Noto Sans TC (SIL OFL 1.1, see
//! `assets/fonts/NotoSansTC-OFL.txt`) subset to Big5 common hanzi, GB2312
//! level-1 hanzi, CJK and fullwidth punctuation, kana and bopomofo; rarer
//! characters still fall back to the system.
//!
//! `RmdvCJKPunct-{Regular,Bold}.ttf` hold only the fullwidth brackets and
//! stops, at their own code points, with the half-width shapes of the
//! font's `halt` feature. They are a derived subset (family renamed, since
//! the Noto Sans TC copyright names the Reserved Font Name 'Source'); text
//! that [`squeeze_segments`] marks as narrow is set in this family, so the
//! text itself never changes. cosmic-text shapes every punctuation mark as
//! its own word, so OpenType pair kerning cannot do this.

pub const BUNDLED: [&[u8]; 10] = [
    include_bytes!("assets/fonts/Inter-Variable.ttf"),
    include_bytes!("assets/fonts/Inter-Medium.ttf"),
    include_bytes!("assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("assets/fonts/Inter-Bold.ttf"),
    include_bytes!("assets/fonts/JetBrainsMono-Regular.otf"),
    include_bytes!("assets/fonts/lucide.ttf"),
    include_bytes!("assets/fonts/NotoSansTC-Regular.ttf"),
    include_bytes!("assets/fonts/NotoSansTC-Bold.ttf"),
    include_bytes!("assets/fonts/RmdvCJKPunct-Regular.ttf"),
    include_bytes!("assets/fonts/RmdvCJKPunct-Bold.ttf"),
];

/// Brackets and stops that the half-width face covers: opening brackets
/// first (the first `OPENING` characters), then closing brackets and stops.
const SQUEEZABLE: &str = "「『（【〔〈《［｛」』）】〕〉》］｝，。、．";
const OPENING: usize = 9;

fn is_squeezable(c: char) -> bool {
    SQUEEZABLE.contains(c)
}

fn is_opening(c: char) -> bool {
    SQUEEZABLE.chars().take(OPENING).any(|o| o == c)
}

/// Split `text` into consecutive segments, each flagged "narrow" or not
/// (CSS `text-spacing-trim: normal`). A closing bracket or stop followed by
/// another bracket or stop, and an opening bracket followed by an opening
/// bracket, are narrow: the caller sets them in [`PUNCT_FAMILY`]. The
/// segments always concatenate to `text`.
pub fn squeeze_segments(text: &str) -> Vec<(&str, bool)> {
    let mut out: Vec<(&str, bool)> = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut start = 0usize;
    let mut current: Option<bool> = None;
    while let Some((i, c)) = chars.next() {
        let narrow = match chars.peek() {
            Some(&(_, n)) if is_squeezable(n) && is_squeezable(c) => {
                !is_opening(c) || is_opening(n)
            }
            _ => false,
        };
        match current {
            Some(flag) if flag != narrow => {
                out.push((&text[start..i], flag));
                start = i;
                current = Some(narrow);
            }
            None => current = Some(narrow),
            _ => {}
        }
    }
    if let Some(flag) = current {
        out.push((&text[start..], flag));
    }
    out
}

/// Family name of the bundled CJK face (Noto Sans TC, Traditional forms).
pub const CJK_FAMILY: &str = "Noto Sans TC";

/// Family name of the half-width punctuation face (same code points).
pub const PUNCT_FAMILY: &str = "rmdv CJK Punct";

/// Load the OS font list into Iced's font system exactly once. The first
/// caller pays the few-hundred-millisecond scan, wherever it runs (the first
/// `view()`, a background font check); later calls are no-ops.
pub fn ensure_system_fonts_loaded() {
    static LOADED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    LOADED.get_or_init(|| {
        let fs = iced::advanced::graphics::text::font_system();
        if let Ok(mut guard) = fs.write() {
            guard.raw().db_mut().load_system_fonts();
        }
        if std::env::var_os("RMDV_BENCH_STARTUP").is_some() {
            if let Some(d) = crate::bench::since_process_start() {
                eprintln!("startup: fonts_loaded={:?}", d);
            }
        }
    });
}

/// Whether `family` draws CJK with its own regular face. When it does, the
/// reader lets CJK runs keep the user's chosen family instead of pinning
/// them to the bundled Noto Sans TC. Meant to run on a background worker:
/// the first call pays the system font scan.
///
/// Regular weight only, and either a simplified or a traditional hanzi is
/// enough. Bold faces are often registered under a second family name
/// (`Microsoft YaHei Bold`), and requiring both weights and both characters
/// rejected families that draw body text themselves. Those runs — often the
/// first column of a table — then stayed on Noto Sans TC.
pub fn family_covers_cjk(family: &str) -> bool {
    use iced::advanced::graphics::text::cosmic_text::{
        Attrs, Buffer, Family, Metrics, Shaping, Weight,
    };
    ensure_system_fonts_loaded();
    let fs = iced::advanced::graphics::text::font_system();
    let Ok(mut guard) = fs.write() else {
        return false;
    };
    let fonts = guard.raw();
    ["中", "體"].iter().any(|sample| {
        let mut buffer = Buffer::new(fonts, Metrics::new(16.0, 20.0));
        buffer.set_text(
            fonts,
            sample,
            &Attrs::new()
                .family(Family::Name(family))
                .weight(Weight::NORMAL),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(fonts, false);
        let mut saw_glyph = false;
        let covered = buffer
            .layout_runs()
            .flat_map(|run| run.glyphs)
            .all(|glyph| {
                saw_glyph = true;
                fonts
                    .db()
                    .face(glyph.font_id)
                    .is_some_and(|face| face.families.iter().any(|(name, _)| name == family))
            });
        saw_glyph && covered
    })
}

/// Enumerate the OS font families available to Iced's text shaper. Meant to
/// run on a background worker (the caller wraps it in `Task::perform`): the
/// first call loads the system font list, which takes a few hundred
/// milliseconds and must not block the UI thread. The result is sorted
/// case-insensitively and deduplicated.
pub fn list_families() -> Vec<String> {
    use std::collections::HashSet;
    let fs = iced::advanced::graphics::text::font_system();
    let mut seen = HashSet::new();
    let mut families = Vec::new();
    if let Ok(mut guard) = fs.write() {
        // The first `view()` defers `load_system_fonts`; opening Settings
        // before any view would otherwise see only the bundled faces. It
        // is idempotent, so calling it again is cheap.
        guard.raw().db_mut().load_system_fonts();
        for face in guard.raw().db().faces() {
            for (family, _lang) in &face.families {
                if seen.insert(family.clone()) {
                    families.push(family.clone());
                }
            }
        }
    }
    families.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    families
}

#[cfg(test)]
mod tests {
    use super::BUNDLED;
    use iced::advanced::graphics::text::cosmic_text::{
        fontdb, Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Weight,
    };
    use std::sync::Arc;

    /// Shape `text` the way Iced does (family "Inter" at `weight`, only the
    /// bundled fonts loaded) and return the family of each glyph's face.
    fn shaped_families(text: &str, weight: Weight) -> Vec<String> {
        shaped_families_in("Inter", text, weight)
    }

    fn shaped_families_in(family: &'static str, text: &str, weight: Weight) -> Vec<String> {
        let sources = BUNDLED
            .iter()
            .map(|bytes| fontdb::Source::Binary(Arc::new(*bytes)));
        let mut fonts = FontSystem::new_with_fonts(sources);
        let mut buffer = Buffer::new(&mut fonts, Metrics::new(16.0, 20.0));
        buffer.set_text(
            &mut fonts,
            text,
            &Attrs::new().family(Family::Name(family)).weight(weight),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut fonts, false);
        let mut out = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let face = fonts.db().face(glyph.font_id).expect("face");
                out.push(face.families[0].0.clone());
            }
        }
        out
    }

    #[test]
    fn noto_sans_tc_draws_the_cjk_coverage_sample_at_regular_weight() {
        // `family_covers_cjk` asks only this: regular weight, and either
        // simplified 中 or traditional 體. Bold is a separate family name
        // on many system fonts and must not fail the probe.
        for sample in ["中", "體"] {
            let families = shaped_families_in(super::CJK_FAMILY, sample, Weight::NORMAL);
            assert!(
                families.iter().all(|f| f == super::CJK_FAMILY),
                "{sample} shaped with {families:?}"
            );
        }
    }

    #[test]
    fn inter_weights_resolve_to_inter_not_the_monospace_fallback() {
        for weight in [
            Weight::NORMAL,
            Weight::MEDIUM,
            Weight::SEMIBOLD,
            Weight::BOLD,
        ] {
            let families = shaped_families("Plain bold words", weight);
            assert!(!families.is_empty());
            assert!(
                families.iter().all(|f| f == "Inter"),
                "weight {weight:?} shaped with {families:?}"
            );
        }
    }

    /// Sum of glyph advances (px at 16 px) for `text`, each squeeze segment
    /// shaped in its own family as the renderer does.
    fn cjk_advance(text: &str) -> f32 {
        let sources = BUNDLED
            .iter()
            .map(|bytes| fontdb::Source::Binary(Arc::new(*bytes)));
        let mut fonts = FontSystem::new_with_fonts(sources);
        let mut total = 0.0;
        for (segment, narrow) in super::squeeze_segments(text) {
            let family = if narrow {
                super::PUNCT_FAMILY
            } else {
                super::CJK_FAMILY
            };
            let mut buffer = Buffer::new(&mut fonts, Metrics::new(16.0, 24.0));
            buffer.set_size(&mut fonts, Some(100_000.0), None);
            buffer.set_text(
                &mut fonts,
                segment,
                &Attrs::new()
                    .family(Family::Name(family))
                    .weight(Weight::NORMAL),
                Shaping::Advanced,
                None,
            );
            buffer.shape_until_scroll(&mut fonts, false);
            total += buffer
                .layout_runs()
                .flat_map(|run| run.glyphs.iter().map(|g| g.w).collect::<Vec<_>>())
                .sum::<f32>();
        }
        total
    }

    #[test]
    fn cjk_text_resolves_to_the_bundled_noto_sans_tc() {
        for weight in [Weight::NORMAL, Weight::BOLD] {
            let families = shaped_families_in(super::CJK_FAMILY, "閱讀體驗，中文。", weight);
            assert!(!families.is_empty());
            assert!(
                families.iter().all(|f| f == "Noto Sans TC"),
                "weight {weight:?} shaped with {families:?}"
            );
        }
    }

    #[test]
    fn adjacent_fullwidth_brackets_and_stops_are_squeezed_to_half() {
        let eps = 0.01;
        // Closing + opening, closing + closing, opening + opening: first glyph halves.
        assert!(
            (cjk_advance("」「") - 24.0).abs() < eps,
            "{}",
            cjk_advance("」「")
        );
        assert!(
            (cjk_advance("）」") - 24.0).abs() < eps,
            "{}",
            cjk_advance("）」")
        );
        assert!(
            (cjk_advance("（「") - 24.0).abs() < eps,
            "{}",
            cjk_advance("（「")
        );
        assert!(
            (cjk_advance("。「") - 24.0).abs() < eps,
            "{}",
            cjk_advance("。「")
        );
        // Not adjacent to other punctuation: stay full width.
        assert!(
            (cjk_advance("「這」") - 48.0).abs() < eps,
            "{}",
            cjk_advance("「這」")
        );
        assert!(
            (cjk_advance("這。這") - 48.0).abs() < eps,
            "{}",
            cjk_advance("這。這")
        );
        assert!((cjk_advance("。") - 16.0).abs() < eps);
    }

    #[test]
    fn squeeze_segments_cover_the_text_exactly_and_mark_only_adjacent_punctuation() {
        use super::squeeze_segments as seg;
        let joined = |t: &str| seg(t).iter().map(|(s, _)| *s).collect::<String>();
        for s in [
            "」「",
            "他說（好",
            "是）。",
            "他說：「好」。「對」",
            "沒有標點",
            "",
        ] {
            assert_eq!(joined(s), s);
        }
        assert_eq!(seg("「這」"), vec![("「這」", false)]);
        assert_eq!(seg("沒有標點"), vec![("沒有標點", false)]);
        assert_eq!(seg("」「"), vec![("」", true), ("「", false)]);
        assert_eq!(
            seg("是）。"),
            vec![("是", false), ("）", true), ("。", false)]
        );
        assert_eq!(seg("（「"), vec![("（", true), ("「", false)]);
        // Opening followed by closing is left alone; colon is not squeezed.
        assert_eq!(seg("「」"), vec![("「」", false)]);
        assert_eq!(seg("：「"), vec![("：「", false)]);
    }

    #[test]
    fn punct_face_keeps_original_code_points_in_its_own_family() {
        for weight in [Weight::NORMAL, Weight::BOLD] {
            let families = shaped_families_in(super::PUNCT_FAMILY, "」「，。", weight);
            assert!(
                families.iter().all(|f| f == "rmdv CJK Punct"),
                "{families:?}"
            );
        }
    }
}
