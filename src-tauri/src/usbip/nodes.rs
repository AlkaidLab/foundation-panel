//! What the panel knows about the usbip-win2 host-controller devnodes, and how a removal that did not finish is told to the person.
//!
//! This file has no Windows calls: the readiness rule, the removal targets and the failure classification are plain data
//! logic, tested on any platform. `devnode.rs` reads the nodes and removes them.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

/// CONFIGFLAG_DISABLED from the Windows device-installation flags. A disabled
/// devnode remains under Enum and does not publish a present VHCI interface, so
/// usbip.exe does not treat it as a second host controller.
pub(super) const CONFIGFLAG_DISABLED: u32 = 0x0000_0001;

/// The hardware ID every usbip-win2 host controller carries, whichever root enumerator names the instance.
pub(super) fn is_vhci_hardware_id(value: &str) -> bool {
    value.eq_ignore_ascii_case(r"ROOT\USBIP_WIN2\UDE")
}

/// Device interface symbolic links are laid out as
/// `\\?\ROOT#USB#0001#{interface-guid}`; the portion before the GUID brace is
/// the device instance ID with `#` separators.
pub(super) fn instance_id_from_interface_path(path: &str) -> Option<String> {
    let body = path.strip_prefix(r"\\?\")?;
    let head = body.split_once('{')?.0;
    let head = head.strip_suffix('#')?;
    if head.is_empty() {
        return None;
    }
    Some(head.replace('#', "\\"))
}

/// One usbip-win2 host-controller devnode. Older installs use ROOT\USB; a
/// later root uses ROOT\ALKAIDUSB. Both keep the same hardware ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TransportNode {
    pub instance_id: String,
    pub disabled: bool,
    /// In the device tree now (`CM_Locate_DevNode` finds it without `PHANTOM`). A registry key under Enum that the tree
    /// no longer holds is a ghost: Windows will not start it, usbip.exe cannot see it, and nothing can come of it.
    pub present: bool,
}

/// Whether the transport must be reported not ready.
///
/// usbip.exe fails when more than one present VHCI interface exists. Only a node that is in the device tree can become a
/// second controller, so only a present node counts:
/// - an enabled present node that is not the one behind the single live interface is the leftover of an old version (the
///   new driver matches its hardware ID, so Windows may start it as a second controller) and blocks;
/// - a present node with no live interface at all is a controller that does not work and blocks;
/// - a ghost (not present) never blocks, and neither does a disabled node next to the live controller (the previous
///   enumerator after the controller moved between ROOT\USB and ROOT\ALKAIDUSB).
pub(super) fn transport_not_ready(interfaces: &[String], nodes: &[TransportNode]) -> bool {
    if interfaces.len() > 1 {
        return true;
    }
    if interfaces.is_empty() {
        return nodes.iter().any(|node| node.present);
    }
    let live = instance_id_from_interface_path(&interfaces[0]);
    nodes.iter().any(|node| {
        node.present
            && !node.disabled
            && live
                .as_ref()
                .is_none_or(|id| !node.instance_id.eq_ignore_ascii_case(id))
    })
}

/// What kind of leftover blocks the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Residual {
    Clear,
    /// One controller is live and works; older controllers of the same driver are still in the device tree beside it.
    /// Removing those is enough, nothing needs to be reinstalled.
    Stale,
    /// No working controller, or more than one live interface: the transport has to be cleaned and installed again.
    Broken,
}

impl Residual {
    /// The value the frontend reads (`residual_kind`): empty when nothing blocks.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "",
            Self::Stale => "stale",
            Self::Broken => "broken",
        }
    }
}

pub(super) fn residual_kind(interfaces: &[String], nodes: &[TransportNode]) -> Residual {
    if !transport_not_ready(interfaces, nodes) {
        Residual::Clear
    } else if interfaces.len() == 1 {
        Residual::Stale
    } else {
        Residual::Broken
    }
}

