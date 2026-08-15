//! OLE drag-out: start a native shell drag with the selected files so they
//! can be dropped on Explorer, browsers, mail clients, etc. Uses the
//! shell-provided data object (no custom COM classes needed).

use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
use windows::Win32::System::Ole::{DoDragDrop, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{SHCreateDataObject, SHParseDisplayName};

use std::os::windows::ffi::OsStrExt;

struct PidlGuard(*mut ITEMIDLIST);
impl Drop for PidlGuard {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
    }
}

/// Start a blocking OLE drag with `paths`. Returns the effect the target
/// performed (Some(true) = moved, Some(false) = copied, None = cancelled).
pub fn start_drag(paths: &[PathBuf]) -> Option<bool> {
    if paths.is_empty() {
        return None;
    }
    unsafe {
        let mut guards: Vec<PidlGuard> = Vec::new();
        let mut raw: Vec<*const ITEMIDLIST> = Vec::new();
        for p in paths {
            let w: Vec<u16> = p
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHParseDisplayName(PCWSTR::from_raw(w.as_ptr()), None, &mut pidl, 0, None).is_ok() {
                raw.push(pidl as *const _);
                guards.push(PidlGuard(pidl));
            }
        }
        if raw.is_empty() {
            return None;
        }
        let data_object: IDataObject = SHCreateDataObject(None, Some(&raw), None).ok()?;
        let drop_source = crate::dropsource::create_drop_source();
        let mut effect = DROPEFFECT(0);
        let hr = DoDragDrop(
            &data_object,
            &drop_source,
            DROPEFFECT_COPY | DROPEFFECT_MOVE,
            &mut effect,
        );
        // DRAGDROP_S_DROP = 0x00040100
        if hr.0 == 0x0004_0100 {
            Some(effect.0 & DROPEFFECT_MOVE.0 != 0)
        } else {
            None
        }
    }
}
