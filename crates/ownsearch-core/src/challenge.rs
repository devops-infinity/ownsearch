#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChallengeKind {
    Turnstile,
    HCaptcha,
    RecaptchaV2,
    RecaptchaV2Enterprise,
    RecaptchaV3,
    RecaptchaV3Enterprise,
    MtCaptcha,
}

impl ChallengeKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Turnstile => "turnstile",
            Self::HCaptcha => "hcaptcha",
            Self::RecaptchaV2 => "recaptcha-v2",
            Self::RecaptchaV2Enterprise => "recaptcha-v2-enterprise",
            Self::RecaptchaV3 => "recaptcha-v3",
            Self::RecaptchaV3Enterprise => "recaptcha-v3-enterprise",
            Self::MtCaptcha => "mtcaptcha",
        }
    }

    pub(crate) const fn task_type(self) -> &'static str {
        match self {
            Self::Turnstile => "AntiTurnstileTaskProxyLess",
            Self::HCaptcha => "HCaptchaTaskProxyLess",
            Self::RecaptchaV2 => "ReCaptchaV2TaskProxyLess",
            Self::RecaptchaV2Enterprise => "ReCaptchaV2EnterpriseTaskProxyLess",
            Self::RecaptchaV3 => "ReCaptchaV3TaskProxyLess",
            Self::RecaptchaV3Enterprise => "ReCaptchaV3EnterpriseTaskProxyLess",
            Self::MtCaptcha => "MtCaptchaTaskProxyLess",
        }
    }

    pub(crate) const fn response_field(self) -> &'static str {
        match self {
            Self::Turnstile => "cf-turnstile-response",
            Self::HCaptcha => "h-captcha-response",
            Self::RecaptchaV2 | Self::RecaptchaV2Enterprise => "g-recaptcha-response",
            Self::RecaptchaV3 | Self::RecaptchaV3Enterprise => "",
            Self::MtCaptcha => "mtcaptcha-verifiedtoken",
        }
    }

    pub(crate) const fn takes_action(self) -> bool {
        matches!(self, Self::RecaptchaV3 | Self::RecaptchaV3Enterprise)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Challenge {
    pub kind: ChallengeKind,
    pub sitekey: String,
    pub action: Option<String>,
}

const DEFAULT_ACTION: &str = "verify";

pub(crate) fn detect(document: &str) -> Option<Challenge> {
    let sitekey = sitekey_of(document)?;
    let kind = marker_kind(document)?;
    let action = kind
        .takes_action()
        .then(|| action_of(document).unwrap_or_else(|| DEFAULT_ACTION.to_owned()));
    Some(Challenge {
        kind,
        sitekey,
        action,
    })
}

fn marker_kind(document: &str) -> Option<ChallengeKind> {
    let has = |needle: &str| document.contains(needle);
    if has("challenges.cloudflare.com/turnstile") || has("cf-turnstile") {
        return Some(ChallengeKind::Turnstile);
    }
    if has("hcaptcha.com/1/api.js") || has("h-captcha") {
        return Some(ChallengeKind::HCaptcha);
    }
    if has("recaptcha/enterprise.js?render=") {
        return Some(ChallengeKind::RecaptchaV3Enterprise);
    }
    if has("recaptcha/enterprise.js") {
        return Some(ChallengeKind::RecaptchaV2Enterprise);
    }
    if has("recaptcha/api.js?render=") || has("grecaptcha.execute") {
        return Some(ChallengeKind::RecaptchaV3);
    }
    if has("google.com/recaptcha/api.js") || has("g-recaptcha") {
        return Some(ChallengeKind::RecaptchaV2);
    }
    if has("mtcaptcha.com") || has("mtcaptcha") {
        return Some(ChallengeKind::MtCaptcha);
    }
    None
}

fn sitekey_of(document: &str) -> Option<String> {
    data_sitekey(document).or_else(|| {
        ["render=", "execute('", "execute(\""]
            .iter()
            .find_map(|needle| after_needle(document, needle))
    })
}

fn data_sitekey(document: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("data-sitekey={quote}");
        let Some(start) = document.find(&needle).map(|at| at + needle.len()) else {
            continue;
        };
        let rest = &document[start..];
        let Some(end) = rest.find(quote) else {
            continue;
        };
        let key = rest[..end].trim();
        let usable = !key.is_empty()
            && key.len() <= 128
            && key.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            });
        if usable {
            return Some(key.to_owned());
        }
    }
    None
}

fn after_needle(document: &str, needle: &str) -> Option<String> {
    let start = document.find(needle)? + needle.len();
    let rest = &document[start..];
    let end = rest
        .find(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        })
        .unwrap_or(rest.len());
    let key = &rest[..end];
    let usable = !key.is_empty() && key.len() <= 128;
    usable.then(|| key.to_owned())
}