/// The devnodes to remove.
///
/// `keep_live` keeps the controller behind the single live interface (the person has a working transport and only the
/// leftovers go: enabled nodes that are not the live one, present or ghost; a disabled node is left alone, it is the
/// previous enumerator). Without it every node and every interface owner goes (the whole transport is being removed).
pub(super) fn removal_targets(
    interfaces: &[String],
    nodes: &[TransportNode],
    keep_live: bool,
) -> Vec<String> {
    let live = if keep_live && interfaces.len() == 1 {
        instance_id_from_interface_path(&interfaces[0])
    } else {
        None
    };
    let mut targets: Vec<String> = Vec::new();
    for node in nodes {
        if keep_live && node.disabled && !node.present {
            continue;
        }
        if keep_live && node.disabled && live.is_some() {
            continue;
        }
        if live
            .as_ref()
            .is_some_and(|id| node.instance_id.eq_ignore_ascii_case(id))
        {
            continue;
        }
        targets.push(node.instance_id.clone());
    }
    if live.is_none() {
        targets.extend(
            interfaces
                .iter()
                .filter_map(|path| instance_id_from_interface_path(path)),
        );
    }
    targets.sort_unstable_by_key(|id| id.to_ascii_uppercase());
    targets.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    targets
}

// Config Manager return codes (cfgmgr32.h) the panel tells apart.
pub(super) const CR_NO_SUCH_DEVNODE: u32 = 13;
pub(super) const CR_REMOVE_VETOED: u32 = 23;
pub(super) const CR_NOT_DISABLEABLE: u32 = 40;
pub(super) const CR_ACCESS_DENIED: u32 = 51;
pub(super) const ERROR_ACCESS_DENIED: u32 = 5;

/// How one removal step failed: the call that failed and the codes it returned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct NodeFailure {
    /// `DiUninstallDevice`, `CM_Query_And_Remove_SubTree` or `CM_Uninstall_DevNode`.
    pub step: &'static str,
    pub win32: Option<u32>,
    pub cr: Option<u32>,
    /// `PNP_VETO_TYPE` value and the name of what vetoed (a driver, a service, a device).
    pub veto: Option<(i32, String)>,
}

/// How one removal ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NodeRemoval {
    Removed,
    /// Windows accepted the removal and finishes it at the next restart (`NeedReboot`, or the node is still there).
    RebootRequired,
    /// Every step that was tried, in order (the first is the one the panel prefers; the fallback follows it).
    Failed(Vec<NodeFailure>),
}

/// What the person is told to do next. Each reason has its own code and its own next step in the panel's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CleanupReason {
    /// Windows needs a restart to finish removing the controller.
    RebootRequired,
    /// The administrator token was refused (or lacks the privilege): the controller can be removed by hand.
    AccessDenied,
    /// Something holds the controller (the vetoing driver, an attached device, a running stream).
    InUse,
    Unknown,
}

impl CleanupReason {
    /// The structured error code the panel shows text for.
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::RebootRequired => "USBIP-CLEAN-006",
            Self::AccessDenied => "USBIP-CLEAN-007",
            Self::InUse => "USBIP-CLEAN-008",
            Self::Unknown => "USBIP-CLEAN-002",
        }
    }
}

/// What the person can do about the failed steps of one removal: a veto says "in use" even when the other step was refused.
pub(super) fn classify_failures(failures: &[NodeFailure]) -> CleanupReason {
    let vetoed = |failure: &NodeFailure| failure.cr == Some(CR_REMOVE_VETOED) || failure.veto.is_some();
    let denied = |failure: &NodeFailure| {
        failure.cr == Some(CR_ACCESS_DENIED) || failure.win32 == Some(ERROR_ACCESS_DENIED)
    };
    if failures.iter().any(vetoed) {
        CleanupReason::InUse
    } else if failures.iter().any(denied) {
        CleanupReason::AccessDenied
    } else {
        CleanupReason::Unknown
    }
}

/// A registry value that is absent is `None`; any other error stays an error.
pub(super) fn registry_optional<T>(result: std::io::Result<T>) -> Result<Option<T>, String> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "USBIP-CLEAN-002: cannot inspect VHCI registry: {error}"
        )),
    }
}

/// `CR_*` names for the diagnostics line (the number is always printed beside it).
fn cr_name(code: u32) -> &'static str {
    match code {
        CR_NO_SUCH_DEVNODE => "CR_NO_SUCH_DEVNODE",
        CR_REMOVE_VETOED => "CR_REMOVE_VETOED",
        CR_NOT_DISABLEABLE => "CR_NOT_DISABLEABLE",
        CR_ACCESS_DENIED => "CR_ACCESS_DENIED",
        _ => "CR",
    }
}

