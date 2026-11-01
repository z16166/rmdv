use std::path::{Path, PathBuf};

/// A bounded count of files rmdv can open inside a folder.
///
/// This deliberately mirrors the workspace tree's filtering rules.  The cap
/// keeps folder-selection previews useful without turning a quick peek into an
/// unbounded filesystem walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportedFileCount {
    pub count: usize,
    pub capped: bool,
}

pub const FOLDER_FILE_COUNT_MAX_DEPTH: usize = 12;
pub const FOLDER_FILE_COUNT_MAX_FILES: usize = 5_000;
/// Maximum directory entries examined for one chooser preview. This bounds
/// background work even when a tree contains only unsupported files.
pub const FOLDER_FILE_COUNT_MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_md: bool,
}

#[derive(Debug, Clone)]
pub struct Picker {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub selected: usize,
    /// A read error for the current folder.
    pub error: Option<String>,
    /// The last rejected path-field submission; cleared on a successful one.
    pub path_error: Option<String>,
    /// Jump targets for the root row: every available drive on Windows,
    /// `/` and mounted volumes elsewhere.
    pub roots: Vec<PathBuf>,
    pub mode: PickerMode,
    pub show_hidden: bool,
}

/// What a submitted path field resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathTarget {
    /// A folder: the picker is already showing it.
    Folder,
    /// A supported document: the caller should open it.
    File(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    /// Pick any folder or markdown file (used when no workspace yet).
    OpenAny,
    /// Pick only folders (workspace root selection).
    Folder,
}

