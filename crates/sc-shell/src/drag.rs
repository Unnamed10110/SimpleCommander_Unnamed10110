//! OLE drag-out: start a native shell drag with the selected files so they
//! can be dropped on Explorer, browsers, mail clients, etc.

use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Once;
use windows::core::{PCWSTR, Result as WinResult};
use windows::Win32::Foundation::{
    DRAGDROP_S_DROP, DV_E_FORMATETC, E_NOTIMPL, HWND, OLE_E_ADVISENOTSUPPORTED, POINT, RECT, S_OK,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{
    IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, DATADIR_GET,
    DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{
    DoDragDrop, OleInitialize, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::Shell::SHCreateStdEnumFmtEtc;
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow, GA_ROOT,
};
use windows::Win32::Foundation::HGLOBAL;
use windows_core::{implement, BOOL};

fn hwnd_from(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

fn resolve_hwnd(preferred: Option<isize>) -> isize {
    if let Some(h) = preferred.filter(|h| *h != 0) {
        return h;
    }
    unsafe { GetForegroundWindow().0 as isize }
}

fn root_hwnd(preferred: Option<isize>) -> HWND {
    let raw = resolve_hwnd(preferred);
    if raw == 0 {
        return HWND::default();
    }
    unsafe {
        let hwnd = hwnd_from(raw);
        let root = GetAncestor(hwnd, GA_ROOT);
        if root.0.is_null() {
            hwnd
        } else {
            root
        }
    }
}

/// OLE must be initialized on the UI thread before `DoDragDrop`.
pub fn ensure_ole() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let _ = OleInitialize(None);
    });
}

pub fn capture_mouse(hwnd: Option<isize>) {
    let h = root_hwnd(hwnd);
    if h.0.is_null() {
        return;
    }
    unsafe {
        let _ = SetCapture(h);
    }
}

pub fn release_mouse() {
    unsafe {
        let _ = ReleaseCapture();
    }
}

/// True when the OS cursor is outside the window's client area.
pub fn cursor_outside_window(hwnd: Option<isize>) -> bool {
    let h = root_hwnd(hwnd);
    if h.0.is_null() {
        return false;
    }
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return false;
        }
        let mut rc = RECT::default();
        if GetClientRect(h, &mut rc).is_err() {
            return false;
        }
        let mut tl = POINT {
            x: rc.left,
            y: rc.top,
        };
        let mut br = POINT {
            x: rc.right,
            y: rc.bottom,
        };
        let _ = ClientToScreen(h, &mut tl);
        let _ = ClientToScreen(h, &mut br);
        pt.x < tl.x || pt.x >= br.x || pt.y < tl.y || pt.y >= br.y
    }
}

#[repr(C)]
struct DropFiles {
    p_files: u32,
    pt: POINT,
    f_nc: i32,
    f_wide: i32,
}

fn preferred_drop_effect_format() -> u16 {
    let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
    unsafe { RegisterClipboardFormatW(PCWSTR::from_raw(name.as_ptr())) as u16 }
}

use windows::Win32::System::DataExchange::RegisterClipboardFormatW;

fn pack_hdrop(paths: &[PathBuf]) -> Option<Vec<u8>> {
    if paths.is_empty() {
        return None;
    }
    let mut list: Vec<u16> = Vec::new();
    for p in paths {
        list.extend(p.as_os_str().encode_wide());
        list.push(0);
    }
    list.push(0);
    let header = std::mem::size_of::<DropFiles>();
    let total = header + list.len() * 2;
    let mut buf = vec![0u8; total];
    unsafe {
        let df = buf.as_mut_ptr() as *mut DropFiles;
        (*df).p_files = header as u32;
        (*df).pt = POINT::default();
        (*df).f_nc = 0;
        (*df).f_wide = 1;
        std::ptr::copy_nonoverlapping(
            list.as_ptr() as *const u8,
            buf.as_mut_ptr().add(header),
            list.len() * 2,
        );
    }
    Some(buf)
}

fn hglobal_from_bytes(bytes: &[u8]) -> WinResult<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let ptr = GlobalLock(h) as *mut u8;
        if ptr.is_null() {
            return Err(windows::core::Error::from(E_NOTIMPL));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(h);
        Ok(h)
    }
}

fn medium_hglobal(h: HGLOBAL) -> STGMEDIUM {
    STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: h },
        pUnkForRelease: ManuallyDrop::new(None),
    }
}