/// One line for the diagnostics: node, call, codes. Never localized and never parsed.
pub(super) fn describe_failure(instance_id: &str, failure: &NodeFailure) -> String {
    let mut parts = vec![format!("node={instance_id}"), format!("call={}", failure.step)];
    if let Some(code) = failure.cr {
        parts.push(format!("cr={code}({})", cr_name(code)));
    }
    if let Some(code) = failure.win32 {
        parts.push(format!("win32={code}"));
    }
    if let Some((kind, name)) = &failure.veto {
        parts.push(format!("veto_type={kind}"));
        if !name.is_empty() {
            parts.push(format!("veto_name={name}"));
        }
    }
    parts.join(" ")
}

/// The error the helper answers with: the code the panel has text for, then the technical lines.
pub(super) fn cleanup_error(reason: CleanupReason, details: &[String]) -> String {
    let summary = match reason {
        CleanupReason::RebootRequired => "restart Windows to finish removing the USB/IP host controller",
        CleanupReason::AccessDenied => "Windows refused to remove the USB/IP host controller",
        CleanupReason::InUse => "the USB/IP host controller is in use and Windows did not release it",
        CleanupReason::Unknown => "residual USB/IP host controller devices could not be removed",
    };
    if details.is_empty() {
        format!("{}: {summary}", reason.code())
    } else {
        format!("{}: {summary} [{}]", reason.code(), details.join("; "))
    }
}

/// The restart marker: the removal was accepted but finishes at the next restart. Written by the unelevated side after
/// the helper's answer, read by the status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RestartMarker {
    /// When the marker was written (seconds since the Unix epoch).
    pub written: u64,
    /// How many times the panel has already continued the cleanup after a restart.
    pub resumed: u32,
}

/// A cleanup that waited for a restart is continued once Windows has restarted since the marker was written and the
/// leftover is still there; it is tried this many times at most, then the person is told how to remove it by hand.
pub(super) const MAX_RESUMES: u32 = 1;

impl RestartMarker {
    pub(super) fn encode(&self) -> String {
        format!("{}\t{}\n", self.written, self.resumed)
    }

    pub(super) fn decode(text: &str) -> Option<Self> {
        let mut parts = text.trim().split('\t');
        Some(Self {
            written: parts.next()?.parse().ok()?,
            resumed: parts.next()?.parse().ok()?,
        })
    }

    /// `boot` is when this Windows session started (seconds since the Unix epoch).
    pub(super) fn restarted_since(&self, boot: u64) -> bool {
        boot > self.written
    }

    pub(super) fn may_resume(&self) -> bool {
        self.resumed < MAX_RESUMES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(instance_id: &str, disabled: bool, present: bool) -> TransportNode {
        TransportNode {
            instance_id: instance_id.to_string(),
            disabled,
            present,
        }
    }

    fn live(path: &str) -> Vec<String> {
        vec![format!(
            r"\\?\{path}#{{b4030c06-dc5f-4fcc-87eb-e5515a0935c0}}"
        )]
    }

    #[test]
    fn only_missing_registry_values_are_treated_as_absent() {
        assert_eq!(registry_optional(Ok(7)).unwrap(), Some(7));
        assert_eq!(
            registry_optional::<()>(Err(std::io::ErrorKind::NotFound.into())).unwrap(),
            None
        );
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::Other,
            std::io::ErrorKind::InvalidData,
        ] {
            assert!(registry_optional::<()>(Err(kind.into())).is_err());
        }
    }

    #[test]
    fn a_present_second_node_beside_a_live_interface_blocks() {
        let interfaces = live("ROOT#USB#0001");
        assert!(!transport_not_ready(
            &interfaces,
            &[node(r"root\usb\0001", false, true)]
        ));
        assert!(transport_not_ready(
            &interfaces,
            &[
                node(r"ROOT\USB\0001", false, true),
                node(r"ROOT\USB\0002", false, true)
            ]
        ));
        assert!(transport_not_ready(
            &[interfaces[0].clone(), interfaces[0].clone()],
            &[]
        ));
    }

    /// The 0.9.7.7 → 0.9.8.1 state: the old controller is a ghost under Enum, the new one is live.
    #[test]
    fn a_ghost_of_the_old_version_does_not_block_the_new_one() {
        let interfaces = live("ROOT#USB#0001");
        assert!(!transport_not_ready(
            &interfaces,
            &[
                node(r"ROOT\USB\0000", false, false),
                node(r"ROOT\USB\0001", false, true)
            ]
        ));
        // …and with no live interface and nothing but ghosts, the transport is simply absent.
        assert!(!transport_not_ready(&[], &[node(r"ROOT\USB\0000", false, false)]));
        assert!(!transport_not_ready(&[], &[]));
    }

