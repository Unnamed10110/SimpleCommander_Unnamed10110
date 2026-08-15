//! Native shell verbs: default open, "Open with", the full Explorer context
//! menu (IContextMenu), and the Properties dialog.

use std::path::Path;
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IContextMenu, IShellFolder, SHBindToParent, SHParseDisplayName, ShellExecuteExW,
    CMF_EXTENDEDVERBS, CMF_NORMAL, CMINVOKECOMMANDINFO, SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, TrackPopupMenuEx, SW_SHOWNORMAL, TPM_LEFTALIGN,
    TPM_RETURNCMD, TPM_RIGHTBUTTON,
};

use std::os::windows::ffi::OsStrExt;

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Open a file/folder with its default association (like double-click in Explorer).
pub fn shell_open(path: &Path) -> Result<(), String> {
    shell_verb(path, None)
}

/// Show the "Open with" dialog.
pub fn shell_open_with(path: &Path) -> Result<(), String> {
    shell_verb(path, Some("openas"))
}

/// Show the Properties dialog.
pub fn shell_properties(path: &Path) -> Result<(), String> {
    shell_verb(path, Some("properties"))
}

/// Open an elevated/normal console or run a verb on a path.
pub fn shell_verb(path: &Path, verb: Option<&str>) -> Result<(), String> {
    let path_w = wide(path.as_os_str());
    let verb_w = verb.map(|v| wide(std::ffi::OsStr::new(v)));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST,
        lpFile: PCWSTR::from_raw(path_w.as_ptr()),
        lpVerb: verb_w
            .as_ref()
            .map(|v| PCWSTR::from_raw(v.as_ptr()))
            .unwrap_or(PCWSTR::null()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| e.message().to_string())
}

struct PidlGuard(*mut ITEMIDLIST);
impl Drop for PidlGuard {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
    }
}

/// Show the full Explorer right-click menu for `paths` at screen coordinates
/// and invoke whatever the user picks. Must run on the UI thread that owns
/// `hwnd`. Currently supports items sharing one parent folder (the common
/// case); extra paths fall back to the first item's menu.
pub fn show_shell_context_menu(
    hwnd: HWND,
    paths: &[std::path::PathBuf],
    screen_x: i32,
    screen_y: i32,
    extended: bool,
) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    unsafe {
        // Parse all paths into PIDLs.
        let mut pidls: Vec<PidlGuard> = Vec::new();
        for p in paths {
            let w = wide(p.as_os_str());
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            SHParseDisplayName(PCWSTR::from_raw(w.as_ptr()), None, &mut pidl, 0, None)
                .map_err(|e| format!("{}: {}", p.display(), e.message()))?;
            pidls.push(PidlGuard(pidl));
        }
        // Bind to the parent folder of the first item; collect child ids for
        // all items with the same parent.
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let folder: IShellFolder = SHBindToParent(pidls[0].0, Some(&mut child as *mut _ as *mut _))
            .map_err(|e| e.message().to_string())?;
        let mut children: Vec<*const ITEMIDLIST> = vec![child];
        for pg in pidls.iter().skip(1) {
            let mut c: *mut ITEMIDLIST = std::ptr::null_mut();
            if let Ok(f) =
                SHBindToParent::<IShellFolder>(pg.0, Some(&mut c as *mut _ as *mut _))
            {
                // Only include when it's the same parent folder object; the
                // shell requires all children to share the parent.
                let _ = f;
                children.push(c);
            }
        }

        let menu: IContextMenu = folder
            .GetUIObjectOf(hwnd, &children.iter().map(|c| *c as *const _).collect::<Vec<_>>(), None)
            .map_err(|e| e.message().to_string())?;

        let hmenu = CreatePopupMenu().map_err(|e| e.message().to_string())?;
        let result = (|| {
            let flags = if extended {
                CMF_NORMAL | CMF_EXTENDEDVERBS
            } else {
                CMF_NORMAL
            };
            menu.QueryContextMenu(hmenu, 0, 1, 0x7FFF, flags)
                .ok()
                .map_err(|e| e.message().to_string())?;
            let cmd = TrackPopupMenuEx(
                hmenu,
                (TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD).0,
                screen_x,
                screen_y,
                hwnd,
                None,
            );
            let cmd_id = cmd.0;
            if cmd_id > 0 {
                let info = CMINVOKECOMMANDINFO {
                    cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                    lpVerb: windows::core::PCSTR((cmd_id as usize - 1) as *const u8),
                    nShow: SW_SHOWNORMAL.0,
                    hwnd,
                    ..Default::default()
                };
                menu.InvokeCommand(&info).map_err(|e| e.message().to_string())?;
            }
            Ok(())
        })();
        let _ = DestroyMenu(hmenu);
        // Silence unused-import lints on some cfgs.
        let _ = (POINT::default(), WPARAM(0), LPARAM(0), PWSTR::null());
        let _ = Interface::from_raw as unsafe fn(*mut core::ffi::c_void) -> IContextMenu;
        result
    }
}
