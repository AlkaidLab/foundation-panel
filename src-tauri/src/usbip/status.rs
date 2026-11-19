//! Transport status probing.

use super::{REQUIRED_VERSION, UsbipStatus};

/// Extracts the structured "USBIP-XXX-NNN" code from a detail string.
/// Returns an empty string when the prefix is not a well-shaped code, so
/// consumers can branch on `error_code` instead of parsing detail text.
pub(super) fn detail_error_code(detail: &str) -> String {
    let code = detail.split(':').next().unwrap_or_default().trim();
    let mut segments = code.split('-');
    let shaped = matches!(
        (segments.next(), segments.next(), segments.next(), segments.next()),
        (Some(area), Some(kind), Some(number), None)
            if area == "USBIP"
                && (3..=8).contains(&kind.len())
                && kind.bytes().all(|b| b.is_ascii_uppercase())
                && number.len() == 3
                && number.bytes().all(|b| b.is_ascii_digit())
    );
    if shaped {
        code.to_string()
    } else {
        String::new()
    }
}

/// The status answer is shared for a moment: the USB page, the DualSense status and the component cards ask for it
/// together, and one probe answers all of them. Anything that changes what it reports (`fresh()`) drops it.
static STATUS: crate::probe_cache::ProbeCache<UsbipStatus> =
    crate::probe_cache::ProbeCache::new("usbip status", std::time::Duration::from_secs(3));

/// A command that changes the installation or the attached devices: the shared answers are dropped when it starts
/// and again when it ends, however it ends, so a read in between cannot keep the old state.
pub(super) struct Fresh;

fn drop_shared_answers() {
    STATUS.invalidate();
    super::install::forget_replaceable();
    super::signature::forget();
}

pub(super) fn fresh() -> Fresh {
    drop_shared_answers();
    Fresh
}

impl Drop for Fresh {
    fn drop(&mut self) {
        drop_shared_answers();
    }
}

/// Spawns a `usbip.exe port` subprocess to enumerate attached devices when no recent answer exists. The two
/// PowerShell-backed parts (the driver signature, the host helper's `replaceable`) are kept while their inputs
/// are unchanged (memo.rs).
#[tauri::command]
pub async fn usbip_get_status() -> Result<UsbipStatus, String> {
    STATUS.get(probe_status).await
}

async fn probe_status() -> Result<UsbipStatus, String> {
    #[cfg(not(target_os = "windows"))]
    return Ok(UsbipStatus {
        externally_managed: false,
        replaceable: false,
        supported: false,
        installed: false,
        ready: false,
        version: String::new(),
        version_valid: false,
        reboot_recommended: false,
        vhci_residual: false,
        error_code: String::new(),
        attached_devices: Vec::new(),
        detail: "USB/IP passthrough is only supported on Windows".to_string(),
    });

    #[cfg(target_os = "windows")]
    {
        // Fail closed: if the residual state cannot be determined, do not
        // report a clean transport. Propagating the error surfaces the probe
        // as unavailable, hiding install and attach actions.
        let externally_managed = super::manager::externally_managed();
        // Only an external installation can be offered for replacement; the host's helper decides which ones.
        let replaceable = externally_managed && super::install::host_replaceable().await;
        let interfaces = super::device::enumerate_vhci_interfaces()?;
        let nodes = super::device::enumerate_transport_nodes()?;
        let installation = match super::manager::find_installation() {
            Ok(installation) => installation,
            Err(detail) => {
                let registered_version = super::installed_usbip_version();
                let partially_installed =
                    externally_managed || !matches!(registered_version, Ok(None));
                return Ok(UsbipStatus {
                    externally_managed,
                    replaceable,
                    supported: true,
                    installed: partially_installed,
                    ready: false,
                    version: registered_version.ok().flatten().unwrap_or_default(),
                    version_valid: false,
                    reboot_recommended: false,
                    // A registration without a usable executable is also a
                    // partial uninstall and must go through shared cleanup.
                    vhci_residual: !interfaces.is_empty()
                        || !nodes.is_empty()
                        || partially_installed,
                    error_code: detail_error_code(&detail),
                    attached_devices: Vec::new(),
                    detail,
                });
            }
        };
        let version_valid = super::supported_usbip_installed(Some(&installation.version));
        // A disabled node under the other enumerator is not a residual: the
        // live controller is the single present interface, on ROOT\USB or
        // ROOT\ALKAIDUSB. An enabled phantom or a second interface still blocks.
        let vhci_residual = super::device::transport_not_ready(&interfaces, &nodes);
        if !version_valid {
            let detail = format!(
                "USBIP-SETUP-003: externally managed USB/IP is incompatible; choose whether to replace it in Windows Installed apps; only {REQUIRED_VERSION} is supported"
            );
            return Ok(UsbipStatus {
                externally_managed,
                replaceable,
                supported: true,
                installed: true,
                ready: false,
                version: installation.version,
                version_valid,
                reboot_recommended: false,
                vhci_residual,
                error_code: detail_error_code(&detail),
                attached_devices: Vec::new(),
                detail,
            });
        }
        match super::exec::list_attached().await {
            Ok(attached_devices) => Ok(UsbipStatus {
                externally_managed,
                replaceable,
                supported: true,
                installed: true,
                // Residual instances break every usbip.exe operation; reflect
                // that even if a race let this probe succeed.
                ready: !vhci_residual,
                version: installation.version,
                version_valid,
                reboot_recommended: false,
                vhci_residual,
                error_code: String::new(),
                attached_devices,
                detail: String::new(),
            }),
            Err(detail) => Ok(UsbipStatus {
                externally_managed,
                replaceable,
                supported: true,
                installed: true,
                ready: false,
                version: installation.version,
                version_valid,
                reboot_recommended: false,
                vhci_residual,
                error_code: detail_error_code(&detail),
                attached_devices: Vec::new(),
                detail,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::detail_error_code;

    #[test]
    fn extracts_only_well_shaped_codes() {
        assert_eq!(
            detail_error_code("USBIP-EXEC-006: the shared transport"),
            "USBIP-EXEC-006"
        );
        assert_eq!(
            detail_error_code("USBIP-SETUP-001: not installed"),
            "USBIP-SETUP-001"
        );
        assert_eq!(detail_error_code("USBIP-EXEC-006"), "USBIP-EXEC-006");
        // 非码前缀的 detail 必须得到空串，前端不得据此推断语义
        assert_eq!(
            detail_error_code("USB/IP passthrough is only supported"),
            ""
        );
        assert_eq!(detail_error_code("usbip-exec-006: lowercase"), "");
        assert_eq!(detail_error_code("USBIP-EXEC-6: short number"), "");
        assert_eq!(detail_error_code(""), "");
    }
}
