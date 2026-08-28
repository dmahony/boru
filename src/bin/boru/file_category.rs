#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::if_same_then_else,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::redundant_guards,
    clippy::manual_let_else,
    clippy::vec_init_then_push,
    clippy::let_underscore_future,
    clippy::needless_update,
    clippy::unnecessary_unwrap,
    clippy::single_match,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::question_mark,
    clippy::unnecessary_sort_by,
    clippy::result_large_err,
    clippy::enum_variant_names,
    clippy::explicit_counter_loop,
    clippy::wrong_self_convention,
    missing_debug_implementations,
    unfulfilled_lint_expectations
)]
#![allow(dead_code)]

//! Stable internal file-category model for Boru's file-type presentation.
//!
//! A `FileCategory` is a coarse, presentation-only grouping of files and
//! folders.  It exists so every Boru surface (chat cards, file-sharing
//! dashboard, transfer history, notifications) can show a consistent label
//! and, later, a consistent icon for a given kind of file.
//!
//! The category is a **local** concern:
//! - it is never transmitted over the network,
//! - it never replaces precise MIME data in the transfer model,
//! - it does not change any message or file-transfer types.
//!
//! Mapping filenames / MIME types onto a category belongs to the central
//! resolver (PAPIRUS-05), which builds on top of this module.

/// The rendering category for a file or folder.
///
/// Deliberately coarse: the central resolver maps filenames, MIME types,
/// and directory state onto one of these categories, and the `FileTypeIcon`
/// component maps the category onto an icon and an accessible label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileCategory {
    /// A directory or shared folder.
    Folder,
    /// A generic office document (DOCX, ODT, RTF, ...).
    Document,
    /// A PDF document.
    Pdf,
    /// A spreadsheet (XLSX, ODS, CSV, TSV, ...).
    Spreadsheet,
    /// A presentation (PPTX, ODP, ...).
    Presentation,
    /// Plain text (TXT, LOG, ...).
    Text,
    /// Markdown source (MD).
    Markdown,
    /// Source code (RS, PY, JS, ...).
    SourceCode,
    /// A raster or vector image (PNG, JPEG, SVG, ...).
    Image,
    /// A video file (MP4, MKV, WEBM, ...).
    Video,
    /// An audio file (MP3, FLAC, WAV, ...).
    Audio,
    /// A compressed archive (ZIP, TAR, GZ, 7Z, ...).
    Archive,
    /// An executable or application binary.
    Executable,
    /// An installer or setup package.
    Installer,
    /// A disk image (ISO, IMG, ...).
    DiskImage,
    /// A database file (SQLite, ...).
    Database,
    /// A font file (TTF, OTF, WOFF, ...).
    Font,
    /// A security certificate (CRT, PEM, ...).
    Certificate,
    /// A cryptographic or signing key.
    Key,
    /// An ebook (EPUB, MOBI, ...).
    Ebook,
    /// A BitTorrent metafile (TORRENT).
    Torrent,
    /// A CAD drawing (DWG, DXF, ...).
    Cad,
    /// A 3D model (STL, OBJ, GLTF, ...).
    ThreeDimensional,
    /// The category could not be determined.
    Unknown,
}

// ── Category helpers ─────────────────────────────────────────────────────