fn action_of(document: &str) -> Option<String> {
    let start = document.find("action:")? + "action:".len();
    let rest = document[start..].trim_start();
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let rest = &rest[1..];
    let end = rest.find(quote)?;
    let action = rest[..end].trim();
    let usable = !action.is_empty()
        && action.len() <= 64
        && action
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    usable.then(|| action.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turnstile_widget_yields_its_kind_and_sitekey() {
        let markup = r#"<div class="cf-turnstile" data-sitekey="fixtureturnstile"></div>"#;
        let challenge = detect(markup).expect("turnstile is detected");
        assert_eq!(challenge.kind, ChallengeKind::Turnstile);
        assert_eq!(challenge.sitekey, "fixtureturnstile");
        assert!(challenge.action.is_none());
    }

    #[test]
    fn an_hcaptcha_widget_yields_its_kind_and_sitekey() {
        let markup = r#"<div class="h-captcha" data-sitekey='fixturehcaptcha'></div>"#;
        let challenge = detect(markup).expect("hcaptcha is detected");
        assert_eq!(challenge.kind, ChallengeKind::HCaptcha);
        assert_eq!(challenge.sitekey, "fixturehcaptcha");
    }

    #[test]
    fn a_recaptcha_v2_widget_yields_its_kind() {
        let markup = r#"<div class="g-recaptcha" data-sitekey="fixturerecaptcha"></div>"#;
        assert_eq!(
            detect(markup).expect("detected").kind,
            ChallengeKind::RecaptchaV2
        );
    }

    #[test]
    fn a_recaptcha_v2_enterprise_page_yields_its_kind() {
        let markup = r#"<script src="https://www.google.com/recaptcha/enterprise.js"></script><div data-sitekey="fixtureenterprise"></div>"#;
        assert_eq!(
            detect(markup).expect("detected").kind,
            ChallengeKind::RecaptchaV2Enterprise
        );
    }

    #[test]
    fn a_recaptcha_v3_page_yields_its_kind_and_action() {
        let markup = r#"<script src="https://www.google.com/recaptcha/api.js?render=fixturethree"></script><script>grecaptcha.execute('fixturethree', {action: 'login'});</script>"#;
        let challenge = detect(markup).expect("detected");
        assert_eq!(challenge.kind, ChallengeKind::RecaptchaV3);
        assert_eq!(challenge.sitekey, "fixturethree");
        assert_eq!(challenge.action.as_deref(), Some("login"));
    }

    #[test]
    fn a_recaptcha_v3_page_without_an_action_uses_the_default() {
        let markup = r#"<script src="https://www.google.com/recaptcha/api.js?render=fixturethree"></script>"#;
        let challenge = detect(markup).expect("detected");
        assert_eq!(challenge.action.as_deref(), Some(DEFAULT_ACTION));
    }

    #[test]
    fn a_recaptcha_v3_enterprise_page_yields_its_kind() {
        let markup = r#"<script src="https://www.google.com/recaptcha/enterprise.js?render=fixtureenterprisethree"></script>"#;
        assert_eq!(
            detect(markup).expect("detected").kind,
            ChallengeKind::RecaptchaV3Enterprise
        );
    }

    #[test]
    fn a_mtcaptcha_page_yields_its_kind() {
        let markup = r#"<script src="https://service.mtcaptcha.com/mtcv1/client/mtcaptcha.min.js"></script><div class="mtcaptcha" data-sitekey="fixturemtcaptcha"></div>"#;
        assert_eq!(
            detect(markup).expect("detected").kind,
            ChallengeKind::MtCaptcha
        );
    }

    #[test]
    fn a_page_without_a_widget_is_not_a_captcha() {
        assert!(detect("<html><body>no widget here</body></html>").is_none());
        assert!(detect(r#"<div data-sitekey="abc">no marker</div>"#).is_none());
        assert!(detect(r#"<script id="anubis_challenge"></script>"#).is_none());
    }

    #[test]
    fn a_hostile_sitekey_is_refused() {
        assert!(detect(r#"<div class="cf-turnstile" data-sitekey=""><p>"#).is_none());
        assert!(detect(r#"<div class="cf-turnstile" data-sitekey="a b"></div>"#).is_none());
        let long = "a".repeat(200);
        let markup = format!(r#"<div class="cf-turnstile" data-sitekey="{long}"></div>"#);
        assert!(detect(&markup).is_none());
    }

    #[test]
    fn every_kind_names_itself_its_task_and_its_field() {
        let kinds = [
            ChallengeKind::Turnstile,
            ChallengeKind::HCaptcha,
            ChallengeKind::RecaptchaV2,
            ChallengeKind::RecaptchaV2Enterprise,
            ChallengeKind::RecaptchaV3,
            ChallengeKind::RecaptchaV3Enterprise,
            ChallengeKind::MtCaptcha,
        ];
        for kind in kinds {
            assert!(!kind.as_str().is_empty());
            assert!(kind.task_type().ends_with("ProxyLess"), "{kind:?}");
        }
        assert_eq!(
            ChallengeKind::Turnstile.response_field(),
            "cf-turnstile-response"
        );
        assert_eq!(ChallengeKind::RecaptchaV3.response_field(), "");
        assert!(ChallengeKind::RecaptchaV3.takes_action());
        assert!(!ChallengeKind::Turnstile.takes_action());
    }
}
