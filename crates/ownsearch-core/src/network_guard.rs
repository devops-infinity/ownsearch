use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub(crate) fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    let first = octets.first().copied().unwrap_or(0);
    let second = octets.get(1).copied().unwrap_or(0);
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || first == 0
        || first >= 240
        || (first == 100 && (64..=127).contains(&second))
        || (first == 198 && (18..=19).contains(&second))
        || (first == 192 && second == 0)
        || (first == 192 && second == 88 && octets.get(2).copied() == Some(99)))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_public_v4(mapped);
    }
    let segments = ip.segments();
    let documentation =
        segments.first().copied() == Some(0x2001) && segments.get(1).copied() == Some(0x0db8);
    let discard = segments.first().copied() == Some(0x0100)
        && segments.iter().skip(1).take(3).all(|segment| *segment == 0);
    let teredo = segments.first().copied() == Some(0x2001) && segments.get(1).copied() == Some(0);
    let six_to_four = segments.first().copied() == Some(0x2002);
    let nat64 =
        segments.first().copied() == Some(0x0064) && segments.get(1).copied() == Some(0xff9b);
    let ipv4_compatible = segments.iter().take(6).all(|segment| *segment == 0)
        && segments.iter().skip(6).any(|segment| *segment != 0);
    let site_local = segments
        .first()
        .copied()
        .is_some_and(|first| (0xfec0..=0xfeff).contains(&first));
    let benchmarking =
        segments.first().copied() == Some(0x2001) && segments.get(1).copied() == Some(0x0002);
    let orchid = segments.first().copied() == Some(0x2001)
        && segments.get(1).copied().is_some_and(|second| {
            (0x0010..=0x001f).contains(&second) || (0x0020..=0x002f).contains(&second)
        });
    let documentation_3fff = segments
        .first()
        .copied()
        .is_some_and(|first| (0x3fff..0x4000).contains(&first));
    let sr_v6 = segments.first().copied() == Some(0x5f00);
    !(ip.is_loopback()
        || ipv4_compatible
        || site_local
        || benchmarking
        || orchid
        || documentation_3fff
        || sr_v6
        || teredo
        || six_to_four
        || nat64
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || documentation
        || discard)
}

pub(crate) async fn url_is_public(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(address)) => is_public(IpAddr::V4(address)),
        Some(url::Host::Ipv6(address)) => is_public(IpAddr::V6(address)),
        Some(url::Host::Domain(host)) => {
            let port = url.port_or_known_default().unwrap_or(443);
            let lookup = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tokio::net::lookup_host((host, port)),
            )
            .await;
            let Ok(Ok(mut addresses)) = lookup else {
                return false;
            };
            let mut seen = false;
            let mut all_public = true;
            for address in addresses.by_ref() {
                seen = true;
                if !is_public(address.ip()) {
                    all_public = false;
                }
            }
            seen && all_public
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_and_private_addresses_are_not_public() {
        for raw in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "198.18.0.1",
            "100.64.0.1",
        ] {
            let address: IpAddr = raw.parse().expect("address");
            assert!(!is_public(address), "{raw} must not be public");
        }
    }

    #[test]
    fn a_public_address_is_public() {
        for raw in ["1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            let address: IpAddr = raw.parse().expect("address");
            assert!(is_public(address), "{raw} must be public");
        }
    }

    #[test]
    fn ipv6_loopback_and_unique_local_are_not_public() {
        for raw in ["::1", "fc00::1", "fd12:3456::1", "fe80::1", "::"] {
            let address: IpAddr = raw.parse().expect("address");
            assert!(!is_public(address), "{raw} must not be public");
        }
    }

    #[test]
    fn ipv6_tunnels_and_translation_prefixes_are_not_public() {
        for raw in [
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2002:c000:204::1",
            "64:ff9b::7f00:1",
            "64:ff9b:1::a00:1",
            "::7f00:1",
            "fec0::1",
            "fed0::1",
            "2001:2::1",
            "2001:10::1",
            "2001:11::1",
            "2001:21::1",
            "3fff::1",
            "5f00::1",
        ] {
            let address: IpAddr = raw.parse().expect("address");
            assert!(!is_public(address), "{raw} must not be public");
        }
    }

    #[test]
    fn documentation_and_reserved_ranges_are_not_public() {
        for raw in ["2001:db8::1", "100::1", "192.0.0.1", "192.88.99.1"] {
            let address: IpAddr = raw.parse().expect("address");
            assert!(!is_public(address), "{raw} must not be public");
        }
    }

    #[tokio::test]
    async fn a_loopback_url_with_a_port_is_not_public() {
        for raw in ["https://127.0.0.1:8443/", "https://[::1]:8443/"] {
            let url = url::Url::parse(raw).expect("url");
            assert!(!url_is_public(&url).await, "{raw} must not be public");
        }
        let url = url::Url::parse("https://1.1.1.1/").expect("url");
        assert!(url_is_public(&url).await);
    }

    #[test]
    fn an_ipv4_mapped_ipv6_address_uses_the_v4_rule() {
        let address: IpAddr = "::ffff:192.168.0.1".parse().expect("address");
        assert!(!is_public(address));
        let address: IpAddr = "::ffff:1.1.1.1".parse().expect("address");
        assert!(is_public(address));
    }
}