    #[test]
    fn a_controller_in_the_tree_without_an_interface_blocks() {
        assert!(transport_not_ready(&[], &[node(r"ROOT\USB\0000", false, true)]));
        assert!(transport_not_ready(&[], &[node(r"ROOT\USB\0000", true, true)]));
        assert!(transport_not_ready(
            &[],
            &[node(r"ROOT\ALKAIDUSB\0000", true, true)]
        ));
    }

    #[test]
    fn a_disabled_previous_root_does_not_block_either_live_name() {
        let alk = live("ROOT#ALKAIDUSB#0000");
        assert!(!transport_not_ready(
            &alk,
            &[
                node(r"ROOT\USB\0000", true, true),
                node(r"ROOT\ALKAIDUSB\0000", false, true),
            ]
        ));
        let usb = live("ROOT#USB#0000");
        assert!(!transport_not_ready(
            &usb,
            &[
                node(r"ROOT\USB\0000", false, true),
                node(r"ROOT\ALKAIDUSB\0000", true, true),
            ]
        ));
    }

    #[test]
    fn a_leftover_beside_a_working_controller_is_stale_not_broken() {
        let interfaces = live("ROOT#USB#0001");
        let both = [
            node(r"ROOT\USB\0000", false, true),
            node(r"ROOT\USB\0001", false, true),
        ];
        assert_eq!(residual_kind(&interfaces, &both), Residual::Stale);
        assert_eq!(residual_kind(&interfaces, &both[1..]), Residual::Clear);
        assert_eq!(
            residual_kind(&[], &[node(r"ROOT\USB\0000", false, true)]),
            Residual::Broken
        );
        assert_eq!(
            residual_kind(&[interfaces[0].clone(), interfaces[0].clone()], &[]),
            Residual::Broken
        );
        assert_eq!(residual_kind(&[], &[]), Residual::Clear);
        assert_eq!(Residual::Stale.as_str(), "stale");
        assert_eq!(Residual::Clear.as_str(), "");
    }

    #[test]
    fn derives_instance_ids_from_interface_paths() {
        assert_eq!(
            instance_id_from_interface_path(
                r"\\?\ROOT#USB#0001#{b4030c06-dc5f-4fcc-87eb-e5515a0935c0}"
            ),
            Some("ROOT\\USB\\0001".to_string())
        );
        assert_eq!(
            instance_id_from_interface_path(
                r"\\?\ROOT#ALKAIDUSB#0000#{b4030c06-dc5f-4fcc-87eb-e5515a0935c0}"
            ),
            Some("ROOT\\ALKAIDUSB\\0000".to_string())
        );
        assert_eq!(instance_id_from_interface_path(r"\\?\ROOT#USB#0001"), None);
        assert_eq!(instance_id_from_interface_path(r"\\?\#{guid}"), None);
        assert_eq!(instance_id_from_interface_path("C:\\plain\\path"), None);
    }

    #[test]
    fn recognizes_only_the_usbip_win2_vhci_hardware_id() {
        assert!(is_vhci_hardware_id(r"ROOT\USBIP_WIN2\UDE"));
        assert!(is_vhci_hardware_id(r"root\usbip_win2\ude"));
        assert!(!is_vhci_hardware_id(r"ROOT\USB\0000"));
        assert!(!is_vhci_hardware_id(r"ROOT\HIDMAESTRO_UDE"));
    }

    #[test]
    fn keeping_the_live_controller_removes_only_the_enabled_leftovers() {
        let interfaces = live("ROOT#USB#0001");
        let nodes = [
            node(r"ROOT\USB\0000", false, false),
            node(r"ROOT\USB\0001", false, true),
            node(r"ROOT\USB\0002", false, true),
            node(r"ROOT\ALKAIDUSB\0000", true, true),
        ];
        assert_eq!(
            removal_targets(&interfaces, &nodes, true),
            vec![r"ROOT\USB\0000".to_string(), r"ROOT\USB\0002".to_string()]
        );
    }