fn format_etc(cf: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: cf,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn format_matches(fmt: &FORMATETC, cf: u16) -> bool {
    fmt.cfFormat == cf
        && (fmt.dwAspect == 0 || fmt.dwAspect == DVASPECT_CONTENT.0)
        && (fmt.tymed & TYMED_HGLOBAL.0 as u32) != 0
}

/// `IDataObject` that exposes `CF_HDROP` so Explorer and other apps accept the drag.
#[implement(IDataObject)]
struct FileDataObject {
    hdrop: Vec<u8>,
    effect: Vec<u8>,
    effect_cf: u16,
}

impl FileDataObject {
    fn from_paths(paths: &[PathBuf]) -> Option<IDataObject> {
        let hdrop = pack_hdrop(paths)?;
        let effect = DROPEFFECT_COPY.0.to_le_bytes().to_vec();
        let obj = Self {
            hdrop,
            effect,
            effect_cf: preferred_drop_effect_format(),
        };
        Some(obj.into())
    }
}

impl IDataObject_Impl for FileDataObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> WinResult<STGMEDIUM> {
        let fmt = unsafe { pformatetcin.as_ref() }.ok_or(windows::core::Error::from(E_NOTIMPL))?;
        let bytes = if format_matches(fmt, CF_HDROP.0) {
            &self.hdrop
        } else if format_matches(fmt, self.effect_cf) {
            &self.effect
        } else {
            return Err(windows::core::Error::from(DV_E_FORMATETC));
        };
        let h = hglobal_from_bytes(bytes)?;
        Ok(medium_hglobal(h))
    }

    fn GetDataHere(
        &self,
        _pformatetc: *const FORMATETC,
        _pmedium: *mut STGMEDIUM,
    ) -> WinResult<()> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> windows::core::HRESULT {
        let Some(fmt) = (unsafe { pformatetc.as_ref() }) else {
            return DV_E_FORMATETC;
        };
        if format_matches(fmt, CF_HDROP.0) || format_matches(fmt, self.effect_cf) {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }

    fn GetCanonicalFormatEtc(
        &self,
        pformatectin: *const FORMATETC,
        pformatetcout: *mut FORMATETC,
    ) -> windows::core::HRESULT {
        if pformatetcout.is_null() {
            return E_NOTIMPL;
        }
        unsafe {
            if let Some(src) = pformatectin.as_ref() {
                *pformatetcout = *src;
                (*pformatetcout).ptd = std::ptr::null_mut();
            }
        }
        S_OK
    }

    fn SetData(
        &self,
        _pformatetc: *const FORMATETC,
        _pmedium: *const STGMEDIUM,
        _frelease: BOOL,
    ) -> WinResult<()> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn EnumFormatEtc(&self, dwdirection: u32) -> WinResult<IEnumFORMATETC> {
        if dwdirection != DATADIR_GET.0 as u32 {
            return Err(windows::core::Error::from(E_NOTIMPL));
        }
        let fmts = [format_etc(CF_HDROP.0), format_etc(self.effect_cf)];
        unsafe { SHCreateStdEnumFmtEtc(&fmts) }
    }

    fn DAdvise(
        &self,
        _pformatetc: *const FORMATETC,
        _advf: u32,
        _padvsink: windows_core::Ref<IAdviseSink>,
    ) -> WinResult<u32> {
        Err(windows::core::Error::from(OLE_E_ADVISENOTSUPPORTED))
    }

    fn DUnadvise(&self, _dwconnection: u32) -> WinResult<()> {
        Err(windows::core::Error::from(OLE_E_ADVISENOTSUPPORTED))
    }

    fn EnumDAdvise(&self) -> WinResult<IEnumSTATDATA> {
        Err(windows::core::Error::from(OLE_E_ADVISENOTSUPPORTED))
    }
}

/// Start a blocking OLE drag with `paths`. Returns the effect the target
/// performed (Some(true) = moved, Some(false) = copied, None = cancelled).
pub fn start_drag(paths: &[PathBuf]) -> Option<bool> {
    ensure_ole();
    if paths.is_empty() {
        return None;
    }
    let data = FileDataObject::from_paths(paths)?;
    let drop_source = crate::dropsource::create_drop_source();
    unsafe {
        let mut effect = DROPEFFECT(0);
        let hr = DoDragDrop(
            &data,
            &drop_source,
            DROPEFFECT_COPY | DROPEFFECT_MOVE,
            &mut effect,
        );
        if hr == DRAGDROP_S_DROP {
            Some(effect.0 & DROPEFFECT_MOVE.0 != 0)
        } else {
            None
        }
    }
}