impl FileCategory {
    /// All categories in declaration order.
    pub const ALL: &'static [FileCategory] = &[
        FileCategory::Folder,
        FileCategory::Document,
        FileCategory::Pdf,
        FileCategory::Spreadsheet,
        FileCategory::Presentation,
        FileCategory::Text,
        FileCategory::Markdown,
        FileCategory::SourceCode,
        FileCategory::Image,
        FileCategory::Video,
        FileCategory::Audio,
        FileCategory::Archive,
        FileCategory::Executable,
        FileCategory::Installer,
        FileCategory::DiskImage,
        FileCategory::Database,
        FileCategory::Font,
        FileCategory::Certificate,
        FileCategory::Key,
        FileCategory::Ebook,
        FileCategory::Torrent,
        FileCategory::Cad,
        FileCategory::ThreeDimensional,
        FileCategory::Unknown,
    ];

    /// Short human-readable label shown next to a file row or card.
    ///
    /// Examples: `"PDF document"`, `"Video"`, `"ZIP archive"`, `"Folder"`,
    /// `"Unknown file"`.
    pub fn display_label(self) -> &'static str {
        match self {
            FileCategory::Folder => "Folder",
            FileCategory::Document => "Document",
            FileCategory::Pdf => "PDF document",
            FileCategory::Spreadsheet => "Spreadsheet",
            FileCategory::Presentation => "Presentation",
            FileCategory::Text => "Text file",
            FileCategory::Markdown => "Markdown",
            FileCategory::SourceCode => "Source code",
            FileCategory::Image => "Image",
            FileCategory::Video => "Video",
            FileCategory::Audio => "Audio",
            FileCategory::Archive => "Archive",
            FileCategory::Executable => "Executable",
            FileCategory::Installer => "Installer",
            FileCategory::DiskImage => "Disk image",
            FileCategory::Database => "Database",
            FileCategory::Font => "Font",
            FileCategory::Certificate => "Certificate",
            FileCategory::Key => "Key",
            FileCategory::Ebook => "Ebook",
            FileCategory::Torrent => "Torrent",
            FileCategory::Cad => "CAD",
            FileCategory::ThreeDimensional => "3D model",
            FileCategory::Unknown => "Unknown file",
        }
    }

    /// Longer accessible description for screen readers and tooltips.
    ///
    /// Examples: `"PDF document"`, `"Video file"`, `"Compressed archive"`,
    /// `"Shared folder"`, `"Unknown file type"`.
    pub fn accessible_description(self) -> &'static str {
        match self {
            FileCategory::Folder => "Shared folder",
            FileCategory::Document => "Office document",
            FileCategory::Pdf => "Portable Document Format (PDF) document",
            FileCategory::Spreadsheet => "Spreadsheet document",
            FileCategory::Presentation => "Presentation document",
            FileCategory::Text => "Plain text file",
            FileCategory::Markdown => "Markdown document",
            FileCategory::SourceCode => "Source code file",
            FileCategory::Image => "Image file",
            FileCategory::Video => "Video file",
            FileCategory::Audio => "Audio file",
            FileCategory::Archive => "Compressed archive",
            FileCategory::Executable => "Executable file",
            FileCategory::Installer => "Installer package",
            FileCategory::DiskImage => "Disk image file",
            FileCategory::Database => "Database file",
            FileCategory::Font => "Font file",
            FileCategory::Certificate => "Security certificate",
            FileCategory::Key => "Encryption or signing key",
            FileCategory::Ebook => "Ebook file",
            FileCategory::Torrent => "BitTorrent metafile",
            FileCategory::Cad => "CAD drawing",
            FileCategory::ThreeDimensional => "3D model file",
            FileCategory::Unknown => "Unknown file type",
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::FileCategory;

    #[test]
    fn all_lists_every_variant_without_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for category in FileCategory::ALL {
            assert!(
                seen.insert(*category),
                "duplicate category in ALL: {category:?}"
            );
        }
        assert_eq!(FileCategory::ALL.len(), 24);
    }

    #[test]
    fn display_labels_match_expected_wording() {
        let expectations: &[(FileCategory, &str)] = &[
            (FileCategory::Folder, "Folder"),
            (FileCategory::Document, "Document"),
            (FileCategory::Pdf, "PDF document"),
            (FileCategory::Spreadsheet, "Spreadsheet"),
            (FileCategory::Presentation, "Presentation"),
            (FileCategory::Text, "Text file"),
            (FileCategory::Markdown, "Markdown"),
            (FileCategory::SourceCode, "Source code"),
            (FileCategory::Image, "Image"),
            (FileCategory::Video, "Video"),
            (FileCategory::Audio, "Audio"),
            (FileCategory::Archive, "Archive"),
            (FileCategory::Executable, "Executable"),
            (FileCategory::Installer, "Installer"),
            (FileCategory::DiskImage, "Disk image"),
            (FileCategory::Database, "Database"),
            (FileCategory::Font, "Font"),
            (FileCategory::Certificate, "Certificate"),
            (FileCategory::Key, "Key"),
            (FileCategory::Ebook, "Ebook"),
            (FileCategory::Torrent, "Torrent"),
            (FileCategory::Cad, "CAD"),
            (FileCategory::ThreeDimensional, "3D model"),
            (FileCategory::Unknown, "Unknown file"),
        ];
        for (category, expected) in expectations {
            assert_eq!(
                category.display_label(),
                *expected,
                "display_label mismatch for {category:?}"
            );
        }
    }

    #[test]
    fn accessible_descriptions_match_expected_wording() {
        let expectations: &[(FileCategory, &str)] = &[
            (FileCategory::Folder, "Shared folder"),
            (FileCategory::Document, "Office document"),
            (FileCategory::Pdf, "Portable Document Format (PDF) document"),
            (FileCategory::Spreadsheet, "Spreadsheet document"),
            (FileCategory::Presentation, "Presentation document"),
            (FileCategory::Text, "Plain text file"),
            (FileCategory::Markdown, "Markdown document"),
            (FileCategory::SourceCode, "Source code file"),
            (FileCategory::Image, "Image file"),
            (FileCategory::Video, "Video file"),
            (FileCategory::Audio, "Audio file"),
            (FileCategory::Archive, "Compressed archive"),
            (FileCategory::Executable, "Executable file"),
            (FileCategory::Installer, "Installer package"),
            (FileCategory::DiskImage, "Disk image file"),
            (FileCategory::Database, "Database file"),
            (FileCategory::Font, "Font file"),
            (FileCategory::Certificate, "Security certificate"),
            (FileCategory::Key, "Encryption or signing key"),
            (FileCategory::Ebook, "Ebook file"),
            (FileCategory::Torrent, "BitTorrent metafile"),
            (FileCategory::Cad, "CAD drawing"),
            (FileCategory::ThreeDimensional, "3D model file"),
            (FileCategory::Unknown, "Unknown file type"),
        ];
        for (category, expected) in expectations {
            assert_eq!(
                category.accessible_description(),
                *expected,
                "accessible_description mismatch for {category:?}"
            );
        }
    }

    #[test]
    fn every_category_has_nonempty_labels() {
        for category in FileCategory::ALL {
            assert!(!category.display_label().trim().is_empty());
            assert!(!category.accessible_description().trim().is_empty());
        }
    }

    #[test]
    fn labels_and_descriptions_are_distinct_fields() {
        // A description that merely repeats the short label adds no
        // accessible value; every variant should provide a fuller phrase.
        for category in FileCategory::ALL {
            assert_ne!(
                category.display_label(),
                category.accessible_description(),
                "label and description identical for {category:?}"
            );
        }
    }
}