    #[test]
    fn removing_the_whole_transport_removes_every_node_and_interface_owner() {
        let interfaces = live("ROOT#USB#0001");
        let nodes = [
            node(r"ROOT\USB\0000", false, false),
            node(r"ROOT\USB\0001", false, true),
            node(r"ROOT\ALKAIDUSB\0000", true, true),
        ];
        assert_eq!(
            removal_targets(&interfaces, &nodes, false),
            vec![
                r"ROOT\ALKAIDUSB\0000".to_string(),
                r"ROOT\USB\0000".to_string(),
                r"ROOT\USB\0001".to_string()
            ]
        );
        // an interface whose node the registry scan did not list is still a target
        assert_eq!(
            removal_targets(&interfaces, &[], false),
            vec![r"ROOT\USB\0001".to_string()]
        );
    }

    #[test]
    fn without_a_single_live_interface_nothing_is_kept() {
        let nodes = [
            node(r"ROOT\USB\0000", false, true),
            node(r"ROOT\USB\0001", false, true),
        ];
        assert_eq!(removal_targets(&[], &nodes, true).len(), 2);
    }

    #[test]
    fn failures_are_told_apart_by_what_the_person_can_do() {
        let vetoed = NodeFailure {
            step: "CM_Query_And_Remove_SubTree",
            cr: Some(CR_REMOVE_VETOED),
            veto: Some((3, "usbip2_ude".to_string())),
            ..Default::default()
        };
        assert_eq!(classify_failures(&[vetoed.clone()]), CleanupReason::InUse);
        let denied_cr = NodeFailure {
            step: "CM_Uninstall_DevNode",
            cr: Some(CR_ACCESS_DENIED),
            ..Default::default()
        };
        assert_eq!(classify_failures(&[denied_cr.clone()]), CleanupReason::AccessDenied);
        let denied_win32 = NodeFailure {
            step: "DiUninstallDevice",
            win32: Some(ERROR_ACCESS_DENIED),
            ..Default::default()
        };
        assert_eq!(classify_failures(&[denied_win32.clone()]), CleanupReason::AccessDenied);
        // the veto wins over a refusal of the other step: the person has something to close first
        assert_eq!(
            classify_failures(&[denied_win32.clone(), vetoed]),
            CleanupReason::InUse
        );
        assert_eq!(classify_failures(&[denied_cr, denied_win32]), CleanupReason::AccessDenied);
        let other = NodeFailure {
            step: "DiUninstallDevice",
            win32: Some(1168),
            ..Default::default()
        };
        assert_eq!(classify_failures(&[other]), CleanupReason::Unknown);
        assert_eq!(classify_failures(&[]), CleanupReason::Unknown);
    }

    #[test]
    fn the_error_starts_with_a_code_and_carries_the_codes_it_got() {
        let failure = NodeFailure {
            step: "CM_Query_And_Remove_SubTree",
            cr: Some(CR_REMOVE_VETOED),
            veto: Some((3, "usbip2_ude".to_string())),
            ..Default::default()
        };
        let line = describe_failure(r"ROOT\USB\0000", &failure);
        assert_eq!(
            line,
            r"node=ROOT\USB\0000 call=CM_Query_And_Remove_SubTree cr=23(CR_REMOVE_VETOED) veto_type=3 veto_name=usbip2_ude"
        );
        let error = cleanup_error(CleanupReason::InUse, &[line]);
        assert!(error.starts_with("USBIP-CLEAN-008: "), "{error}");
        assert!(error.contains("cr=23(CR_REMOVE_VETOED)"), "{error}");
        // every code the reasons produce is a well-shaped USBIP code (status::detail_error_code reads it)
        for reason in [
            CleanupReason::RebootRequired,
            CleanupReason::AccessDenied,
            CleanupReason::InUse,
            CleanupReason::Unknown,
        ] {
            let text = cleanup_error(reason, &[]);
            assert!(text.starts_with(reason.code()), "{text}");
            assert!(text[reason.code().len()..].starts_with(": "), "{text}");
        }
    }

    #[test]
    fn the_restart_marker_resumes_once_after_a_restart() {
        let marker = RestartMarker {
            written: 1_000,
            resumed: 0,
        };
        assert_eq!(RestartMarker::decode(&marker.encode()), Some(marker.clone()));
        assert!(!marker.restarted_since(900));
        assert!(!marker.restarted_since(1_000));
        assert!(marker.restarted_since(1_001));
        assert!(marker.may_resume());
        assert!(!RestartMarker {
            written: 1_000,
            resumed: MAX_RESUMES
        }
        .may_resume());
        assert_eq!(RestartMarker::decode(""), None);
        assert_eq!(RestartMarker::decode("x\t1"), None);
        assert_eq!(RestartMarker::decode("5"), None);
    }
}
