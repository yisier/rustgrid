//! The changelog shown at the foot of 选项 → 关于 — what installing the newer release would bring.
//!
//! It only ever describes **another** version, so it appears only while a check has found one (see
//! `update.rs`): with this build already the newest release there is nothing to describe, and the
//! section stays off the page rather than repeating the version shown above it.
//!
//! The notes are read from the repository's curated changelog for the UI's own language: the
//! language's file (`CHANGELOG_<language>.md`, `CHANGELOG.md` for English) and the version's section
//! inside it. A language with no file of its own — or one whose translation has not reached the
//! release yet — falls back to English, so 关于 always has something truthful to show. The release
//! body is no longer a changelog: the release workflow publishes links to these files instead (see
//! `.github/workflows/release.yml`), so adding a language never grows it.

use super::*;

/// The English changelog, which every language falls back to.
pub(super) const ENGLISH_CHANGELOG: &str = "CHANGELOG.md";

/// The marker the release workflow puts in the link-only release body it publishes. Such a body is
/// navigation, not notes, so 关于 never renders it.
pub(super) const CHANGELOG_LINKS_MARKER: &str = "<!-- rustgrid:changelog -->";

/// The changelog file that documents `language`: `CHANGELOG.md` for English, and
/// `CHANGELOG_<language>.md` (`zh-CN` → `CHANGELOG_zh.md`) otherwise. The file may not exist yet —
/// a language can ship its UI before its changelog is translated — and [`version_notes`] then falls
/// back to English.
pub(super) fn changelog_file(language: LanguageSetting) -> String {
    match language {
        LanguageSetting::En => ENGLISH_CHANGELOG.to_string(),
        other => {
            let language = other.locale().split('-').next().unwrap_or_default();
            format!("CHANGELOG_{language}.md")
        }
    }
}

/// Whether a release body is the link-only one the release job writes (see
/// `.github/workflows/release.yml`), which 关于 must not render as notes.
pub(super) fn is_changelog_links(body: &str) -> bool {
    body.contains(CHANGELOG_LINKS_MARKER)
}

/// `version`'s section of the changelog in `language`, falling back to English when the language
/// has no changelog of its own or its translation has not reached this release.
///
/// `fetch` reads a repository file by name (see `update::fetch_repo_file`); taking it as a parameter
/// keeps the fallback testable without the network.
pub(super) fn version_notes(
    version: &str,
    language: LanguageSetting,
    fetch: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let localized = changelog_file(language);
    if let Some(section) = fetch(&localized).and_then(|text| version_section(&text, version)) {
        return Some(section);
    }
    if localized == ENGLISH_CHANGELOG {
        return None;
    }
    fetch(ENGLISH_CHANGELOG).and_then(|text| version_section(&text, version))
}

/// One line of a version's notes, classified so 关于 can style it.
pub(super) enum NoteLine {
    /// A version heading: `0.0.11`, with the release date when the changelog names one.
    Version {
        version: String,
        date: Option<String>,
    },
    /// A group heading (`### Added` / `### 新增`).
    Group(String),
    /// A bullet item, its wrapped continuation lines joined.
    Bullet(String),
    /// A plain paragraph (e.g. the changelog's `First release.`).
    Text(String),
}

/// The notes parsed into renderable lines. Empty when there is no notes text, which is what keeps
/// the section off the page (see the module docs).
pub(super) fn notes(text: Option<&str>) -> Vec<NoteLine> {
    text.map(parse).unwrap_or_default()
}

/// The changelog block for one version, as written in the file (heading included).
pub(super) fn version_section(text: &str, version: &str) -> Option<String> {
    version_blocks(text)
        .into_iter()
        .find(|block| block_is_version(block, version))
        .map(|block| block.trim_end().to_string())
}

