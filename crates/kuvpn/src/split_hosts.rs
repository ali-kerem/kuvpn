#[cfg(any(unix, test))]
use std::net::IpAddr;
use std::net::Ipv4Addr;
#[cfg(unix)]
use std::net::ToSocketAddrs;

/// Validate a split destination without performing DNS lookups.
pub fn parse_split_host(value: &str) -> Result<String, String> {
    let value = value.trim();
    if let Ok(address) = value.parse::<Ipv4Addr>() {
        return Ok(address.to_string());
    }

    let hostname = value.strip_suffix('.').unwrap_or(value);
    let valid = !hostname.is_empty()
        && hostname.len() <= 253
        && !hostname
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        && hostname.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    if !valid {
        return Err(
            "Each split host must be an IPv4 address or hostname, without a port or URL."
                .to_string(),
        );
    }
    Ok(hostname.to_ascii_lowercase())
}

/// Parse comma-separated destinations, retaining compatibility with a single IP.
pub fn parse_split_hosts(value: &str) -> Result<Vec<String>, String> {
    let mut hosts = Vec::new();
    for value in value.split(',') {
        let host = parse_split_host(value)?;
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    Ok(hosts)
}

#[cfg(unix)]
pub(crate) fn resolve_split_hosts(hosts: &[String]) -> Result<Vec<Ipv4Addr>, String> {
    resolve_with(hosts, |host| {
        (host, 0)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect())
    })
}

#[cfg(any(unix, test))]
fn resolve_with(
    hosts: &[String],
    mut resolve: impl FnMut(&str) -> std::io::Result<Vec<IpAddr>>,
) -> Result<Vec<Ipv4Addr>, String> {
    if hosts.is_empty() {
        return Err("Enter at least one split destination.".to_string());
    }
    let mut addresses = Vec::new();
    for value in hosts {
        let host = parse_split_host(value)?;
        let resolved = if let Ok(address) = host.parse::<Ipv4Addr>() {
            vec![address]
        } else {
            let resolved = resolve(&host)
                .map_err(|error| format!("Could not resolve split host '{host}': {error}. Check the name and your current network's DNS."))?;
            let ipv4 = resolved
                .into_iter()
                .filter_map(|address| match address {
                    IpAddr::V4(address) => Some(address),
                    IpAddr::V6(_) => None,
                })
                .collect::<Vec<_>>();
            if ipv4.is_empty() {
                return Err(format!("Split host '{host}' has no IPv4 addresses. Split tunneling currently supports IPv4 only."));
            }
            ipv4
        };
        for address in resolved {
            if !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_destinations_are_normalized_and_deduplicated() {
        assert_eq!(
            parse_split_hosts("203.0.113.10, SSH.Example.Test.,ssh.example.test, 198.51.100.20")
                .unwrap(),
            ["203.0.113.10", "ssh.example.test", "198.51.100.20"],
        );
    }

    #[test]
    fn malformed_destinations_are_rejected_before_dns() {
        for value in [
            "",
            "203.0.113.10,",
            "::1",
            "999.0.0.1",
            "ssh..example.test",
            "https://ssh.example.test",
            "ssh.example.test:22",
            "bad host",
            "-ssh.example.test",
            "ssh_.example.test",
            "203.0.113.0/24",
        ] {
            assert!(
                parse_split_hosts(value).is_err(),
                "unexpectedly accepted {value}"
            );
        }
        assert!(parse_split_host(&format!("{}.example.test", "x".repeat(64))).is_err());
    }

    #[test]
    fn ipv4_literals_do_not_require_dns() {
        let addresses = resolve_with(&["203.0.113.10".to_string()], |_| {
            panic!("literal IP triggered DNS")
        })
        .unwrap();
        assert_eq!(addresses, ["203.0.113.10".parse::<Ipv4Addr>().unwrap()]);
    }

    #[test]
    fn all_ipv4_answers_are_routed_once_and_ipv6_is_ignored() {
        let hosts = parse_split_hosts("203.0.113.10,ssh.example.test,alias.example.test").unwrap();
        let mut queried = Vec::new();
        let addresses = resolve_with(&hosts, |host| {
            queried.push(host.to_string());
            Ok(vec![
                "198.51.100.20".parse().unwrap(),
                "2001:db8::1".parse().unwrap(),
                "203.0.113.10".parse().unwrap(),
            ])
        })
        .unwrap();
        assert_eq!(queried, ["ssh.example.test", "alias.example.test"]);
        assert_eq!(
            addresses,
            [
                "203.0.113.10".parse::<Ipv4Addr>().unwrap(),
                "198.51.100.20".parse().unwrap()
            ]
        );
    }

    #[test]
    fn unresolved_or_ipv6_only_hosts_do_not_create_a_partial_tunnel() {
        assert!(resolve_with(&[], |_| unreachable!()).is_err());
        let hosts = parse_split_hosts("203.0.113.10,ssh.example.test").unwrap();
        let error = resolve_with(&hosts, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "DNS lookup failed",
            ))
        })
        .unwrap_err();
        assert!(error.contains("Could not resolve split host 'ssh.example.test'"));
        let error = resolve_with(&hosts, |_| Ok(vec!["2001:db8::1".parse().unwrap()])).unwrap_err();
        assert!(error.contains("has no IPv4 addresses"));
    }
}
