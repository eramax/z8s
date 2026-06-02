//! Declarative capability profiles (C3) for host-native and privileged workloads.

pub const ANNOTATION_CAP_PROFILE: &str = "z8s.io/cap-profile";

/// Merge a pod/container cap profile annotation with securityContext.
pub fn apply_cap_profile(
    profile: &str,
    privileged: bool,
    extra_caps: Vec<String>,
) -> (bool, Vec<String>) {
    let mut privileged = privileged;
    let mut extra = extra_caps;
    match profile.trim() {
        "privileged" => privileged = true,
        "container-minimal" => {}
        "host-native" => merge_caps(&mut extra, &["DAC_READ_SEARCH"]),
        "host-dhcp" => merge_caps(
            &mut extra,
            &["NET_RAW", "NET_ADMIN", "NET_BIND_SERVICE", "SYS_CHROOT"],
        ),
        "host-sshd" => merge_caps(
            &mut extra,
            &[
                "CHOWN",
                "SETUID",
                "SETGID",
                "DAC_OVERRIDE",
                "SYS_CHROOT",
                "NET_BIND_SERVICE",
            ],
        ),
        other => tracing::warn!("Unknown capability profile '{}', ignoring", other),
    }
    (privileged, extra)
}

fn merge_caps(extra: &mut Vec<String>, add: &[&str]) {
    for cap in add {
        let upper = cap.to_uppercase();
        if !extra.iter().any(|c| c.eq_ignore_ascii_case(&upper)) {
            extra.push(upper);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privileged_profile_sets_flag() {
        let (p, _) = apply_cap_profile("privileged", false, vec![]);
        assert!(p);
    }

    #[test]
    fn host_dhcp_adds_net_caps() {
        let (_, caps) = apply_cap_profile("host-dhcp", false, vec![]);
        assert!(caps.iter().any(|c| c.eq_ignore_ascii_case("NET_ADMIN")));
    }
}