/// The changelog's blocks, each starting at its `## [version]` heading, in file order (newest
/// first). Everything before the first heading — the file's title and preamble — is dropped.
fn version_blocks(text: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut start = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let heading = line.trim_end_matches(['\r', '\n']).starts_with("## [");
        if heading {
            if let Some(from) = start {
                blocks.push(&text[from..offset]);
            }
            start = Some(offset);
        }
        offset += line.len();
    }
    if let Some(from) = start {
        blocks.push(&text[from..]);
    }
    blocks
}

/// Whether a block is the entry for `version`. The heading is parsed rather than prefix-matched, so
/// `0.0.1` never claims `0.0.11`'s entry.
fn block_is_version(block: &str, version: &str) -> bool {
    version_heading(block).is_some_and(|(found, _date)| found == version)
}

/// A `## [0.0.11] - 2026-10-15` heading, as its version and (optional) date.
fn version_heading(block: &str) -> Option<(String, Option<String>)> {
    let heading = block.lines().next()?.trim_end().strip_prefix("## ")?;
    let rest = heading.strip_prefix('[')?;
    let (version, rest) = rest.split_once(']')?;
    let date = rest
        .trim()
        .strip_prefix('-')
        .map(str::trim)
        .filter(|date| !date.is_empty())
        .map(str::to_string);
    Some((version.trim().to_string(), date))
}

/// Split a version's notes into renderable lines: headings, group headings, bullets (with their
/// wrapped continuation lines joined) and paragraphs.
pub(super) fn parse(text: &str) -> Vec<NoteLine> {
    let mut lines = Vec::new();
    let mut bullet: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("<!--") {
            push_bullet(&mut lines, &mut bullet);
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ") {
            push_bullet(&mut lines, &mut bullet);
            match version_heading(line) {
                Some((version, date)) => lines.push(NoteLine::Version { version, date }),
                None => lines.push(NoteLine::Group(strip_inline(heading))),
            }
        } else if let Some(heading) = line.strip_prefix("### ") {
            push_bullet(&mut lines, &mut bullet);
            lines.push(NoteLine::Group(strip_inline(heading)));
        } else if let Some(item) = line.strip_prefix("- ") {
            push_bullet(&mut lines, &mut bullet);
            bullet = Some(item.trim().to_string());
        } else if let Some(item) = bullet.as_mut() {
            // A wrapped bullet: the changelog keeps its entries inside 100 columns.
            item.push(' ');
            item.push_str(line);
        } else {
            lines.push(NoteLine::Text(strip_inline(line)));
        }
    }
    push_bullet(&mut lines, &mut bullet);
    lines
}

/// Close the bullet being collected, unescaping its inline markdown.
fn push_bullet(lines: &mut Vec<NoteLine>, bullet: &mut Option<String>) {
    if let Some(item) = bullet.take() {
        lines.push(NoteLine::Bullet(strip_inline(&item)));
    }
}

