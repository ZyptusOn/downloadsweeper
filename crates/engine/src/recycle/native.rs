use super::Bin;
use anyhow::{ensure, Result};
use std::path::Path;
pub struct Native;
pub fn supported() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

#[cfg(windows)]
fn normalized(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .trim_end_matches('\\')
        .to_lowercase()
}
#[cfg(windows)]
fn item(stage: &Path) -> Result<Option<trash::TrashItem>> {
    let mut items = trash::os_limited::list()?
        .into_iter()
        .filter(|i| normalized(&i.original_path()) == normalized(stage));
    let found = items.next();
    ensure!(
        items.next().is_none(),
        "同一恢复标识存在多个回收站条目，拒绝猜测"
    );
    Ok(found)
}
#[cfg(windows)]
impl Bin for Native {
    fn check(&self, root: &Path) -> Result<()> {
        ensure!(
            !normalized(root).starts_with(r"unc\") && !normalized(root).starts_with(r"\\"),
            "网络目录不支持可靠回收，请使用本地磁盘"
        );
        // Verify that the system can enumerate recovery receipts before moving files.
        trash::os_limited::list()?;
        Ok(())
    }
    fn find(&self, stage: &Path) -> Result<Option<String>> {
        item(stage)?
            .map(|i| {
                i.id.into_string()
                    .map_err(|_| anyhow::anyhow!("回收站标识无法编码"))
            })
            .transpose()
    }
    fn put(&self, stage: &Path) -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            core::PCWSTR,
            Win32::{System::Com::*, UI::Shell::*},
        };
        // RECYCLEONDELETE fails when recycling is unavailable. No permanent-delete fallback.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            struct Com;
            impl Drop for Com {
                fn drop(&mut self) {
                    unsafe { CoUninitialize() }
                }
            }
            let _com = Com;
            let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
            operation.SetOperationFlags(
                FOF_NO_UI
                    | FOF_WANTNUKEWARNING
                    | FOFX_RECYCLEONDELETE
                    | FOFX_ADDUNDORECORD
                    | FOFX_EARLYFAILURE,
            )?;
            let value = stage.to_string_lossy();
            let wide: Vec<u16> = std::ffi::OsStr::new(value.trim_start_matches(r"\\?\"))
                .encode_wide()
                .chain(Some(0))
                .collect();
            let source: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)?;
            let sink: IFileOperationProgressSink = super::windows_guard::RecycleOnly.into();
            operation.DeleteItem(&source, &sink)?;
            operation.PerformOperations()?;
            ensure!(
                !operation.GetAnyOperationsAborted()?.as_bool(),
                "系统取消了回收；文件保留在恢复目录，可点击撤销"
            );
        }
        Ok(())
    }
    fn restore(&self, stage: &Path, receipt: &str) -> Result<()> {
        ensure!(!stage.try_exists()?, "恢复暂存目录已占用");
        let entry = item(stage)?.ok_or_else(|| anyhow::anyhow!("回收站条目已不存在"))?;
        ensure!(entry.id.to_str() == Some(receipt), "回收站条目标识不匹配");
        trash::os_limited::restore_all([entry])?;
        ensure!(stage.is_dir(), "系统未完成恢复，保留记录以便重试");
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn mac_path(path: &Path, moving: bool) -> Result<std::path::PathBuf> {
    use std::{
        ffi::{CStr, CString},
        os::unix::ffi::OsStrExt,
    };
    unsafe extern "C" {
        fn ds_recycle_macos(
            path: *const std::ffi::c_char,
            moving: i32,
            output: *mut std::ffi::c_char,
            capacity: usize,
        ) -> i32;
    }
    let input = CString::new(path.as_os_str().as_bytes())?;
    let mut output = vec![0i8; 32768];
    let result = unsafe {
        ds_recycle_macos(
            input.as_ptr(),
            moving as i32,
            output.as_mut_ptr(),
            output.len(),
        )
    };
    ensure!(
        result == 0,
        "macOS 回收站操作失败（{result}）；文件和恢复记录保留"
    );
    let bytes = unsafe { CStr::from_ptr(output.as_ptr()) }.to_bytes();
    Ok(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}
#[cfg(target_os = "macos")]
impl Bin for Native {
    fn check(&self, root: &Path) -> Result<()> {
        mac_path(root, false)?;
        Ok(())
    }
    fn find(&self, stage: &Path) -> Result<Option<String>> {
        let bin = mac_path(
            stage.parent().ok_or_else(|| anyhow::anyhow!("无父目录"))?,
            false,
        )?;
        let path = bin.join(
            stage
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("无恢复标识"))?,
        );
        ensure!(!crate::safe_fs::is_link(&path), "回收站条目不能是链接");
        if path.try_exists()? {
            Ok(Some(
                path.to_str()
                    .ok_or_else(|| anyhow::anyhow!("回收站路径无法编码"))?
                    .into(),
            ))
        } else {
            Ok(None)
        }
    }
    fn put(&self, stage: &Path) -> Result<()> {
        ensure!(self.find(stage)?.is_none(), "恢复标识已存在，停止回收");
        let actual = mac_path(stage, true)?;
        let expected = self
            .find(stage)?
            .ok_or_else(|| anyhow::anyhow!("无法确认系统回收位置"))?;
        ensure!(
            actual == std::path::PathBuf::from(expected),
            "系统回收路径发生变化，请保留记录并在废纸篓中恢复"
        );
        Ok(())
    }
    fn restore(&self, stage: &Path, receipt: &str) -> Result<()> {
        ensure!(
            self.find(stage)?.as_deref() == Some(receipt),
            "回收条目已不存在或位置已改变"
        );
        crate::safe_fs::move_noreplace(Path::new(receipt), stage)
    }
}
#[cfg(not(any(windows, target_os = "macos")))]
impl Bin for Native {
    fn check(&self, _: &Path) -> Result<()> {
        anyhow::bail!("当前平台尚不支持可撤销回收，未移动文件")
    }
    fn find(&self, _: &Path) -> Result<Option<String>> {
        anyhow::bail!("当前平台不支持回收站恢复")
    }
    fn put(&self, _: &Path) -> Result<()> {
        anyhow::bail!("当前平台不支持可撤销回收")
    }
    fn restore(&self, _: &Path, _: &str) -> Result<()> {
        anyhow::bail!("当前平台不支持回收站恢复")
    }
}
