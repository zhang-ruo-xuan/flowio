use super::super::utils::str_to_vec_u16;
pub(crate) use super::com::Result;
use super::com::{
    read_to_string, wrap_err, IFileDialog, IFileDialogV, IFileOpenDialog, IShellItem,
    COMDLG_FILTERSPEC, FILEOPENDIALOGOPTIONS, HWND,
};
use crate::FileDialog;

use windows_sys::{
    core::GUID,
    Win32::{
        System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER},
        UI::Shell::{
            FileOpenDialog, FileSaveDialog, SHCreateItemFromParsingName, SHGetKnownFolderPath,
            FOS_ALLOWMULTISELECT, FOS_PICKFOLDERS,
        },
    },
};

#[link(name = "shell32")]
extern "system" {
    /// Documented: https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-shgetknownfolderidlist
    fn SHGetKnownFolderIDList(
        rfid: *const GUID,
        dwflags: u32,
        htoken: *mut std::ffi::c_void,
        ppidl: *mut *mut std::ffi::c_void,
    ) -> i32;

    /// Documented: https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-shcreateitemfromidlist
    fn SHCreateItemFromIDList(
        pidl: *const std::ffi::c_void,
        riid: *const GUID,
        ppv: *mut *mut std::ffi::c_void,
    ) -> i32;
}

use std::{ffi::c_void, path::PathBuf};

/// Try to resolve a file system path to a Known Folder IShellItem.
///
/// Windows IFileDialog sidebar tree only highlights items with known-folder
/// identity (FOLDERID). Path-based IShellItems from SHCreateItemFromParsingName
/// lack this identity, so the sidebar won't highlight them.
///
/// This function uses two documented APIs:
/// 1. SHGetKnownFolderIDList — gets the PIDL for a known folder
/// 2. SHCreateItemFromIDList — creates an IShellItem from the PIDL
/// The resulting IShellItem carries the known-folder identity and should
/// get proper sidebar highlighting.
unsafe fn try_get_known_folder_item(path: &str) -> Option<IShellItem> {
    const SHELL_ITEM_IID: GUID = GUID::from_u128(0x43826d1e_e718_42ee_bc55_a1e261c37bfe);
    const KF_FLAG_DEFAULT: u32 = 0;

    #[rustfmt::skip]
    let known_folders: &[(GUID, &str)] = &[
        (GUID::from_u128(0xb4bfcc3a_db2c_424c_b029_7fe99a87c641), "Desktop"),
        (GUID::from_u128(0xfdd39ad0_238f_46af_adb4_6c85480369c7), "Documents"),
        (GUID::from_u128(0x374de290_123f_4565_9164_39c4925e467b), "Downloads"),
        (GUID::from_u128(0x33e28130_4e1e_4676_835a_98395c3bc3bb), "Pictures"),
        (GUID::from_u128(0x4bd8d571_6d19_48d3_be97_422220080e43), "Music"),
        (GUID::from_u128(0x18989b1d_99b5_455b_841c_ab7c74e4ddfd), "Videos"),
    ];

    let path_trimmed = path.trim_end_matches('\\');
    let path_lower = path_trimmed.to_lowercase();

    for (guid, _name) in known_folders {
        let mut known_path_ptr: *mut u16 = std::ptr::null_mut();
        if SHGetKnownFolderPath(guid, 0, std::ptr::null_mut(), &mut known_path_ptr) < 0 {
            continue;
        }

        let known_path_raw = read_to_string(known_path_ptr);
        CoTaskMemFree(known_path_ptr as *mut c_void);
        let known_path_trimmed = known_path_raw.trim_end_matches('\\');

        if known_path_trimmed.to_lowercase() == path_lower {
            // Step 1: get the PIDL for this known folder (documented API)
            let mut pidl: *mut c_void = std::ptr::null_mut();
            if SHGetKnownFolderIDList(
                guid,
                KF_FLAG_DEFAULT,
                std::ptr::null_mut(),
                &mut pidl,
            ) < 0
            {
                continue;
            }

            // Step 2: create an IShellItem from the PIDL (documented API)
            let mut item_ptr: *mut c_void = std::ptr::null_mut();
            let hr = SHCreateItemFromIDList(pidl, &SHELL_ITEM_IID, &mut item_ptr);

            // Free the PIDL regardless of outcome
            CoTaskMemFree(pidl);

            if hr >= 0 {
                return Some(IShellItem(item_ptr.cast()));
            }
        }
    }

    None
}