/// Drop the inline markdown a plain-text panel cannot render: `**emphasis**`, `` `code` `` and
/// `[label](target)` keep their text, not their syntax.
fn strip_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['*', '`', '[']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(inner) = tail.strip_prefix('[') {
            match inner.split_once(']') {
                // `[label](url)`: keep the label, skip the target.
                Some((label, after)) if after.starts_with('(') => {
                    out.push_str(label);
                    rest = after.split_once(')').map_or(after, |(_url, rest)| rest);
                }
                _ => {
                    out.push('[');
                    rest = inner;
                }
            }
        } else {
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::super::update;
    use super::super::update::APP_VERSION;
    use super::*;

    /// The repository changelogs, as the release job reads them. 关于 itself no longer embeds them —
    /// it describes only a newer release — but the rule that every version is written into both is
    /// still what gives the release job (and the panel) something to publish, so a test reads them.
    const CHANGELOG_EN: &str = include_str!("../../../../CHANGELOG.md");
    const CHANGELOG_ZH: &str = include_str!("../../../../CHANGELOG_zh.md");

    const SAMPLE: &str = "\
# Changelog

Intro.

## [0.0.2] - 2026-02-02

### Added

- A **bold** point that wraps onto
  the next line, with `code`.
- Another point.

## [0.0.1] - 2026-01-01

First release.
";

    #[test]
    fn a_version_block_stops_at_the_next_heading() {
        let section = version_section(SAMPLE, "0.0.2").expect("0.0.2 has an entry");
        assert!(section.contains("A **bold** point"));
        assert!(!section.contains("First release"));
    }

    #[test]
    fn a_version_never_claims_its_own_prefix() {
        // `0.0.1` must not match `0.0.11`'s heading, which starts with the same characters.
        let changelog = "## [0.0.11] - 2026-03-03\n\n### Added\n\n- eleven\n";
        assert_eq!(version_section(changelog, "0.0.1"), None);
        assert_eq!(
            version_section(changelog, "0.0.11").as_deref(),
            Some(changelog.trim_end())
        );
    }

    #[test]
    fn notes_are_classified_and_unmarked() {
        let notes = parse(version_section(SAMPLE, "0.0.2").unwrap().as_str());
        assert!(matches!(
            notes.first(),
            Some(NoteLine::Version { version, date })
                if version == "0.0.2" && date.as_deref() == Some("2026-02-02")
        ));
        assert!(matches!(&notes[1], NoteLine::Group(group) if group == "Added"));
        match &notes[2] {
            NoteLine::Bullet(item) => assert_eq!(
                item,
                "A bold point that wraps onto the next line, with code."
            ),
            _ => panic!("expected the wrapped bullet to be joined"),
        }
        assert!(matches!(&notes[3], NoteLine::Bullet(item) if item == "Another point."));
    }

    #[test]
    fn a_language_reads_its_own_changelog_and_falls_back_to_english() {
        let english = "## [0.1.1]\n\n- english\n";
        let chinese = "## [0.1.1]\n\n- 中文\n";
        let files = |file: &str| match file {
            "CHANGELOG.md" => Some(english.to_string()),
            "CHANGELOG_zh.md" => Some(chinese.to_string()),
            _ => None,
        };

        assert_eq!(changelog_file(LanguageSetting::En), "CHANGELOG.md");
        assert_eq!(changelog_file(LanguageSetting::ZhCn), "CHANGELOG_zh.md");
        assert_eq!(changelog_file(LanguageSetting::Ru), "CHANGELOG_ru.md");
        assert_eq!(
            version_notes("0.1.1", LanguageSetting::ZhCn, files).as_deref(),
            Some(chinese.trim_end())
        );
        // No Russian changelog yet; English covers it.
        assert_eq!(
            version_notes("0.1.1", LanguageSetting::Ru, files).as_deref(),
            Some(english.trim_end())
        );

        // A translation that has not reached this version falls back to English too.
        let stale = |file: &str| match file {
            "CHANGELOG.md" => Some(english.to_string()),
            "CHANGELOG_zh.md" => Some("## [0.1.0]\n\n- 旧\n".to_string()),
            _ => None,
        };
        assert_eq!(
            version_notes("0.1.1", LanguageSetting::ZhCn, stale).as_deref(),
            Some(english.trim_end())
        );
        // Neither file documents it: no notes.
        assert_eq!(version_notes("9.9.9", LanguageSetting::En, |_| None), None);
    }

    #[test]
    fn a_link_only_release_body_is_not_notes() {
        assert!(is_changelog_links(&format!(
            "{CHANGELOG_LINKS_MARKER}\n### Changelog\n\n- [English](https://example.com)\n"
        )));
        assert!(!is_changelog_links(""));
        assert!(!is_changelog_links(
            "**Full Changelog**: https://github.com/yisier/rustgrid/compare/v0.0.10...v0.0.11"
        ));
    }

    #[test]
    fn notes_parse_the_version_section_of_a_changelog() {
        let section = version_section(SAMPLE, "0.0.2").expect("0.0.2 has an entry");
        let notes = notes(Some(section.as_str()));
        assert!(matches!(
            notes.first(),
            Some(NoteLine::Version { version, .. }) if version == "0.0.2"
        ));
        assert!(
            notes
                .iter()
                .any(|note| matches!(note, NoteLine::Bullet(item) if item == "Another point."))
        );
    }

    #[test]
    fn a_release_that_documents_nothing_carries_no_notes() {
        // What keeps the 更新日志 section off the page when no changelog documents the newer
        // version: 关于 then has nothing truthful to show — the running build cannot embed an entry
        // for a version it is older than.
        assert!(notes(None).is_empty());
        assert!(notes(Some("")).is_empty());
        assert!(notes(Some("   \n")).is_empty());
        assert!(notes(Some("<!-- rustgrid:changelog -->\n\n")).is_empty());
    }

    #[test]
    #[ignore = "network: fetches the repository's changelogs from the GitHub API"]
    fn the_repository_changelog_documents_another_version() {
        // The live half: the contents API really answers with the file text (the media type and the
        // User-Agent GitHub demands are not something a stub can check), and the version's section
        // comes back in every UI language, English standing in where a translation is missing. The
        // version is taken from the fetched file rather than from this build, which may carry a bump
        // the repository has not seen yet.
        let english = update::fetch_repo_file(ENGLISH_CHANGELOG, UpdateSource::Github)
            .expect("CHANGELOG.md is fetchable");
        let newest = version_blocks(&english)
            .into_iter()
            .filter_map(version_heading)
            .map(|(version, _)| version)
            .find(|version| version != "Unreleased")
            .expect("the English changelog documents a version");

        for language in [
            LanguageSetting::En,
            LanguageSetting::ZhCn,
            LanguageSetting::Ru,
        ] {
            let section = version_notes(&newest, language, |file| {
                update::fetch_repo_file(file, UpdateSource::Github)
            })
            .unwrap_or_else(|| panic!("{language:?}: no changelog documents {newest}"));
            assert!(
                section.starts_with(&format!("## [{newest}]")),
                "{language:?}: 关于 must show {newest}'s entry, got {section}"
            );
        }
    }

    #[test]
    fn the_changelogs_document_the_running_version() {
        // Guards the release rule: bumping the workspace version without writing the entry in both
        // changelogs leaves the release job with nothing to publish — and 关于 with nothing to show
        // for that version while it is the newer release.
        for (name, changelog) in [
            ("CHANGELOG.md", CHANGELOG_EN),
            ("CHANGELOG_zh.md", CHANGELOG_ZH),
        ] {
            let section = version_section(changelog, APP_VERSION)
                .unwrap_or_else(|| panic!("{name} has no entry for version {APP_VERSION}"));
            assert!(
                section.starts_with(&format!("## [{APP_VERSION}]")),
                "{name}'s {APP_VERSION} entry must start at its own heading"
            );
        }
    }

    #[test]
    fn the_english_changelog_is_written_in_english() {
        // Each changelog stays in its own language, so its UI labels read the way that language's
        // `locales/*.yml` has them — 选项 → 关于 is "Options → About" here. The header's language
        // switcher and the name of the other language are the only exceptions.
        const ALLOWED: &str = "简体中文";
        let offenders = CHANGELOG_EN
            .lines()
            .enumerate()
            .filter(|(_, line)| line.replace(ALLOWED, "").chars().any(is_cjk))
            .map(|(index, line)| format!("{}: {}", index + 1, line.trim()))
            .collect::<Vec<_>>();
        assert!(
            offenders.is_empty(),
            "CHANGELOG.md must be English throughout — quote UI labels from locales/en.yml:\n{}",
            offenders.join("\n")
        );
    }

    /// Han characters, CJK punctuation and full-width forms, i.e. text that is not English.
    fn is_cjk(c: char) -> bool {
        matches!(
            c as u32,
            0x3000..=0x303F | 0x3040..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF
        )
    }
}
