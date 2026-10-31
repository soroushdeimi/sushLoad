//! File category rules and destination-name helpers.

use crate::model::{Category, Settings};
use std::path::{Path, PathBuf};

/// Category from the file extension (case-insensitive); `Other` when unknown or none.
pub fn classify(file_name: &str) -> Category {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "m4v" | "flv" | "wmv" | "ts" | "3gp" => {
            Category::Video
        }
        "mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "opus" | "wma" => Category::Music,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "rtf" | "epub"
        | "odt" | "csv" | "md" => Category::Documents,
        "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "iso" => {
            Category::Archives
        }
        "dmg" | "pkg" | "app" | "exe" | "msi" | "deb" | "rpm" | "apk" | "ipa" => Category::Programs,
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "svg" | "bmp" | "tiff" => {
            Category::Images
        }
        _ => Category::Other,
    }
}

/// Default folder for a category: the user's per-category folder if set, else the category
/// sub-folder of `download_dir` (when sorting), else `download_dir`.
pub fn dir_for(settings: &Settings, category: Category) -> PathBuf {
    if let Some(dir) = settings
        .category_dirs
        .get(&category)
        .filter(|dir| !dir.as_os_str().is_empty())
    {
        return dir.clone();
    }
    if settings.sort_into_categories {
        settings.download_dir.join(category.label())
    } else {
        settings.download_dir.clone()
    }
}

/// Folder a file with this name is saved to under the given settings.
pub fn destination_dir(settings: &Settings, file_name: &str) -> PathBuf {
    dir_for(settings, classify(file_name))
}

/// True when `dir/name` (or its `.part`) is already there, i.e. `unique_path` would rename.
pub fn name_taken(dir: &Path, name: &str) -> bool {
    is_taken(dir, name)
}

/// `dir/name`, or `dir/name (1).ext`, `(2)`… if that path already exists (or is reserved by a `.part`).
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    if !is_taken(dir, file_name) {
        return dir.join(file_name);
    }

    let (stem, ext) = split_name(file_name);
    let mut n: u64 = 1;
    loop {
        let candidate = match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        if !is_taken(dir, &candidate) {
            return dir.join(candidate);
        }
        n += 1;
    }
}

fn is_taken(dir: &Path, name: &str) -> bool {
    let path = dir.join(name);
    if path.exists() {
        return true;
    }
    dir.join(format!("{name}.part")).exists()
}

fn split_name(name: &str) -> (&str, Option<&str>) {
    let path = Path::new(name);
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if !ext.is_empty() => {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
            (stem, Some(ext))
        }
        _ => (name, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Category;
    use std::fs::File;

    #[test]
    fn classifies_known_extensions_case_insensitively() {
        assert_eq!(classify("movie.MP4"), Category::Video);
        assert_eq!(classify("song.FlAc"), Category::Music);
        assert_eq!(classify("report.PDF"), Category::Documents);
        assert_eq!(classify("backup.tar.gz"), Category::Archives);
        assert_eq!(classify("installer.dmg"), Category::Programs);
        assert_eq!(classify("photo.JPeG"), Category::Images);
    }

    #[test]
    fn unknown_or_missing_extension_is_other() {
        assert_eq!(classify("README"), Category::Other);
        assert_eq!(classify("data.bin"), Category::Other);
        assert_eq!(classify(""), Category::Other);
        assert_eq!(classify("weird."), Category::Other);
    }

    #[test]
    fn destination_dir_honours_sort_setting() {
        let mut settings = Settings {
            download_dir: PathBuf::from("/tmp/Downloads"),
            sort_into_categories: true,
            ..Settings::default()
        };
        assert_eq!(
            destination_dir(&settings, "clip.mov"),
            PathBuf::from("/tmp/Downloads/Video")
        );

        settings.sort_into_categories = false;
        assert_eq!(
            destination_dir(&settings, "clip.mov"),
            PathBuf::from("/tmp/Downloads")
        );
    }

    #[test]
    fn category_dirs_override_the_default_folder() {
        let mut settings = Settings {
            download_dir: PathBuf::from("/tmp/Downloads"),
            sort_into_categories: true,
            ..Settings::default()
        };
        settings
            .category_dirs
            .insert(Category::Video, PathBuf::from("/Volumes/Media/Movies"));
        assert_eq!(
            destination_dir(&settings, "clip.mov"),
            PathBuf::from("/Volumes/Media/Movies")
        );
        assert_eq!(
            destination_dir(&settings, "a.zip"),
            PathBuf::from("/tmp/Downloads/Archives")
        );
        // The override also wins when category folders are off.
        settings.sort_into_categories = false;
        assert_eq!(
            destination_dir(&settings, "clip.mov"),
            PathBuf::from("/Volumes/Media/Movies")
        );
        assert_eq!(
            destination_dir(&settings, "a.zip"),
            PathBuf::from("/tmp/Downloads")
        );
    }

    #[test]
    fn unique_path_returns_plain_name_when_free() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(unique_path(dir.path(), "a.txt"), dir.path().join("a.txt"));
    }

    #[test]
    fn unique_path_counts_up_without_touching_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        File::create(dir.path().join("a.txt")).expect("create");
        File::create(dir.path().join("a (1).txt")).expect("create");
        assert_eq!(
            unique_path(dir.path(), "a.txt"),
            dir.path().join("a (2).txt")
        );
    }

    #[test]
    fn unique_path_treats_part_file_as_reserved() {
        let dir = tempfile::tempdir().expect("tempdir");
        File::create(dir.path().join("movie.mkv.part")).expect("create");
        assert_eq!(
            unique_path(dir.path(), "movie.mkv"),
            dir.path().join("movie (1).mkv")
        );
    }

    #[test]
    fn unique_path_handles_no_extension_and_multi_dot() {
        let dir = tempfile::tempdir().expect("tempdir");
        File::create(dir.path().join("README")).expect("create");
        assert_eq!(
            unique_path(dir.path(), "README"),
            dir.path().join("README (1)")
        );

        File::create(dir.path().join("archive.tar.gz")).expect("create");
        assert_eq!(
            unique_path(dir.path(), "archive.tar.gz"),
            dir.path().join("archive.tar (1).gz")
        );
    }
}