use raw_window_handle::RawWindowHandle;

enum DialogInner {
    Open(IFileOpenDialog),
    Save(IFileDialog),
}

impl DialogInner {
    unsafe fn new(open: bool) -> Result<Self> {
        const FILE_OPEN_DIALOG_IID: GUID = GUID::from_u128(0xd57c7288_d4ad_4768_be02_9d969532d960);
        const FILE_SAVE_DIALOG_IID: GUID = GUID::from_u128(0x84bccd23_5fde_4cdb_aea4_af64b83d78ab);

        unsafe {
            let (cls_id, iid) = if open {
                (&FileOpenDialog, &FILE_OPEN_DIALOG_IID)
            } else {
                (&FileSaveDialog, &FILE_SAVE_DIALOG_IID)
            };

            let mut iptr = std::mem::MaybeUninit::uninit();
            wrap_err(CoCreateInstance(
                cls_id,
                std::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                iid,
                iptr.as_mut_ptr(),
            ))?;

            let iptr = iptr.assume_init();

            Ok(if open {
                Self::Open(IFileOpenDialog(iptr.cast()))
            } else {
                Self::Save(IFileDialog(iptr.cast()))
            })
        }
    }

    #[inline]
    unsafe fn open() -> Result<Self> {
        unsafe { Self::new(true) }
    }

    #[inline]
    unsafe fn save() -> Result<Self> {
        unsafe { Self::new(false) }
    }

    #[inline]
    unsafe fn fd(&self) -> (*mut c_void, &IFileDialogV) {
        match self {
            Self::Save(s) => unsafe { (s.0.cast(), (*s.0).vtbl()) },
            Self::Open(o) => unsafe { (o.0.cast(), &(*o.0).vtbl().base) },
        }
    }

    #[inline]
    unsafe fn set_options(&self, opts: FILEOPENDIALOGOPTIONS) -> Result<()> {
        let (d, v) = self.fd();
        wrap_err((v.SetOptions)(d, opts))
    }

    #[inline]
    unsafe fn set_title(&self, title: &[u16]) -> Result<()> {
        let (d, v) = self.fd();
        wrap_err((v.SetTitle)(d, title.as_ptr()))
    }

    #[inline]
    unsafe fn set_default_extension(&self, extension: &[u16]) -> Result<()> {
        let (d, v) = self.fd();
        wrap_err((v.SetDefaultExtension)(d, extension.as_ptr()))
    }

    #[inline]
    unsafe fn clear_client_data(&self) -> Result<()> {
        let (d, v) = self.fd();
        wrap_err((v.ClearClientData)(d))
    }

    #[inline]
    unsafe fn set_client_guid(&self, guid: &GUID) -> Result<()> {
        let (d, v) = self.fd();
        wrap_err((v.SetClientGuid)(d, guid))
    }

    #[inline]
    unsafe fn set_file_types(&self, specs: &[COMDLG_FILTERSPEC]) -> Result<()> {
        let (d, v) = self.fd();
        wrap_err((v.SetFileTypes)(d, specs.len() as _, specs.as_ptr()))
    }

    #[inline]
    unsafe fn set_filename(&self, fname: &[u16]) -> Result<()> {
        let (d, v) = self.fd();
        wrap_err((v.SetFileName)(d, fname.as_ptr()))
    }

    #[inline]
    unsafe fn set_folder(&self, folder: &IShellItem) -> Result<()> {
        let (d, v) = self.fd();
        wrap_err((v.SetFolder)(d, folder.0.cast()))
    }