impl Picker {
    pub fn new(start: Option<PathBuf>, mode: PickerMode, show_hidden: bool) -> Self {
        let cwd = start
            .or_else(|| dirs::home_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        let mut p = Self {
            cwd,
            entries: Vec::new(),
            selected: 0,
            error: None,
            path_error: None,
            roots: Vec::new(),
            mode,
            show_hidden,
        };
        p.refresh();
        p
    }

    pub fn refresh(&mut self) {
        self.entries.clear();
        self.selected = 0;
        self.error = None;
        match std::fs::read_dir(&self.cwd) {
            Ok(rd) => {
                let mut items: Vec<Entry> = rd
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let path = e.path();
                        let name = e.file_name().to_string_lossy().into_owned();
                        // Always skip .git (huge, never wanted).
                        if name == ".git" {
                            return None;
                        }
                        // Other dot-entries gated by toggle.
                        if !self.show_hidden && name.starts_with('.') {
                            return None;
                        }
                        let is_dir = path.is_dir();
                        let is_md = !is_dir && is_markdown_path(&path);
                        match self.mode {
                            PickerMode::OpenAny => {
                                if !is_dir && !is_md {
                                    return None;
                                }
                            }
                            PickerMode::Folder => {
                                if !is_dir {
                                    return None;
                                }
                            }
                        }
                        Some(Entry {
                            name,
                            path,
                            is_dir,
                            is_md,
                        })
                    })
                    .collect();
                items.sort_by(|a, b| {
                    // Optional dot entries are additive. Keep ordinary folders
                    // first so enabling hidden files cannot crowd familiar
                    // destinations out of the initial Full Mindmap viewport.
                    a.name
                        .starts_with('.')
                        .cmp(&b.name.starts_with('.'))
                        .then_with(|| b.is_dir.cmp(&a.is_dir))
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                self.entries = items;
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
        // Drives and mounts can appear or disappear while the picker is
        // open, so refresh the root row alongside every folder listing.
        self.roots = Self::roots();
    }

    pub fn navigate_to(&mut self, p: PathBuf) {
        self.cwd = p;
        self.path_error = None;
        self.refresh();
    }

    pub fn parent(&mut self) {
        if let Some(parent) = self.cwd.parent() {
            self.cwd = parent.to_path_buf();
            self.path_error = None;
            self.refresh();
        }
    }

    /// The root jump targets for the picker's top row.
    ///
    /// Windows enumerates drive letters through `GetLogicalDrives`, which
    /// covers empty and disconnected drives without probing `X:\\` (probing
    /// can prompt for removable media). macOS lists `/` and the mounted
    /// volumes; other Unix platforms list `/` plus `/media` and `/mnt`.
    pub fn roots() -> Vec<PathBuf> {
        #[cfg(windows)]
        {
            let mut out: Vec<PathBuf> = drive_roots();
            out.sort();
            return out;
        }
        #[cfg(not(windows))]
        {
            let mut out = vec![PathBuf::from("/")];
            let mounted: &[&str] = if cfg!(target_os = "macos") {
                &["/Volumes"]
            } else {
                &["/media", "/mnt"]
            };
            for base in mounted {
                let Ok(rd) = std::fs::read_dir(base) else {
                    continue;
                };
                let mut found: Vec<PathBuf> = rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect();
                found.sort();
                out.extend(found);
            }
            out
        }
    }

    /// How a root jump target is labeled in the picker's root row.
    pub fn root_label(root: &Path) -> String {
        #[cfg(windows)]
        {
            return root.as_os_str().to_string_lossy().replace('/', "\\");
        }
        #[cfg(not(windows))]
        {
            if root == Path::new("/") {
                "/".to_string()
            } else {
                root.file_name().map_or_else(
                    || root.to_string_lossy().into_owned(),
                    |name| name.to_string_lossy().into_owned(),
                )
            }
        }
    }

    /// Resolve the text of the picker's path field and navigate there.
    ///
    /// Accepts absolute paths, UNC paths, `~` shorthands (always for the
    /// real home, not a workspace-relative path), and bare relative paths
    /// joined to the current folder. A folder navigates the picker; a
    /// supported document is handed back for the caller to open. Anything
    /// else sets `path_error` and leaves the listing untouched.
    pub fn navigate_path(&mut self, input: &str) -> Result<PathTarget, String> {
        let raw = input.trim();
        if raw.is_empty() {
            return Err(self.reject_path("Enter a folder or file path."));
        }
        let mut text = raw.to_string();
        if let Some(rest) = text.strip_prefix('~') {
            if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') {
                let Some(home) = dirs::home_dir() else {
                    return Err(self.reject_path("No home folder is available."));
                };
                text = format!("{}{}", home.display(), rest);
            }
        }
        // A bare drive letter (`D:`) is drive-relative on Windows; the
        // field's intent there is that drive's root.
        #[cfg(windows)]
        if text.len() == 2 && text.ends_with(':') && text.as_bytes()[0].is_ascii_alphabetic() {
            text.push('\\');
        }
        let mut path = PathBuf::from(&text);
        if path.is_relative() {
            path = self.cwd.join(path);
        }
        // Keep the lexical path: `canonicalize` would rewrite `C:\` into the
        // `\\?\` extended-length form, which the breadcrumbs and root row
        // must show as ordinary drive paths.
        if path.is_dir() {
            self.navigate_to(path);
            return Ok(PathTarget::Folder);
        }
        if path.is_file() {
            match self.mode {
                PickerMode::OpenAny if is_markdown_path(&path) => {
                    return Ok(PathTarget::File(path));
                }
                PickerMode::Folder => {
                    return Err(self.reject_path("That is a file; pick a folder instead."));
                }
                PickerMode::OpenAny => {
                    return Err(self.reject_path("That file type cannot be opened as a document."));
                }
            }
        }
        Err(self.reject_path(format!("No such folder or file: {}", path.display())))
    }

    fn reject_path(&mut self, message: impl Into<String>) -> String {
        let message = message.into();
        self.path_error = Some(message.clone());
        message
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(len);
        self.selected = next as usize;
    }

    pub fn home() -> Option<PathBuf> {
        dirs::home_dir()
    }

    /// Click targets from the volume root down to `cwd`.
    ///
    /// A Windows drive prefix is only the namespace marker: after pushing
    /// it and the root separator, the first crumb is the real `C:\` (a
    /// `C:` target would be drive-relative, i.e. the process's current
    /// directory on that drive, not its root).
    pub fn breadcrumbs(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        let mut acc = PathBuf::new();
        for c in self.cwd.components() {
            acc.push(c.as_os_str());
            match c {
                std::path::Component::Prefix(_) => continue,
                std::path::Component::RootDir => {
                    let label = if cfg!(windows) {
                        acc.as_os_str().to_string_lossy().into_owned()
                    } else {
                        "/".to_string()
                    };
                    out.push((label, acc.clone()));
                }
                std::path::Component::Normal(s) => {
                    out.push((s.to_string_lossy().into_owned(), acc.clone()));
                }
                _ => continue,
            }
        }
        out
    }
}

/// Every drive letter the system reports as present. `GetLogicalDrives`
/// answers with one bit per letter and never touches the media, unlike
/// probing `X:\`, which can wait on or prompt for a removable drive.
#[cfg(windows)]
fn drive_roots() -> Vec<PathBuf> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
    }
    // Bit 0 is A:, bit 25 is Z:.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| PathBuf::from(format!("{}:\\", (b'A' + bit as u8) as char)))
        .collect()
}

