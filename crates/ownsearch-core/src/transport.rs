use url::Url;

pub const PLAIN_HTTP_REFUSED: &str = "policy.plain_http_refused";

pub const PLAIN_HTTP_REMEDY: &str = "set allow_plain_http_private and allow_private_instances to permit a loopback, private, or local-only host over http, or use https";

pub fn plain_http_refusal(
    url: &Url,
    allow_private: bool,
    allow_plain_http: bool,
) -> Option<String> {
    if url.scheme() != "http" {
        return None;
    }
    if !allow_plain_http {
        return Some(format!(
            "plain http is refused because allow_plain_http_private is off; {PLAIN_HTTP_REMEDY}"
        ));
    }
    if !allow_private {
        return Some(format!(
            "plain http is refused because allow_private_instances is off; {PLAIN_HTTP_REMEDY}"
        ));
    }
    if !crate::network_guard::host_is_local(url) {
        return Some(format!(
            "plain http is refused for this host because it is not loopback, private, or a local-only name; {PLAIN_HTTP_REMEDY}"
        ));
    }
    None
}

pub fn plain_http_refusal_for_url(
    raw: &str,
    allow_private: bool,
    allow_plain_http: bool,
) -> Option<String> {
    match Url::parse(raw) {
        Ok(parsed) => plain_http_refusal(&parsed, allow_private, allow_plain_http),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).expect("url")
    }

    #[test]
    fn https_is_never_refused_by_the_plain_http_rule() {
        for raw in [
            "https://1.1.1.1/",
            "https://192.168.110.26:8080/",
            "https://127.0.0.1:8888/",
        ] {
            assert_eq!(plain_http_refusal(&url(raw), false, false), None, "{raw}");
            assert_eq!(plain_http_refusal(&url(raw), true, true), None, "{raw}");
        }
    }

    #[test]
    fn a_public_host_over_plain_http_is_always_refused() {
        for (allow_private, allow_plain_http) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            assert!(
                plain_http_refusal(&url("http://1.1.1.1/"), allow_private, allow_plain_http)
                    .is_some()
            );
            assert!(
                plain_http_refusal(
                    &url("http://searx.example/"),
                    allow_private,
                    allow_plain_http
                )
                .is_some()
            );
        }
    }

    #[test]
    fn a_private_host_over_plain_http_needs_both_settings() {
        let private = url("http://192.168.110.26:8080/");
        let loopback = url("http://127.0.0.1:8888/");
        let local_name = url("http://searx.local:8080/");
        for allow_private in [false, true] {
            for allow_plain_http in [false, true] {
                let allowed = allow_private && allow_plain_http;
                assert_eq!(
                    plain_http_refusal(&private, allow_private, allow_plain_http).is_none(),
                    allowed,
                    "private allow_private={allow_private} allow_plain_http={allow_plain_http}"
                );
                assert_eq!(
                    plain_http_refusal(&loopback, allow_private, allow_plain_http).is_none(),
                    allowed,
                    "loopback allow_private={allow_private} allow_plain_http={allow_plain_http}"
                );
                assert_eq!(
                    plain_http_refusal(&local_name, allow_private, allow_plain_http).is_none(),
                    allowed,
                    "local name allow_private={allow_private} allow_plain_http={allow_plain_http}"
                );
            }
        }
    }

    #[test]
    fn the_refusal_names_the_setting_that_is_off() {
        let private = url("http://192.168.110.26:8080/");
        let missing_plain = plain_http_refusal(&private, true, false).expect("refused");
        assert!(missing_plain.contains("allow_plain_http_private"));
        let missing_private = plain_http_refusal(&private, false, true).expect("refused");
        assert!(missing_private.contains("allow_private_instances"));
        let public = plain_http_refusal(&url("http://1.1.1.1/"), true, true).expect("refused");
        assert!(public.contains("not loopback, private, or a local-only name"));
    }
}