    #[inline]
    unsafe fn set_default_folder(&self, folder: &IShellItem) -> Result<()> {
        let (d, v) = self.fd();
        wrap_err((v.SetDefaultFolder)(d, folder.0.cast()))
    }

    #[inline]
    unsafe fn set_save_as_item(&self, folder: &IShellItem) -> Result<()> {
        let (d, v) = self.fd();
        // SetSaveAsItem is an IFileSaveDialog method; on Open dialog it's a no-op
        wrap_err((v.SetSaveAsItem)(d, folder.0.cast()))
    }

    #[inline]
    unsafe fn show(&self, parent: Option<HWND>) -> Result<()> {
        let (d, v) = self.fd();
        wrap_err((v.base.Show)(d, parent.unwrap_or(std::ptr::null_mut())))
    }

    #[inline]
    unsafe fn get_result(&self) -> Result<PathBuf> {
        let (d, v) = self.fd();
        let mut res = std::mem::MaybeUninit::uninit();
        wrap_err((v.GetResult)(d, res.as_mut_ptr()))?;
        let res = res.assume_init();
        res.get_path()
    }

    #[inline]
    unsafe fn get_results(&self) -> Result<Vec<PathBuf>> {
        let Self::Open(od) = self else { unreachable!() };

        let items = od.get_results()?;
        let count = items.get_count()?;

        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            let item = items.get_item_at(index)?;

            let path = item.get_path()?;
            paths.push(path);
        }

        Ok(paths)
    }
}

pub struct IDialog(DialogInner, Option<HWND>, bool /* is_save */);

impl IDialog {
    fn new_open_dialog(opt: &FileDialog) -> Result<Self> {
        let dialog = unsafe { DialogInner::open()? };

        let parent = match opt.parent {
            Some(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get() as _),
            None => None,
            _ => unreachable!("unsupported window handle, expected: Windows"),
        };

        Ok(Self(dialog, parent, false))
    }

    fn new_save_dialog(opt: &FileDialog) -> Result<Self> {
        let dialog = unsafe { DialogInner::save()? };

        let parent = match opt.parent {
            Some(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get() as _),
            None => None,
            _ => unreachable!("unsupported window handle, expected: Windows"),
        };

        Ok(Self(dialog, parent, true))
    }

    fn is_save(&self) -> bool {
        self.2
    }

    fn add_filters(&self, filters: &[crate::file_dialog::Filter]) -> Result<()> {
        if let Some(first_filter) = filters.first() {
            if let Some(first_extension) = first_filter.extensions.first() {
                let extension = str_to_vec_u16(first_extension);
                unsafe { self.0.set_default_extension(&extension)? }
            }
        }

        let mut f_list = {
            let mut f_list = Vec::new();
            let mut ext_string = String::new();

            for f in filters.iter() {
                let name = str_to_vec_u16(&f.name);
                ext_string.clear();

                for ext in &f.extensions {
                    use std::fmt::Write;
                    // This is infallible for String (barring OOM)
                    let _ = write!(&mut ext_string, "*.{ext};");
                }

                // pop trailing ;
                ext_string.pop();

                f_list.push((name, str_to_vec_u16(&ext_string)));
            }
            f_list
        };

        if f_list.is_empty() {
            f_list.push((str_to_vec_u16("All Files"), str_to_vec_u16("*.*")));
        }

        let spec: Vec<_> = f_list
            .iter()
            .map(|(name, ext)| COMDLG_FILTERSPEC {
                pszName: name.as_ptr(),
                pszSpec: ext.as_ptr(),
            })
            .collect();

        unsafe {
            self.0.set_file_types(&spec)?;
        }
        Ok(())
    }