pub fn is_markdown_path(p: &Path) -> bool {
    let ext = p.extension().and_then(|s| s.to_str());
    // PDFs are viewable (extracted to markdown) only when the `pdf` feature is
    // compiled in, so only surface them in the explorer then.
    #[cfg(feature = "pdf")]
    if ext.is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        return true;
    }
    matches!(
        ext,
        Some("md")
            | Some("markdown")
            | Some("tex")
            | Some("json")
            | Some("yaml")
            | Some("yml")
            | Some("toml")
    )
}

/// Count supported files below `root` without allocating their paths.
///
/// The depth, hidden-file, and large-directory exclusions intentionally match
/// `tree::build`, so the chooser's count describes the files that would become
/// navigable after choosing the folder as a workspace.
pub fn count_supported_files(
    root: &Path,
    max_depth: usize,
    max_files: usize,
    max_entries: usize,
    show_hidden: bool,
) -> Result<SupportedFileCount, String> {
    let mut count = 0;
    let mut examined_entries = 0;
    let mut stack = vec![(root.to_path_buf(), 0usize)];

    while let Some((dir, depth)) = stack.pop() {
        if depth >= max_depth {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if depth == 0 => return Err(error.to_string()),
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if examined_entries >= max_entries {
                return Ok(SupportedFileCount {
                    count,
                    capped: true,
                });
            }
            examined_entries += 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            if matches!(name.as_str(), ".git" | "node_modules" | "target")
                || (!show_hidden && name.starts_with('.'))
            {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
            } else if is_markdown_path(&path) {
                count += 1;
                if count >= max_files {
                    return Ok(SupportedFileCount {
                        count,
                        capped: true,
                    });
                }
            }
        }
    }

    Ok(SupportedFileCount {
        count,
        capped: false,
    })
}

/// Walk a workspace folder gathering all markdown files (limited depth + count).
pub fn walk_markdown(
    root: &Path,
    max_depth: usize,
    max_files: usize,
    show_hidden: bool,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= max_files {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut sub = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".git" || name == "node_modules" || name == "target" {
                continue;
            }
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                if depth + 1 < max_depth {
                    sub.push((p, depth + 1));
                }
            } else if is_markdown_path(&p) {
                out.push(p);
                if out.len() >= max_files {
                    break;
                }
            }
        }
        sub.reverse();
        stack.extend(sub);
    }
    out.sort();
    out
}

pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    fuzzy_score_lowered(&query.to_lowercase(), candidate)
}

/// [`fuzzy_score`] with the query already lowercased, for scoring many
/// candidates against one query. ASCII candidates are lowercased on the fly
/// instead of into a fresh `String`.
pub fn fuzzy_score_lowered(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    if candidate.is_ascii() {
        let lowered = candidate.chars().map(|ch| ch.to_ascii_lowercase());
        score_lowered(query, lowered, candidate.len())
    } else {
        score_lowered(query, candidate.to_lowercase().chars(), candidate.len())
    }
}

