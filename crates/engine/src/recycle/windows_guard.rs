//! Abort the shell operation if it proposes a non-recyclable deletion.
use windows::Win32::UI::Shell::*;
#[windows::core::implement(IFileOperationProgressSink)]
pub struct RecycleOnly;
#[test]
fn shell_callback_rejects_non_recyclable_delete() {
    let sink: IFileOperationProgressSink = RecycleOnly.into();
    unsafe {
        assert!(sink.PreDeleteItem(0, None).is_err());
        assert!(sink
            .PreDeleteItem(TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32, None)
            .is_ok());
    }
}
#[allow(non_snake_case, unused_variables)]
impl IFileOperationProgressSink_Impl for RecycleOnly_Impl {
    fn StartOperations(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, hrresult: windows_core::HRESULT) -> windows_core::Result<()> {
        hrresult.ok()
    }
    fn PreRenameItem(
        &self,
        dwflags: u32,
        psiitem: windows_core::Ref<'_, IShellItem>,
        psznewname: &windows_core::PCWSTR,
    ) -> windows_core::Result<()> {
        Err(windows::Win32::Foundation::E_ABORT.into())
    }
    fn PostRenameItem(
        &self,
        dwflags: u32,
        psiitem: windows_core::Ref<'_, IShellItem>,
        psznewname: &windows_core::PCWSTR,
        hrrename: windows_core::HRESULT,
        psinewlycreated: windows_core::Ref<'_, IShellItem>,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        dwflags: u32,
        psiitem: windows_core::Ref<'_, IShellItem>,
        psidestinationfolder: windows_core::Ref<'_, IShellItem>,
        psznewname: &windows_core::PCWSTR,
    ) -> windows_core::Result<()> {
        Err(windows::Win32::Foundation::E_ABORT.into())
    }
    fn PostMoveItem(
        &self,
        dwflags: u32,
        psiitem: windows_core::Ref<'_, IShellItem>,
        psidestinationfolder: windows_core::Ref<'_, IShellItem>,
        psznewname: &windows_core::PCWSTR,
        hrmove: windows_core::HRESULT,
        psinewlycreated: windows_core::Ref<'_, IShellItem>,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        dwflags: u32,
        psiitem: windows_core::Ref<'_, IShellItem>,
        psidestinationfolder: windows_core::Ref<'_, IShellItem>,
        psznewname: &windows_core::PCWSTR,
    ) -> windows_core::Result<()> {
        Err(windows::Win32::Foundation::E_ABORT.into())
    }
    fn PostCopyItem(
        &self,
        dwflags: u32,
        psiitem: windows_core::Ref<'_, IShellItem>,
        psidestinationfolder: windows_core::Ref<'_, IShellItem>,
        psznewname: &windows_core::PCWSTR,
        hrcopy: windows_core::HRESULT,
        psinewlycreated: windows_core::Ref<'_, IShellItem>,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreDeleteItem(
        &self,
        dwflags: u32,
        psiitem: windows_core::Ref<'_, IShellItem>,
    ) -> windows_core::Result<()> {
        if dwflags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
            Err(windows::Win32::Foundation::E_ABORT.into())
        } else {
            Ok(())
        }
    }
    fn PostDeleteItem(
        &self,
        dwflags: u32,
        psiitem: windows_core::Ref<'_, IShellItem>,
        hrdelete: windows_core::HRESULT,
        psinewlycreated: windows_core::Ref<'_, IShellItem>,
    ) -> windows_core::Result<()> {
        hrdelete.ok()
    }
    fn PreNewItem(
        &self,
        dwflags: u32,
        psidestinationfolder: windows_core::Ref<'_, IShellItem>,
        psznewname: &windows_core::PCWSTR,
    ) -> windows_core::Result<()> {
        Err(windows::Win32::Foundation::E_ABORT.into())
    }
    fn PostNewItem(
        &self,
        dwflags: u32,
        psidestinationfolder: windows_core::Ref<'_, IShellItem>,
        psznewname: &windows_core::PCWSTR,
        psztemplatename: &windows_core::PCWSTR,
        dwfileattributes: u32,
        hrnew: windows_core::HRESULT,
        psinewitem: windows_core::Ref<'_, IShellItem>,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, iworktotal: u32, iworksofar: u32) -> windows_core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
}
