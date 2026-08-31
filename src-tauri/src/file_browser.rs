use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::SystemTime;

/// Represents a file or directory entry in a directory listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub modified: u64,
}

/// List contents of a directory.
/// Returns directories first (sorted by name), then files (sorted by name).
/// Hidden files (starting with `.` or `$`) and system files are excluded.
#[tauri::command]
pub fn list_directory(path: String) -> Result<Vec<FileEntry>, String> {
    let dir = Path::new(&path);

    if !dir.exists() {
        return Err(format!("Path does not exist: {}", path));
    }
    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", path));
    }

    let read_dir = fs::read_dir(dir).map_err(|e| format!("Failed to read directory '{}': {}", path, e))?;

    let mut entries: Vec<FileEntry> = Vec::new();

    for entry in read_dir {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let file_name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files/folders and Windows system files
        if file_name.starts_with('.')
            || file_name.starts_with('$')
            || file_name.eq_ignore_ascii_case("System Volume Information")
            || file_name.eq_ignore_ascii_case("$RECYCLE.BIN")
        {
            continue;
        }

        let metadata = entry.metadata().map_err(|e| format!("Failed to get metadata: {}", e))?;
        let is_dir = metadata.is_dir();

        // Skip system/hidden attribute files on Windows
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::MetadataExt;
            let attrs = metadata.file_attributes();
            // FILE_ATTRIBUTE_HIDDEN = 2, FILE_ATTRIBUTE_SYSTEM = 4
            if attrs & 2 != 0 || attrs & 4 != 0 {
                continue;
            }
        }

        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        entries.push(FileEntry {
            name: file_name,
            path: entry.path().to_string_lossy().to_string(),
            is_dir,
            modified,
        });
    }

    // Sort: directories first, then files, both case-insensitive alphabetical
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(entries)
}

/// Information about a logical drive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveInfo {
    pub name: String,
    pub path: String,
}

/// Enumerate available drives on the system (C:\, D:\, etc.).
#[tauri::command]
pub fn get_drives() -> Result<Vec<DriveInfo>, String> {
    let mut drives: Vec<DriveInfo> = Vec::new();

    for letter in b'A'..=b'Z' {
        let drive_path = format!("{}:\\", letter as char);
        if Path::new(&drive_path).exists() {
            drives.push(DriveInfo {
                name: format!("本地磁盘 ({}:)", letter as char),
                path: drive_path,
            });
        }
    }

    Ok(drives)
}

/// A known user folder (Desktop, Documents, Downloads, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownFolder {
    pub name: String,
    pub path: String,
}

/// Return common known folders for the current user.
#[tauri::command]
pub fn get_known_folders() -> Result<Vec<KnownFolder>, String> {
    let profile = std::env::var("USERPROFILE")
        .map_err(|_| "USERPROFILE environment variable not set".to_string())?;

    let candidates: Vec<(&str, &str)> = vec![
        ("桌面", "Desktop"),
        ("文档", "Documents"),
        ("下载", "Downloads"),
        ("图片", "Pictures"),
        ("视频", "Videos"),
        ("音乐", "Music"),
    ];

    let mut folders: Vec<KnownFolder> = Vec::new();

    for (name, child) in candidates {
        let full = format!("{}\\{}", profile, child);
        if Path::new(&full).is_dir() {
            folders.push(KnownFolder {
                name: name.to_string(),
                path: full,
            });
        }
    }

    // Always include the user profile root
    if Path::new(&profile).is_dir() {
        folders.push(KnownFolder {
            name: "用户目录".to_string(),
            path: profile,
        });
    }

    Ok(folders)
}

#[cfg(test)]
mod file_browser_tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "flowio_fb_test_{}_{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn list_directory_nonexistent_path_errors() {
        let p = std::env::temp_dir().join("flowio_fb_test_no_such_dir_xyz");
        let r = list_directory(p.to_string_lossy().to_string());
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("Path does not exist"));
    }

    #[test]
    fn list_directory_file_path_errors() {
        let d = temp_dir("filepath");
        let f = d.join("a.txt");
        fs::write(&f, "x").unwrap();
        let r = list_directory(f.to_string_lossy().to_string());
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("Not a directory"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn list_directory_sorts_dirs_first_case_insensitive() {
        let d = temp_dir("sort");
        fs::write(d.join("b.txt"), "").unwrap();
        fs::write(d.join("A.txt"), "").unwrap();
        fs::create_dir(d.join("Zdir")).unwrap();
        fs::create_dir(d.join("adir")).unwrap();
        let entries = list_directory(d.to_string_lossy().to_string()).unwrap();
        // 目录在前（大小写不敏感排序）：adir, Zdir；文件在后：A.txt, b.txt
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].name, "adir");
        assert_eq!(entries[1].name, "Zdir");
        assert_eq!(entries[2].name, "A.txt");
        assert_eq!(entries[3].name, "b.txt");
        assert!(entries[0].is_dir);
        assert!(!entries[2].is_dir);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn list_directory_filters_hidden_dot_and_dollar() {
        let d = temp_dir("hidden");
        fs::write(d.join("visible.txt"), "").unwrap();
        fs::write(d.join(".hidden.txt"), "").unwrap();
        fs::write(d.join("$SystemFile"), "").unwrap();
        let entries = list_directory(d.to_string_lossy().to_string()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "visible.txt");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn list_directory_empty_dir_returns_empty() {
        let d = temp_dir("empty");
        let entries = list_directory(d.to_string_lossy().to_string()).unwrap();
        assert!(entries.is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn list_directory_excludes_special_folder_names() {
        let d = temp_dir("special");
        fs::create_dir(d.join("System Volume Information")).unwrap();
        fs::create_dir(d.join("$RECYCLE.BIN")).unwrap();
        fs::write(d.join("keep.txt"), "").unwrap();
        let entries = list_directory(d.to_string_lossy().to_string()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "keep.txt");
        let _ = fs::remove_dir_all(&d);
    }
}