/// Subsequence score of `query` over the lowercased candidate `chars`;
/// `candidate_len` is the original candidate's byte length.
fn score_lowered(
    query: &str,
    chars: impl Iterator<Item = char>,
    candidate_len: usize,
) -> Option<i32> {
    let mut q_iter = query.chars().peekable();
    let mut score = 0i32;
    let mut prev_match = -1i32;
    let mut prev_char = ' ';
    for (i, ch) in chars.enumerate() {
        if let Some(qc) = q_iter.peek() {
            if *qc == ch {
                let i = i as i32;
                score += 10;
                if prev_match >= 0 && i == prev_match + 1 {
                    score += 15; // contiguous bonus
                }
                if prev_char == '/'
                    || prev_char == '_'
                    || prev_char == '-'
                    || prev_char == ' '
                    || i == 0
                {
                    score += 8; // boundary bonus
                }
                prev_match = i;
                q_iter.next();
            }
        }
        prev_char = ch;
    }
    if q_iter.peek().is_some() {
        return None;
    }
    score -= candidate_len as i32 / 4;
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The previous allocate-both-sides scorer, kept as the oracle.
    fn fuzzy_score_reference(query: &str, candidate: &str) -> Option<i32> {
        if query.is_empty() {
            return Some(0);
        }
        let q = query.to_lowercase();
        let c = candidate.to_lowercase();
        let mut q_iter = q.chars().peekable();
        let mut score = 0i32;
        let mut prev_match = -1i32;
        let mut prev_char = ' ';
        for (i, ch) in c.chars().enumerate() {
            if let Some(qc) = q_iter.peek() {
                if *qc == ch {
                    let i = i as i32;
                    score += 10;
                    if prev_match >= 0 && i == prev_match + 1 {
                        score += 15;
                    }
                    if matches!(prev_char, '/' | '_' | '-' | ' ') || i == 0 {
                        score += 8;
                    }
                    prev_match = i;
                    q_iter.next();
                }
            }
            prev_char = ch;
        }
        if q_iter.peek().is_some() {
            return None;
        }
        score -= candidate.len() as i32 / 4;
        Some(score)
    }

    #[test]
    fn fuzzy_score_matches_the_allocating_reference() {
        let candidates = [
            "",
            "README.md",
            "docs/plans/active/MDV-002-search.md",
            "notes/Straße/ΟΔΟΣ.md",
            "İstanbul trip.md",
            "a_b-c d/e.MD",
            "日本語/メモ.md",
        ];
        let queries = [
            "", "r", "rdm", "DOCS", "plan act", "ss", "οδος", "i", "日メ", "zzz", "md",
        ];
        for candidate in candidates {
            for query in queries {
                assert_eq!(
                    fuzzy_score(query, candidate),
                    fuzzy_score_reference(query, candidate),
                    "query={query:?} candidate={candidate:?}"
                );
                assert_eq!(
                    fuzzy_score_lowered(&query.to_lowercase(), candidate),
                    fuzzy_score_reference(query, candidate),
                    "query={query:?} candidate={candidate:?}"
                );
            }
        }
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_is_listed_when_feature_on() {
        assert!(is_markdown_path(Path::new("doc.pdf")));
        assert!(is_markdown_path(Path::new("DOC.PDF"))); // case-insensitive
    }

    #[cfg(not(feature = "pdf"))]
    #[test]
    fn pdf_not_listed_when_feature_off() {
        assert!(!is_markdown_path(Path::new("doc.pdf")));
    }

    #[test]
    fn markdown_and_data_listed_but_binary_not() {
        assert!(is_markdown_path(Path::new("a.md")));
        assert!(is_markdown_path(Path::new("a.yaml")));
        assert!(!is_markdown_path(Path::new("a.bin")));
        assert!(!is_markdown_path(Path::new("a.png")));
    }

    #[test]
    fn supported_file_count_is_recursive_bounded_and_respects_filters() {
        let root = std::env::temp_dir().join(format!(
            "rmdv-picker-file-count-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = root.join("nested");
        let cap_dir = root.join("cap");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&cap_dir).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("readme.md"), "# Root\n").unwrap();
        std::fs::write(nested.join("data.json"), "{}\n").unwrap();
        std::fs::write(root.join(".hidden/secret.md"), "# Hidden\n").unwrap();
        std::fs::write(root.join("target/generated.md"), "# Generated\n").unwrap();
        std::fs::write(cap_dir.join("one.bin"), "").unwrap();
        std::fs::write(cap_dir.join("two.bin"), "").unwrap();

        assert_eq!(
            count_supported_files(
                &root,
                FOLDER_FILE_COUNT_MAX_DEPTH,
                10,
                FOLDER_FILE_COUNT_MAX_ENTRIES,
                false,
            )
            .unwrap(),
            SupportedFileCount {
                count: 2,
                capped: false
            }
        );
        assert_eq!(
            count_supported_files(
                &root,
                FOLDER_FILE_COUNT_MAX_DEPTH,
                1,
                FOLDER_FILE_COUNT_MAX_ENTRIES,
                false,
            )
            .unwrap(),
            SupportedFileCount {
                count: 1,
                capped: true
            }
        );
        assert_eq!(
            count_supported_files(&cap_dir, FOLDER_FILE_COUNT_MAX_DEPTH, 10, 1, false).unwrap(),
            SupportedFileCount {
                count: 0,
                capped: true,
            }
        );
        assert!(count_supported_files(
            &root.join("missing"),
            FOLDER_FILE_COUNT_MAX_DEPTH,
            10,
            FOLDER_FILE_COUNT_MAX_ENTRIES,
            false,
        )
        .is_err());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn showing_hidden_folders_is_additive_and_keeps_documents_first() {
        let root = std::env::temp_dir().join(format!(
            "rmdv-picker-hidden-union-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("Documents")).unwrap();
        std::fs::create_dir_all(root.join("Downloads")).unwrap();
        std::fs::create_dir_all(root.join(".cache")).unwrap();

        let mut picker = Picker::new(Some(root.clone()), PickerMode::Folder, true);
        let shown: Vec<&str> = picker
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(shown, vec!["Documents", "Downloads", ".cache"]);

        picker.show_hidden = false;
        picker.refresh();
        let hidden_off: Vec<&str> = picker
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(hidden_off, vec!["Documents", "Downloads"]);

        let _ = std::fs::remove_dir_all(root);
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rmdv-picker-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn roots_are_available_for_the_jump_row() {
        let roots = Picker::roots();
        assert!(!roots.is_empty(), "a root row needs at least one target");
        #[cfg(not(windows))]
        {
            assert_eq!(roots[0], PathBuf::from("/"));
            assert_eq!(Picker::root_label(&roots[0]), "/");
        }
    }

    #[test]
    fn path_field_navigates_absolute_relative_and_spaced_paths() {
        let root = temp_root("path-field");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::create_dir_all(root.join("two words")).unwrap();
        std::fs::write(root.join("doc.md"), "# Doc\n").unwrap();
        let mut picker = Picker::new(Some(root.clone()), PickerMode::OpenAny, false);

        let absolute = root.join("nested").display().to_string();
        assert_eq!(picker.navigate_path(&absolute).unwrap(), PathTarget::Folder);
        assert_eq!(picker.cwd, root.join("nested"));

        picker.navigate_to(root.clone());
        assert_eq!(
            picker.navigate_path("two words").unwrap(),
            PathTarget::Folder
        );
        assert_eq!(picker.cwd, root.join("two words"));

        picker.navigate_to(root.clone());
        let file = root.join("doc.md");
        assert_eq!(
            picker.navigate_path(&file.display().to_string()).unwrap(),
            PathTarget::File(file.clone())
        );
        // A file target leaves the listing where it was.
        assert_eq!(picker.cwd, root);

        let err = picker.navigate_path("no-such-folder").unwrap_err();
        assert!(!err.is_empty());
        assert!(picker.path_error.is_some());
        assert_eq!(picker.cwd, root, "a bad path must not move the listing");

        picker.navigate_path(&root.display().to_string()).unwrap();
        assert!(picker.path_error.is_none(), "success clears the hint");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn path_field_rejects_unsupported_files_in_folder_mode() {
        let root = temp_root("path-field-folder-mode");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("doc.md");
        std::fs::write(&file, "# Doc\n").unwrap();
        let mut picker = Picker::new(Some(root.clone()), PickerMode::Folder, false);
        assert!(picker.navigate_path(&file.display().to_string()).is_err());
        assert_eq!(picker.cwd, root);

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn breadcrumbs_start_at_the_drive_root_not_a_drive_relative_prefix() {
        let root = temp_root("crumbs-windows");
        std::fs::create_dir_all(&root).unwrap();
        let mut picker = Picker::new(Some(root.clone()), PickerMode::OpenAny, false);
        // The drive root of whichever volume the temp folder lives on.
        let drive_root = root.ancestors().last().unwrap().to_path_buf();
        let crumbs = picker.breadcrumbs();
        let first = crumbs.first().expect("a drive root crumb");
        assert_eq!(first.1, drive_root, "first crumb is the real root");
        assert_eq!(first.0, Picker::root_label(&drive_root));
        picker.cwd = drive_root.join("Users");
        let crumbs = picker.breadcrumbs();
        assert_eq!(crumbs[0].1, drive_root);
        assert_eq!(crumbs[1], ("Users".to_string(), drive_root.join("Users")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(not(windows))]
    #[test]
    fn breadcrumbs_start_at_a_clickable_root() {
        let root = temp_root("crumbs-unix");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let mut picker = Picker::new(Some(root.join("nested")), PickerMode::OpenAny, false);
        picker.cwd = root.join("nested");
        let crumbs = picker.breadcrumbs();
        assert_eq!(crumbs[0], ("/".to_string(), PathBuf::from("/")));
        assert_eq!(crumbs.last().unwrap().1, root.join("nested"));
        let _ = std::fs::remove_dir_all(root);
    }
}