    fn set_path(&self, path: &Option<PathBuf>) -> Result<()> {
        const SHELL_ITEM_IID: GUID = GUID::from_u128(0x43826d1e_e718_42ee_bc55_a1e261c37bfe);

        let Some(path_str) = path.as_ref().and_then(|p| p.to_str()) else {
            return Ok(());
        };

        // Strip Win32 namespace prefix from the path
        let path_str = path_str.strip_prefix(r"\\?\").unwrap_or(path_str);

        let wide_path = str_to_vec_u16(path_str);

        unsafe {
            let mut item = std::mem::MaybeUninit::uninit();
            if wrap_err(SHCreateItemFromParsingName(
                wide_path.as_ptr(),
                std::ptr::null_mut(),
                &SHELL_ITEM_IID,
                item.as_mut_ptr(),
            ))
            .is_ok()
            {
                let item = IShellItem(item.assume_init().cast());
                if self.is_save() {
                    // SetSaveAsItem navigates AND highlights in sidebar (mutually exclusive with SetFolder)
                    let _ = self.0.set_save_as_item(&item);
                } else {
                    let _ = self.0.set_folder(&item);
                }
            }
        }

        Ok(())
    }

    fn set_file_name(&self, file_name: &Option<String>) -> Result<()> {
        if let Some(path) = file_name {
            let wide_path = str_to_vec_u16(path);

            unsafe {
                self.0.set_filename(&wide_path)?;
            }
        }
        Ok(())
    }

    fn set_title(&self, title: &Option<String>) -> Result<()> {
        if let Some(title) = title {
            let wide_title = str_to_vec_u16(title);

            unsafe {
                self.0.set_title(&wide_title)?;
            }
        }
        Ok(())
    }

    pub fn get_results(&self) -> Result<Vec<PathBuf>> {
        unsafe { self.0.get_results() }
    }

    pub fn get_result(&self) -> Result<PathBuf> {
        unsafe { self.0.get_result() }
    }

    pub fn show(&self) -> Result<()> {
        unsafe { self.0.show(self.1) }
    }
}

impl IDialog {
    pub fn build_pick_file(opt: &FileDialog) -> Result<Self> {
        let dialog = IDialog::new_open_dialog(opt)?;

        dialog.add_filters(&opt.filters)?;
        dialog.set_path(&opt.starting_directory)?;
        dialog.set_file_name(&opt.file_name)?;
        dialog.set_title(&opt.title)?;

        Ok(dialog)
    }

    pub fn build_save_file(opt: &FileDialog) -> Result<Self> {
        let dialog = IDialog::new_save_dialog(opt)?;

        dialog.add_filters(&opt.filters)?;
        dialog.set_path(&opt.starting_directory)?;
        dialog.set_file_name(&opt.file_name)?;
        dialog.set_title(&opt.title)?;

        Ok(dialog)
    }

    pub fn build_pick_folder(opt: &FileDialog) -> Result<Self> {
        let dialog = IDialog::new_open_dialog(opt)?;

        dialog.set_path(&opt.starting_directory)?;
        dialog.set_title(&opt.title)?;

        unsafe {
            dialog.0.set_options(FOS_PICKFOLDERS)?;
        }

        Ok(dialog)
    }

    pub fn build_pick_folders(opt: &FileDialog) -> Result<Self> {
        let dialog = IDialog::new_open_dialog(opt)?;

        dialog.set_path(&opt.starting_directory)?;
        dialog.set_title(&opt.title)?;
        let opts = FOS_PICKFOLDERS | FOS_ALLOWMULTISELECT;

        unsafe {
            dialog.0.set_options(opts)?;
        }

        Ok(dialog)
    }

    pub fn build_pick_files(opt: &FileDialog) -> Result<Self> {
        let dialog = IDialog::new_open_dialog(opt)?;

        dialog.add_filters(&opt.filters)?;
        dialog.set_path(&opt.starting_directory)?;
        dialog.set_file_name(&opt.file_name)?;
        dialog.set_title(&opt.title)?;

        unsafe {
            dialog.0.set_options(FOS_ALLOWMULTISELECT)?;
        }

        Ok(dialog)
    }
}
