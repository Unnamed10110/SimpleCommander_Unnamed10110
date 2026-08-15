//! Minimal IDropSource: standard left-button drag semantics with Escape to
//! cancel and default drag cursors.

use windows_core::{implement, Result, BOOL};
use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK,
};
use windows::Win32::System::Ole::{IDropSource, IDropSource_Impl, DROPEFFECT};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};

#[implement(IDropSource)]
pub struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(
        &self,
        fescapepressed: BOOL,
        grfkeystate: MODIFIERKEYS_FLAGS,
    ) -> windows::core::HRESULT {
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if grfkeystate & MK_LBUTTON == MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> windows::core::HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

pub fn create_drop_source() -> IDropSource {
    DropSource.into()
}

#[allow(dead_code)]
fn _assert_result_used() -> Result<()> {
    Ok(())
}
